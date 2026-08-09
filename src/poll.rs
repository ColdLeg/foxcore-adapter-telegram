//! Telegram 长轮询循环：getUpdates -> convert -> callback.emit -> observe_incoming。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use foxcore_plugin_sdk::abi_stable::std_types::{ROption, RString, RVec};
use foxcore_plugin_sdk::protocol::AdapterEvent;
use foxcore_plugin_sdk::{
    AbiLogEvent, AbiLogLevel, AdapterCallbackBox, HostApi, HostHttpRef,
    HostLogRef, HostTimeRef, HttpRequest, encode_json,
};
use serde::Deserialize;

use crate::config::TelegramConfig;
use crate::convert::{self, Update};
use crate::error::TelegramError;

/// Telegram getUpdates response envelope.
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ok: bool,
    #[serde(default)]
    result: Option<Vec<Update>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    error_code: Option<i32>,
}

/// Background long-polling task.
pub async fn polling_loop(
    host: Arc<HostApi>,
    callback: Arc<AdapterCallbackBox>,
    config: TelegramConfig,
    adapter_name: String,
    stop_flag: Arc<AtomicBool>,
) {
    let mut offset: i64 = 0;
    let mut consecutive_failures: u32 = 0;

    if let Ok(json) = encode_json("AdapterEvent", &AdapterEvent::Connected) {
        callback.emit(json).await;
    }

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            if let Ok(json) = encode_json(
                "AdapterEvent",
                &AdapterEvent::Disconnected {
                    reason: "adapter stopped".to_string(),
                },
            ) {
                callback.emit(json).await;
            }
            break;
        }

        match do_poll(
            &host.http,
            &host.time,
            &host.log,
            &config,
            offset,
            &mut consecutive_failures,
        )
        .await
        {
            Ok(updates) => {
                consecutive_failures = 0;
                for update in updates {
                    offset = offset.max(update.update_id + 1);

                    match convert::update_to_event(
                        &update,
                        &adapter_name,
                        &config,
                        &host.http,
                        &host.resource,
                    )
                    .await
                    {
                        Ok(Some(AdapterEvent::MessageReceived(ref msg))) => {
                            if let Ok(incoming_json) =
                                encode_json("IncomingMessage", msg.as_ref())
                            {
                                callback.observe_incoming(incoming_json);
                            }
                            if let Ok(event_json) = encode_json(
                                "AdapterEvent",
                                &AdapterEvent::MessageReceived(msg.clone()),
                            ) {
                                callback.emit(event_json).await;
                            }
                        }
                        Ok(Some(event)) => {
                            if let Ok(json) = encode_json("AdapterEvent", &event) {
                                callback.emit(json).await;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            host.log.log(AbiLogEvent::message(
                                AbiLogLevel::Warn,
                                "Telegram",
                                format!("update conversion failed: {e}"),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                host.log.log(AbiLogEvent::message(
                    AbiLogLevel::Warn,
                    "Telegram",
                    format!("poll error: {e}"),
                ));

                if config.max_poll_failures > 0
                    && consecutive_failures >= config.max_poll_failures
                {
                    host.log.log(AbiLogEvent::message(
                        AbiLogLevel::Error,
                        "Telegram",
                        "poll failure limit reached, stopping",
                    ));
                    if let Ok(json) = encode_json(
                        "AdapterEvent",
                        &AdapterEvent::Disconnected {
                            reason: format!("poll failure limit: {e}"),
                        },
                    ) {
                        callback.emit(json).await;
                    }
                    break;
                }
            }
        }

        if config.poll_idle_secs > 0 {
            host.time.sleep_ms(config.poll_idle_secs * 1000).await;
        }
    }
}

async fn do_poll(
    http: &HostHttpRef,
    time: &HostTimeRef,
    log: &HostLogRef,
    config: &TelegramConfig,
    offset: i64,
    failures: &mut u32,
) -> Result<Vec<Update>, TelegramError> {
    let allowed_json =
        serde_json::to_string(&config.allowed_updates).unwrap_or_else(|_| "[]".to_string());

    let url = format!(
        "https://api.telegram.org/bot{}/getUpdates?offset={offset}&timeout={}&allowed_updates={}",
        config.bot_token,
        config.poll_timeout_secs,
        url_encode(&allowed_json),
    );

    let req = HttpRequest {
        method: RString::from("GET"),
        url: RString::from(url),
        headers: RVec::new(),
        body: RVec::new(),
        timeout_ms: ROption::RSome((config.poll_timeout_secs + 10) * 1000),
        max_response_bytes: ROption::RNone,
    };

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match http.request(req.clone()).await.into_result() {
            Ok(resp) => {
                let response: GetUpdatesResponse = serde_json::from_slice(resp.body.as_slice())
                    .map_err(|e| TelegramError::json(format!("getUpdates parse: {e}")))?;

                if !response.ok {
                    let status = resp.status;
                    let code = response.error_code;
                    let desc = response.description.unwrap_or_default();

                    if code == Some(409) || status == 401 {
                        return Err(TelegramError::api(status, code, desc));
                    }

                    return Err(TelegramError::api(status, code, desc));
                }

                return Ok(response.result.unwrap_or_default());
            }
            Err(_abi_err) => {
                *failures += 1;
                if attempt >= config.max_request_attempts {
                    return Err(TelegramError::http(format!(
                        "HTTP request failed after {attempt} attempts"
                    )));
                }
                let delay_ms = config.retry_base_delay_ms
                    * 2u64.pow((attempt - 1).min(6));
                log.log(AbiLogEvent::message(
                    AbiLogLevel::Debug,
                    "Telegram",
                    format!("retry {attempt}/{} in {delay_ms}ms", config.max_request_attempts),
                ));
                time.sleep_ms(delay_ms).await;
            }
        }
    }
}

/// Minimal URL-encode for JSON string used in query params.
fn url_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            '[' => "%5B".to_string(),
            ']' => "%5D".to_string(),
            '"' => "%22".to_string(),
            ',' => "%2C".to_string(),
            ':' => "%3A".to_string(),
            ' ' => "%20".to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}