//! AbiPlugin 实现：TelegramPlugin 的生命周期与适配器方法。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use foxcore_plugin_sdk::abi_stable::std_types::{ROption, RResult, RString, RVec};
use foxcore_plugin_sdk::async_ffi::FfiFuture;
use foxcore_plugin_sdk::{
    AbiConversationYield, AbiError, AbiErrorCode, AbiLogEvent, AbiLogLevel, AbiPlugin,
    AdapterCallbackBox, AdapterDescriptor, ConversationContextBox,
    HostApi, PluginCapabilities,
    catch_panic, guarded_async, guarded_fire_and_forget,
};
use foxcore_plugin_sdk::protocol::{AdapterEvent, OutgoingMessage};

use crate::config::{CONFIG_VERSION, TelegramConfig};
use crate::convert;
use crate::poll;

const ADAPTER_NAME: &str = "telegram";

const DEFAULT_CONFIG_TOML: &str = include_str!("../default-config.toml");

// ── Plugin struct ─────────────────────────────────────────────────────

pub struct TelegramPlugin {
    host: Arc<HostApi>,
    config: Mutex<TelegramConfig>,
    adapters: Arc<Mutex<HashMap<String, Arc<AdapterRuntime>>>>,
}

struct AdapterRuntime {
    adapter_name: String,
    config: TelegramConfig,
    callback: Arc<AdapterCallbackBox>,
    task_id: Mutex<Option<u64>>,
    running: AtomicBool,
    host: Arc<HostApi>,
}

