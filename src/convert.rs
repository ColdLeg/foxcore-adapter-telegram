//! Telegram Update <-> FoxCore AdapterEvent/IncomingMessage/OutgoingMessage 互转。

use foxcore_plugin_sdk::abi_stable::std_types::{ROption, RString};
use foxcore_plugin_sdk::protocol::{
    AdapterEvent, ImageMeta, IncomingMessage, MessageAddressing, MessageSegment, MessageStream,
    OutgoingMessage, ResourceKind, Sender,
};
use foxcore_plugin_sdk::{HostHttpRef, HostResourceRef};
use serde::Deserialize;

use crate::config::TelegramConfig;
use crate::error::TelegramError;
use crate::media;

// ── Telegram JSON models ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub edited_message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub photo: Option<Vec<PhotoSize>>,
    #[serde(default)]
    pub voice: Option<Voice>,
    #[serde(default)]
    pub video: Option<Video>,
    #[serde(default)]
    pub document: Option<Document>,
    #[serde(default)]
    pub sticker: Option<Sticker>,
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Voice {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i32,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Video {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_unique_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Sticker {
    pub file_id: String,
    pub file_unique_id: String,
    #[serde(default)]
    pub emoji: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────

fn stream_kind(chat_type: &str) -> &str {
    match chat_type {
        "private" => "private",
        "group" | "supergroup" => "group",
        "channel" => "channel",
        _ => "group",
    }
}

fn sender_name(user: &User) -> String {
    let first = user.first_name.trim();
    match user.last_name.as_deref() {
        Some(last) if !last.trim().is_empty() => format!("{first} {}", last.trim()),
        _ => first.to_string(),
    }
}

fn acl_check(config: &TelegramConfig, chat: &Chat) -> bool {
    let kind = stream_kind(&chat.chat_type);
    let rule = config.acl_for(kind);
    rule.allow(&chat.id.to_string())
}

fn i32_to_u32_option(v: i32) -> Option<u32> {
    u32::try_from(v).ok()
}


// ── Conversion ────────────────────────────────────────────────────────

/// Telegram Message -> FoxCore IncomingMessage.
pub async fn message_to_incoming(
    msg: &Message,
    adapter_name: &str,
    is_edit: bool,
    config: &TelegramConfig,
    http: &HostHttpRef,
    resource_api: &HostResourceRef,
) -> Result<IncomingMessage, TelegramError> {
    let kind = stream_kind(&msg.chat.chat_type);
    let stream = MessageStream::new(adapter_name, kind, msg.chat.id.to_string());

    let sender = match &msg.from {
        Some(user) => Sender::new(format!("tg.{}", user.id), sender_name(user)),
        None => Sender::new(format!("tg.chat.{}", msg.chat.id), "Unknown"),
    };

    let mut segments: Vec<MessageSegment> = Vec::new();
    let mut plain_parts: Vec<String> = Vec::new();

    // Text
    if let Some(ref text) = msg.text {
        if !text.is_empty() {
            segments.push(MessageSegment::Text { text: text.clone() });
            plain_parts.push(text.clone());
        }
    }

    // Caption (skip if == text)
    if let Some(ref caption) = msg.caption {
        if !caption.is_empty() && msg.text.as_deref() != Some(caption.as_str()) {
            segments.push(MessageSegment::Text { text: caption.clone() });
            plain_parts.push(caption.clone());
        }
    }

    // Photo -> download largest, register as Image
    if let Some(ref photos) = msg.photo {
        if let Some(largest) = photos.last() {
            match media::download_file(http, config, &largest.file_id).await {
                Ok(file) => {
                    let meta = serde_json::json!({
                        "width": largest.width,
                        "height": largest.height,
                    });
                    if let Ok(rid) =
                        media::register_media(resource_api, adapter_name, &file, ResourceKind::Image, meta).await
                    {
                        segments.push(MessageSegment::Image {
                            resource_id: rid,
                            alt_text: msg.caption.clone(),
                            meta: ImageMeta {
                                animated: false,
                                width: i32_to_u32_option(largest.width),
                                height: i32_to_u32_option(largest.height),
                                size_bytes: Some(file.file_size),
                            },
                        });
                    }
                    if plain_parts.is_empty() {
                        plain_parts.push("[Image]".to_string());
                    }
                }
                Err(_) => {
                    if plain_parts.is_empty() {
                        plain_parts.push("[Image]".to_string());
                    }
                }
            }
        }
    }

    // Voice
    if let Some(ref voice) = msg.voice {
        match media::download_file(http, config, &voice.file_id).await {
            Ok(file) => {
                let meta = serde_json::json!({"duration": voice.duration});
                let rid =
                    media::register_media(resource_api, adapter_name, &file, ResourceKind::Voice, meta)
                        .await
                        .ok();
                if let Some(resource_id) = rid {
                    segments.push(MessageSegment::Voice {
                        resource_id,
                        duration_secs: i32_to_u32_option(voice.duration),
                        codec: voice.mime_type.clone(),
                    });
                }
                if plain_parts.is_empty() {
                    plain_parts.push("[Voice]".to_string());
                }
            }
            Err(_) => {
                if plain_parts.is_empty() {
                    plain_parts.push("[Voice]".to_string());
                }
            }
        }
    }

    // Video
    if let Some(ref video) = msg.video {
        match media::download_file(http, config, &video.file_id).await {
            Ok(file) => {
                let meta = serde_json::json!({
                    "width": video.width,
                    "height": video.height,
                    "duration": video.duration,
                });
                let rid =
                    media::register_media(resource_api, adapter_name, &file, ResourceKind::Video, meta)
                        .await
                        .ok();
                if let Some(resource_id) = rid {
                    segments.push(MessageSegment::Video {
                        resource_id,
                        duration_secs: i32_to_u32_option(video.duration),
                        width: i32_to_u32_option(video.width),
                        height: i32_to_u32_option(video.height),
                        codec: video.mime_type.clone(),
                    });
                }
                if plain_parts.is_empty() {
                    plain_parts.push("[Video]".to_string());
                }
            }
            Err(_) => {
                if plain_parts.is_empty() {
                    plain_parts.push("[Video]".to_string());
                }
            }
        }
    }

    // Document
    if let Some(ref doc) = msg.document {
        match media::download_file(http, config, &doc.file_id).await {
            Ok(file) => {
                let meta = serde_json::json!({
                    "file_name": doc.file_name,
                    "mime_type": doc.mime_type,
                });
                let rid =
                    media::register_media(resource_api, adapter_name, &file, ResourceKind::File, meta)
                        .await
                        .ok();
                if let Some(resource_id) = rid {
                    segments.push(MessageSegment::File {
                        resource_id,
                        file_name: doc.file_name.clone(),
                        mime: doc.mime_type.clone(),
                        size_bytes: Some(file.file_size),
                    });
                }
                if plain_parts.is_empty() {
                    plain_parts.push(format!(
                        "[File: {}]",
                        doc.file_name.as_deref().unwrap_or("unknown")
                    ));
                }
            }
            Err(_) => {
                if plain_parts.is_empty() {
                    plain_parts.push("[File]".to_string());
                }
            }
        }
    }

    // Sticker (no download - use file_id as native reference)
    if let Some(ref sticker) = msg.sticker {
        segments.push(MessageSegment::Sticker {
            sticker_id: Some(sticker.file_unique_id.clone()),
            resource_id: Some(foxcore_plugin_sdk::protocol::ResourceId::from(
                sticker.file_id.clone(),
            )),
            name: sticker.emoji.clone(),
        });
        if plain_parts.is_empty() {
            plain_parts.push(format!(
                "[Sticker: {}]",
                sticker.emoji.as_deref().unwrap_or("")
            ));
        }
    }

    let plain_text = plain_parts.join(" ");

    let addressing = if kind == "private" {
        MessageAddressing::Direct
    } else {
        MessageAddressing::Ambient
    };

    let mut metadata = serde_json::json!({
        "telegram_message_id": msg.message_id,
        "chat_type": msg.chat.chat_type,
        "is_edit": is_edit,
    });

    if let Some(ref reply_to) = msg.reply_to_message {
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert(
                "foxcore".to_string(),
                serde_json::json!({
                    "response_anchor": {
                        "reference_id": reply_to.message_id.to_string()
                    }
                }),
            );
        }
    }

    Ok(IncomingMessage::new(
        format!("tg.{}.{}", msg.chat.id, msg.message_id),
        stream,
        sender,
        segments,
        plain_text,
        msg.date,
    )
    .with_addressing(addressing)
    .with_metadata(metadata))
}

/// Telegram Update -> AdapterEvent (top-level dispatch).
/// Returns None for unsupported update types or ACL-rejected messages.
pub async fn update_to_event(
    update: &Update,
    adapter_name: &str,
    config: &TelegramConfig,
    http: &HostHttpRef,
    resource_api: &HostResourceRef,
) -> Result<Option<AdapterEvent>, TelegramError> {
    if let Some(ref msg) = update.message {
        if !acl_check(config, &msg.chat) {
            return Ok(None);
        }
        let incoming =
            message_to_incoming(msg, adapter_name, false, config, http, resource_api).await?;
        return Ok(Some(AdapterEvent::MessageReceived(Box::new(incoming))));
    }

    if let Some(ref msg) = update.edited_message {
        if !acl_check(config, &msg.chat) {
            return Ok(None);
        }
        let incoming =
            message_to_incoming(msg, adapter_name, true, config, http, resource_api).await?;
        return Ok(Some(AdapterEvent::MessageReceived(Box::new(incoming))));
    }

    if let Some(ref callback) = update.callback_query {
        return Ok(Some(AdapterEvent::Activity {
            adapter: adapter_name.to_string(),
            activity: "callback_query".to_string(),
            payload: callback.clone(),
        }));
    }

    Ok(None)
}

/// OutgoingMessage -> (chat_id, text, reply_to_message_id).
pub fn outgoing_to_telegram_params(
    outgoing: &OutgoingMessage,
) -> Result<(i64, Option<String>, Option<i64>), TelegramError> {
    let chat_id: i64 = outgoing
        .stream
        .key
        .parse()
        .map_err(|e| TelegramError::invalid_message(format!(
            "invalid chat_id `{}`: {e}",
            outgoing.stream.key
        )))?;

    let reply_to = outgoing.response_anchor.as_ref().and_then(|anchor| {
        anchor
            .message_id
            .rsplit('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
    });

    let mut text_parts: Vec<String> = Vec::new();
    for seg in &outgoing.segments {
        match seg {
            MessageSegment::Text { text } => text_parts.push(text.clone()),
            MessageSegment::Markdown { text } => text_parts.push(text.clone()),
            MessageSegment::Mention {
                display_name, ..
            } => text_parts.push(format!("@{}", display_name.as_deref().unwrap_or("unknown"))),
            _ => {}
        }
    }

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(" "))
    };

    Ok((chat_id, text, reply_to))
}