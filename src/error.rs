//! 插件内部错误类型与 `AbiError` 的互转。

use foxcore_plugin_sdk::{AbiError, AbiErrorCode};

/// Telegram 适配器内部错误。
#[derive(Debug)]
pub enum TelegramError {
    /// 配置错误。
    Config { message: String },
    /// 网络 / 传输错误。
    Http { message: String },
    /// Telegram API 返回错误（非 200 或 `ok: false`）。
    Api {
        status: u16,
        error_code: Option<i32>,
        description: String,
    },
    /// JSON 序列化/反序列化错误。
    Json { message: String },
    /// 消息格式或参数错误。
    InvalidMessage { message: String },
    /// 插件内部逻辑错误。
    Internal { message: String },
    /// 已停止，不应再接受调用。
    Closed,
    /// 目标不存在。
    NotFound { message: String },
}

impl std::fmt::Display for TelegramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config { message } => write!(f, "配置错误：{message}"),
            Self::Http { message } => write!(f, "HTTP 错误：{message}"),
            Self::Api {
                status,
                error_code,
                description,
            } => write!(
                f,
                "Telegram API 错误：status={status} error_code={error_code:?} {description}"
            ),
            Self::Json { message } => write!(f, "JSON 错误：{message}"),
            Self::InvalidMessage { message } => write!(f, "消息格式错误：{message}"),
            Self::Internal { message } => write!(f, "内部错误：{message}"),
            Self::Closed => write!(f, "适配器已关闭"),
            Self::NotFound { message } => write!(f, "未找到：{message}"),
        }
    }
}

impl std::error::Error for TelegramError {}

impl TelegramError {
    /// 网络/HTTP 传输错误。
    #[must_use]
    pub fn http(message: impl Into<String>) -> Self {
        Self::Http {
            message: message.into(),
        }
    }

    /// Telegram API 返回 `ok: false` 的业务错误。
    #[must_use]
    pub fn api(status: u16, error_code: Option<i32>, description: impl Into<String>) -> Self {
        Self::Api {
            status,
            error_code,
            description: description.into(),
        }
    }

    /// JSON 解析失败。
    #[must_use]
    pub fn json(message: impl std::fmt::Display) -> Self {
        Self::Json {
            message: message.into(),
        }
    }

    /// 消息格式无效。
    #[must_use]
    pub fn invalid_message(message: impl Into<String>) -> Self {
        Self::InvalidMessage {
            message: message.into(),
        }
    }

    /// 插件未启动或已停止。
    #[must_use]
    pub fn closed() -> Self {
        Self::Closed
    }

    /// 目标不存在。
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }
}

impl From<TelegramError> for AbiError {
    fn from(error: TelegramError) -> Self {
        let (code, message) = match &error {
            TelegramError::Config { message } => (AbiErrorCode::InvalidArgument, message.clone()),
            TelegramError::Http { message } => (AbiErrorCode::Http, message.clone()),
            TelegramError::Api {
                status,
                error_code,
                description,
            } => {
                let message = format!("telegram API status={status} error_code={error_code:?}: {description}");
                (AbiErrorCode::Http, message)
            }
            TelegramError::Json { message } => (AbiErrorCode::InvalidArgument, message.clone()),
            TelegramError::InvalidMessage { message } => {
                (AbiErrorCode::InvalidArgument, message.clone())
            }
            TelegramError::Internal { message } => (AbiErrorCode::Internal, message.clone()),
            TelegramError::Closed => (AbiErrorCode::Closed, "适配器已停止".into()),
            TelegramError::NotFound { message } => (AbiErrorCode::NotFound, message.clone()),
        };
        AbiError::new(code, message)
    }
}

impl From<AbiError> for TelegramError {
    fn from(error: AbiError) -> Self {
        Self::Internal {
            message: format!("host API 错误：[{:?}] {}", error.code, error.message),
        }
    }
}

impl From<serde_json::Error> for TelegramError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
        }
    }
}