impl TelegramPlugin {
    pub fn new(host: Arc<HostApi>, config: TelegramConfig) -> Result<Self, AbiError> {
        Ok(Self {
            host,
            config: Mutex::new(config),
            adapters: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn config_token(&self) -> String {
        self.config.lock().unwrap().bot_token.clone()
    }
}

impl AbiPlugin for TelegramPlugin {
    fn capabilities(&self) -> RResult<PluginCapabilities, AbiError> {
        catch_panic(|| {
            Ok(PluginCapabilities {
                tools: RVec::new(),
                adapters: RVec::from(vec![AdapterDescriptor {
                    name: RString::from(ADAPTER_NAME),
                    description: RString::from("Telegram Bot API 适配器（HTTP 长轮询）"),
                    inbound_segments: RVec::from(vec![
                        RString::from("text"),
                        RString::from("image"),
                        RString::from("voice"),
                        RString::from("video"),
                        RString::from("file"),
                        RString::from("sticker"),
                    ]),
                    outbound_segments: RVec::from(vec![
                        RString::from("text"),
                        RString::from("image"),
                        RString::from("voice"),
                        RString::from("video"),
                        RString::from("file"),
                        RString::from("sticker"),
                        RString::from("markdown"),
                        RString::from("reply"),
                        RString::from("mention"),
                    ]),
                }]),
                conversations: RVec::new(),
                control: false,
            })
        })
        .into()
    }

    fn initialize(&self) -> FfiFuture<RResult<(), AbiError>> {
        let host = Arc::clone(&self.host);
        let config_guard = self.config.lock().unwrap().clone();
        guarded_async(async move {
            if config_guard.version < CONFIG_VERSION {
                host.log.log(AbiLogEvent::message(
                    AbiLogLevel::Info,
                    "Telegram",
                    format!(
                        "config version {} < {}, writing default",
                        config_guard.version, CONFIG_VERSION
                    ),
                ));
                let _ = host.config.save(RString::from(DEFAULT_CONFIG_TOML));
            }

            if !config_guard.has_token() {
                host.log.log(AbiLogEvent::message(
                    AbiLogLevel::Warn,
                    "Telegram",
                    "bot_token is empty; adapter will not function until configured",
                ));
            }

            host.log.log(AbiLogEvent::message(
                AbiLogLevel::Info,
                "Telegram",
                "plugin initialized",
            ));
            Ok(())
        })
    }

    // ── Tool (not supported) ──────────────────────────────────────────

    fn invoke_tool(
        &self,
        _tool_name: RString,
        _args_json: RString,
    ) -> FfiFuture<RResult<RString, AbiError>> {
        unsupported("tool")
    }

    // ── Adapter methods ────────────────────────────────────────────────

    fn adapter_start(
        &self,
        adapter: RString,
        callback: AdapterCallbackBox,
    ) -> FfiFuture<RResult<(), AbiError>> {
        let host = Arc::clone(&self.host);
        let config = self.config.lock().unwrap().clone();
        let adapter_name = adapter.to_string();
        let adapters = Arc::clone(&self.adapters);

        guarded_async(async move {
            if !config.has_token() {
                return Err(AbiError::new(
                    AbiErrorCode::InvalidArgument,
                    "bot_token is empty",
                ));
            }

            let cb_arc = Arc::new(callback);

            let runtime = Arc::new(AdapterRuntime {
                adapter_name: adapter_name.clone(),
                config: config.clone(),
                callback: Arc::clone(&cb_arc),
                task_id: Mutex::new(None),
                running: AtomicBool::new(true),
                host: Arc::clone(&host),
            });

            // Build polling future — uses Arc<HostApi> directly, no RBox clones
            let poll_host = Arc::clone(&host);
            let poll_cb = Arc::clone(&cb_arc);
            let poll_config = config.clone();
            let poll_name = adapter_name.clone();
            let poll_stop = Arc::new(AtomicBool::new(false));

            let poll_future = guarded_fire_and_forget(async move {
                poll::polling_loop(
                    poll_host,
                    poll_cb,
                    poll_config,
                    poll_name,
                    poll_stop,
                )
                .await;
            });

            let task_id = host.task.spawn(
                RString::from("telegram-poll"),
                poll_future,
            )
            .into_result()?;

            *runtime.task_id.lock().unwrap() = Some(task_id);

            // Store runtime in adapters map
            adapters.lock().unwrap().insert(adapter_name.clone(), Arc::clone(&runtime));

            host.log.log(AbiLogEvent::message(
                AbiLogLevel::Info,
                "Telegram",
                format!("adapter `{adapter_name}` started, poll task={task_id}"),
            ));

            Ok(())
        })
    }

    fn adapter_send_message(
        &self,
        _adapter: RString,
        outgoing_json: RString,
    ) -> FfiFuture<RResult<RString, AbiError>> {
        let host = Arc::clone(&self.host);
        let token = self.config_token();
        let json_str = outgoing_json.to_string();

        guarded_async(async move {
            let outgoing: OutgoingMessage =
                foxcore_plugin_sdk::decode_json("OutgoingMessage", &json_str)?;

            let (chat_id, text, reply_to) =
                convert::outgoing_to_telegram_params(&outgoing)
                    .map_err(|e| AbiError::from(e))?;

            // Only sendMessage for now; media segments need separate sendPhoto/etc.
            let url = format!(
                "https://api.telegram.org/bot{}/sendMessage",
                token
            );

            let mut body_map = serde_json::Map::new();
            body_map.insert(
                "chat_id".to_string(),
                serde_json::Value::Number(chat_id.into()),
            );
            if let Some(ref t) = text {
                body_map.insert(
                    "text".to_string(),
                    serde_json::Value::String(t.clone()),
                );
            }
            if let Some(rt) = reply_to {
                body_map.insert(
                    "reply_to_message_id".to_string(),
                    serde_json::Value::Number(rt.into()),
                );
            }

            let body_json = serde_json::Value::Object(body_map).to_string();

            let req = foxcore_plugin_sdk::HttpRequest {
                method: RString::from("POST"),
                url: RString::from(url),
                headers: RVec::from(vec![foxcore_plugin_sdk::HttpHeader {
                    name: RString::from("Content-Type"),
                    value: RString::from("application/json"),
                }]),
                body: RVec::from(body_json.into_bytes()),
                timeout_ms: ROption::RSome(30_000),
                max_response_bytes: ROption::RNone,
            };

            let resp = host.http.request(req).await.into_result()
                .map_err(|e| AbiError::new(AbiErrorCode::Http, format!("sendMessage failed: {e}")))?;

            let resp_body = String::from_utf8_lossy(resp.body.as_slice()).to_string();
            let parsed: serde_json::Value = serde_json::from_str(&resp_body)
                .map_err(|e| AbiError::invalid_argument(format!("parse sendMessage response: {e}")))?;

            if let Some(true) = parsed.get("ok").and_then(|v| v.as_bool()) {
                if let Some(msg_id) = parsed
                    .get("result")
                    .and_then(|r| r.get("message_id"))
                    .and_then(|v| v.as_i64())
                {
                    return Ok(RString::from(msg_id.to_string()));
                }
            }

            Ok(RString::from(""))
        })
    }

    fn adapter_call_api(
        &self,
        _adapter: RString,
        action: RString,
        params_json: RString,
    ) -> FfiFuture<RResult<RString, AbiError>> {
        let host = Arc::clone(&self.host);
        let token = self.config_token();

        guarded_async(async move {
            let url = format!(
                "https://api.telegram.org/bot{}/{}",
                token,
                action.as_str()
            );

            host.log.log(AbiLogEvent::message(
                AbiLogLevel::Debug,
                "Telegram",
                format!("call_api: {}", action.as_str()),
            ));

            let req = foxcore_plugin_sdk::HttpRequest {
                method: RString::from("POST"),
                url: RString::from(url),
                headers: RVec::from(vec![foxcore_plugin_sdk::HttpHeader {
                    name: RString::from("Content-Type"),
                    value: RString::from("application/json"),
                }]),
                body: RVec::from(params_json.as_str().as_bytes().to_vec()),
                timeout_ms: ROption::RSome(30_000),
                max_response_bytes: ROption::RNone,
            };

            let resp = host.http.request(req).await.into_result()
                .map_err(|e| AbiError::new(AbiErrorCode::Http, format!("call_api failed: {e}")))?;

            Ok(RString::from(
                String::from_utf8_lossy(resp.body.as_slice()).to_string(),
            ))
        })
    }

    fn adapter_stop(&self, adapter: RString) -> FfiFuture<RResult<(), AbiError>> {
        let adapter_name = adapter.to_string();
        let adapters = Arc::clone(&self.adapters);
        let runtime = {
            adapters.lock().unwrap().remove(&adapter_name)
        };

        guarded_async(async move {
            if let Some(rt) = runtime {
                rt.running.store(false, Ordering::Release);
                if let Some(task_id) = rt.task_id.lock().unwrap().take() {
                    rt.host.task.abort(task_id);
                }
                let event = AdapterEvent::Disconnected {
                    reason: "adapter stopped".to_string(),
                };
                if let Ok(json) =
                    foxcore_plugin_sdk::encode_json("AdapterEvent", &event)
                {
                    rt.callback.emit(json).await;
                }
            }
            Ok(())
        })
    }

    // ── Conversation (not supported) ───────────────────────────────────

    fn conversation_factory_start(
        &self,
        _factory: RString,
    ) -> FfiFuture<RResult<(), AbiError>> {
        unsupported("conversation")
    }

    fn conversation_applies_to(
        &self,
        _factory: RString,
        _stream_json: RString,
    ) -> RResult<bool, AbiError> {
        unsupported_sync("conversation")
    }

    fn conversation_create(
        &self,
        _factory: RString,
        _stream_json: RString,
    ) -> RResult<u64, AbiError> {
        unsupported_sync("conversation")
    }

    fn conversation_execute(
        &self,
        _conversation_id: u64,
        _context: ConversationContextBox,
    ) -> FfiFuture<RResult<AbiConversationYield, AbiError>> {
        unsupported("conversation")
    }

    fn conversation_factory_control(
        &self,
        _factory: RString,
        _command: RString,
        _params_json: RString,
    ) -> FfiFuture<RResult<RString, AbiError>> {
        unsupported("conversation")
    }

    fn conversation_drop(&self, _conversation_id: u64) -> RResult<(), AbiError> {
        unsupported_sync("conversation")
    }

    fn conversation_factory_stop(
        &self,
        _factory: RString,
    ) -> FfiFuture<RResult<(), AbiError>> {
        unsupported("conversation")
    }

    // ── Control (not supported) ────────────────────────────────────────

    fn handle_control(
        &self,
        _command: RString,
        _params_json: RString,
    ) -> FfiFuture<RResult<RString, AbiError>> {
        unsupported("control")
    }

    // ── Shutdown ───────────────────────────────────────────────────────

    fn shutdown(&self) -> FfiFuture<RResult<(), AbiError>> {
        let adapters = Arc::clone(&self.adapters);
        guarded_async(async move {
            let names: Vec<String> = {
                let mut map = adapters.lock().unwrap();
                let keys: Vec<String> = map.keys().cloned().collect();
                // Abort all background tasks
                for (_, rt) in map.drain() {
                    rt.running.store(false, Ordering::Release);
                    if let Some(task_id) = rt.task_id.lock().unwrap().take() {
                        rt.host.task.abort(task_id);
                    }
                }
                keys
            };
            let _ = names;
            Ok(())
        })
    }
}

// ── Stub helpers ──────────────────────────────────────────────────────

fn unsupported<T: Send + 'static>(
    capability: &'static str,
) -> FfiFuture<RResult<T, AbiError>> {
    guarded_async(async move {
        Err(AbiError::new(
            AbiErrorCode::NotEnabled,
            format!("plugin does not declare {capability} capability"),
        ))
    })
}

fn unsupported_sync<T>(capability: &'static str) -> RResult<T, AbiError> {
    RResult::RErr(AbiError::new(
        AbiErrorCode::NotEnabled,
        format!("plugin does not declare {capability} capability"),
    ))
}