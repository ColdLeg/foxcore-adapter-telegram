//! 配置模型：Telegram Bot Token、轮询参数、超时、重试与 ACL。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// ACL 过滤模式。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AclMode {
    Disabled,
    Whitelist,
    Blacklist,
}

impl Default for AclMode {
    fn default() -> Self {
        Self::Disabled
    }
}

/// 针对一类会话的 ACL 规则。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct AclRule {
    #[serde(default)]
    pub mode: AclMode,
    #[serde(default)]
    pub list: Vec<String>,
}

impl AclRule {
    /// 检测 `chat_id` 是否被放行。`Disabled` 一律放行。
    #[must_use]
    pub fn allow(&self, chat_id: &str) -> bool {
        match self.mode {
            AclMode::Disabled => true,
            AclMode::Whitelist => self.list.iter().any(|id| id == chat_id),
            AclMode::Blacklist => !self.list.iter().any(|id| id == chat_id),
        }
    }
}

/// 插件主配置，对应 `config/plugins/foxcore-adapter-telegram.toml`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    /// 配置结构版本；低于当前版本时整份文件按默认值重建。
    pub version: u32,

    /// Telegram Bot Token（从 @BotFather 获取）。留空不启动适配器。
    #[serde(default)]
    pub bot_token: String,

    /// 长轮询超时（秒），1–100。
    #[serde(default = "default_poll_timeout_secs")]
    pub poll_timeout_secs: u64,

    /// 两次轮询间的空闲间隔（秒）；0 表示立即重试。
    #[serde(default = "default_poll_idle_secs")]
    pub poll_idle_secs: u64,

    /// 允许的 Telegram 更新类型。空数组 = 接收全部。
    #[serde(default = "default_allowed_updates")]
    pub allowed_updates: Vec<String>,

    /// 常规 API 请求超时（秒）。
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// 媒体上传/下载超时（秒）。
    #[serde(default = "default_upload_timeout_secs")]
    pub upload_timeout_secs: u64,

    /// 单次 API 调用的最大尝试次数（含首次）。
    #[serde(default = "default_max_request_attempts")]
    pub max_request_attempts: u32,

    /// 重试退避基础延迟（毫秒）。
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,

    /// 连续轮询失败次数上限；0 = 不限。
    #[serde(default = "default_max_poll_failures")]
    pub max_poll_failures: u32,

    /// 群组 ACL。
    #[serde(default)]
    pub acl_group: AclRule,

    /// C2C 私聊 ACL。
    #[serde(default)]
    pub acl_user: AclRule,
}

fn default_poll_timeout_secs() -> u64 {
    30
}
fn default_poll_idle_secs() -> u64 {
    1
}
fn default_allowed_updates() -> Vec<String> {
    vec![
        "message".into(),
        "edited_message".into(),
        "callback_query".into(),
    ]
}
fn default_request_timeout_secs() -> u64 {
    30
}
fn default_upload_timeout_secs() -> u64 {
    120
}
fn default_max_request_attempts() -> u32 {
    3
}
fn default_retry_base_delay_ms() -> u64 {
    250
}
fn default_max_poll_failures() -> u32 {
    100
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            bot_token: String::new(),
            poll_timeout_secs: default_poll_timeout_secs(),
            poll_idle_secs: default_poll_idle_secs(),
            allowed_updates: default_allowed_updates(),
            request_timeout_secs: default_request_timeout_secs(),
            upload_timeout_secs: default_upload_timeout_secs(),
            max_request_attempts: default_max_request_attempts(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
            max_poll_failures: default_max_poll_failures(),
            acl_group: AclRule::default(),
            acl_user: AclRule::default(),
        }
    }
}

impl TelegramConfig {
    /// 当前配置结构版本。与 host 侧 Config trait 的 version() 保持一致。
    pub const CURRENT_VERSION: u32 = 2;

    /// 配置文件名（host 按 plugin id 命名，此处仅作文档级引用）。
    pub const FILE_NAME: &str = "foxcore-adapter-telegram.toml";

    /// bot_token 是否已填写。
    #[must_use]
    pub fn has_token(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    /// 按 chat 类型取对应的 ACL 规则。
    #[must_use]
    pub fn acl_for(&self, chat_kind: &str) -> &AclRule {
        match chat_kind {
            "private" => &self.acl_user,
            _ => &self.acl_group,
        }
    }

    /// 收集当前 `allowed_updates` 为高效查找集。
    #[must_use]
    pub fn allowed_update_set(&self) -> HashSet<&str> {
        self.allowed_updates.iter().map(String::as_str).collect()
    }
}

/// foxcore-plugin-sdk 的 `Config` trait。由本插件直接使用而非实现 trait
/// （避免在 abi 边界暴露额外 trait），这里提供等价的独立 API。
pub const CONFIG_VERSION: u32 = TelegramConfig::CURRENT_VERSION;
