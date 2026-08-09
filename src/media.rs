//! Telegram file download, SHA-256, HostResourceApi registration.

use foxcore_plugin_sdk::abi_stable::std_types::{ROption, RString, RVec};
use foxcore_plugin_sdk::protocol::{Resource, ResourceKind, ResourceSource};
use foxcore_plugin_sdk::{
    AbiError, HostHttpRef, HostResourceRef, HttpRequest, ResourceRegistration,
    resource_registration,
};
use serde::Deserialize;

use crate::config::TelegramConfig;
use crate::error::TelegramError;

#[derive(Debug, Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    error_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct GetFileResult {
    file_id: String,
    file_unique_id: String,
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    file_path: Option<String>,
}

/// Download result: bytes + file metadata.
pub struct MediaFile {
    pub bytes: Vec<u8>,
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: u64,
    pub mime_type: Option<String>,
}

/// Download a Telegram file (two-step: getFile -> download bytes).
pub async fn download_file(
    http: &HostHttpRef,
    config: &TelegramConfig,
    file_id: &str,
) -> Result<MediaFile, TelegramError> {
    let url = format!(
        "https://api.telegram.org/bot{}/getFile",
        config.bot_token
    );

    let get_file_req = HttpRequest {
        method: RString::from("GET"),
        url: RString::from(format!("{url}?file_id={file_id}")),
        headers: RVec::new(),
        body: RVec::new(),
        timeout_ms: ROption::RSome(config.request_timeout_secs * 1000),
        max_response_bytes: ROption::RNone,
    };

    let response = do_http(http, &get_file_req).await?;
    let parsed: TelegramResponse =
        serde_json::from_slice(response.body.as_slice()).map_err(TelegramError::json)?;

    if !parsed.ok {
        return Err(TelegramError::api(
            response.status,
            parsed.error_code,
            parsed.description.unwrap_or_default(),
        ));
    }

    let result: GetFileResult = serde_json::from_value(
        parsed
            .result
            .ok_or_else(|| TelegramError::api(response.status, None, "missing result"))?,
    )
    .map_err(TelegramError::json)?;

    let Some(file_path) = result.file_path.filter(|p| !p.is_empty()) else {
        return Err(TelegramError::not_found(format!(
            "file_id `{file_id}` has no file_path"
        )));
    };

    let download_url = format!(
        "https://api.telegram.org/file/bot/{}/{}",
        config.bot_token, file_path
    );

    let download_req = HttpRequest {
        method: RString::from("GET"),
        url: RString::from(download_url),
        headers: RVec::new(),
        body: RVec::new(),
        timeout_ms: ROption::RSome(config.upload_timeout_secs * 1000),
        max_response_bytes: ROption::RNone,
    };

    let download_resp = do_http(http, &download_req).await?;
    let bytes = download_resp.body.to_vec();

    Ok(MediaFile {
        bytes,
        file_id: result.file_id,
        file_unique_id: result.file_unique_id,
        file_size: result.file_size.unwrap_or(0),
        mime_type: None,
    })
}

/// Register media with host ResourceRegistry, returning stable ResourceId.
pub async fn register_media(
    resource_api: &HostResourceRef,
    adapter_name: &str,
    media: &MediaFile,
    kind: ResourceKind,
    extra_meta: serde_json::Value,
) -> Result<foxcore_plugin_sdk::protocol::ResourceId, AbiError> {
    let mut metadata = extra_meta;
    if let Some(meta_obj) = metadata.as_object_mut() {
        meta_obj.insert(
            "telegram_file_id".into(),
            serde_json::Value::String(media.file_id.clone()),
        );
        meta_obj.insert(
            "telegram_file_unique_id".into(),
            serde_json::Value::String(media.file_unique_id.clone()),
        );
        meta_obj.insert(
            "file_size".into(),
            serde_json::Value::Number(media.file_size.into()),
        );
    }

    let digest = sha256_digest(&media.bytes);
    let resource = Resource {
        id: foxcore_plugin_sdk::protocol::ResourceId::from_sha256(&digest),
        kind,
        source: ResourceSource::AdapterBacked {
            adapter: adapter_name.to_string(),
            native_id: media.file_id.clone(),
            fallback_url: None,
        },
        metadata,
        created_at: 0,
        tags: std::collections::HashMap::new(),
    };

    let registration: ResourceRegistration = resource_registration(&resource)?;
    let id_str = resource_api.register(registration).await.into_result()?;
    Ok(foxcore_plugin_sdk::protocol::ResourceId::from(
        id_str.as_str(),
    ))
}

async fn do_http(
    http: &HostHttpRef,
    request: &HttpRequest,
) -> Result<foxcore_plugin_sdk::HttpResponse, TelegramError> {
    let resp = http
        .request(request.clone())
        .await
        .into_result()
        .map_err(|e| TelegramError::http(format!("HTTP request failed: {e}")))?;
    Ok(resp)
}

/// Pure Rust SHA-256 digest.
pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    use std::num::Wrapping;

    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let msg_len_bits = (bytes.len() as u64).wrapping_mul(8);
    let padded_len = ((bytes.len() + 9 + 63) / 64) * 64;
    let mut padded = Vec::with_capacity(padded_len);
    padded.extend_from_slice(bytes);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&msg_len_bits.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate().take(16) {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = (Wrapping(w[i - 16]) + Wrapping(s0) + Wrapping(w[i - 7]) + Wrapping(s1)).0;
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hv) = (
            h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7],
        );

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = Wrapping(hv) + Wrapping(s1) + Wrapping(ch) + Wrapping(K[i]) + Wrapping(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = Wrapping(s0) + Wrapping(maj);

            hv = g;
            g = f;
            f = e;
            e = (Wrapping(d) + temp1).0;
            d = c;
            c = b;
            b = a;
            a = (temp1 + temp2).0;
        }

        h[0] = (Wrapping(h[0]) + Wrapping(a)).0;
        h[1] = (Wrapping(h[1]) + Wrapping(b)).0;
        h[2] = (Wrapping(h[2]) + Wrapping(c)).0;
        h[3] = (Wrapping(h[3]) + Wrapping(d)).0;
        h[4] = (Wrapping(h[4]) + Wrapping(e)).0;
        h[5] = (Wrapping(h[5]) + Wrapping(f)).0;
        h[6] = (Wrapping(h[6]) + Wrapping(g)).0;
        h[7] = (Wrapping(h[7]) + Wrapping(hv)).0;
    }

    let mut digest = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        digest[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_string() {
        let digest = sha256_digest(b"");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        let digest = sha256_digest(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}