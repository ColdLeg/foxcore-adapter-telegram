//! FoxCore Telegram Bot API 动态适配器插件。
//!
//! 通过 HTTP 长轮询接入 Telegram，将 Telegram 消息转为 FoxCore 的
//! `IncomingMessage`，或将核心生成的 `OutgoingMessage` 发回 Telegram。
//!
//! 插件名：`foxcore-adapter-telegram`
//! 适配器名：`telegram`
//! ABI 版本：1.4

extern crate foxcore_plugin_sdk as abi_stable;

use std::sync::Arc;

use foxcore_plugin_sdk::abi_stable::export_root_module;
use foxcore_plugin_sdk::abi_stable::prefix_type::PrefixTypeTrait;
use foxcore_plugin_sdk::abi_stable::sabi_extern_fn;
use foxcore_plugin_sdk::abi_stable::sabi_trait::TD_Opaque;
use foxcore_plugin_sdk::abi_stable::std_types::{RResult, RString};
use foxcore_plugin_sdk::{
    AbiError, AbiPluginBox, AbiPlugin_TO, AbiVersion, HostApi, PluginDescriptor, PluginInitInfo,
    PluginMod, PluginModRef, catch_panic,
};

mod config;
mod convert;
mod error;
mod media;
mod plugin;
mod poll;

use plugin::TelegramPlugin;

const PLUGIN_NAME: &str = "foxcore-adapter-telegram";

// ── Root module export ─────────────────────────────────────────────────

#[export_root_module]
#[must_use]
pub fn get_library() -> PluginModRef {
    PluginMod { descriptor, create }.leak_into_prefix()
}

#[sabi_extern_fn]
fn descriptor() -> RResult<PluginDescriptor, AbiError> {
    catch_panic(|| {
        Ok(PluginDescriptor {
            abi_version: AbiVersion::CURRENT,
            name: RString::from(PLUGIN_NAME),
            version: RString::from(env!("CARGO_PKG_VERSION")),
            description: RString::from("Telegram Bot API 适配器，通过 HTTP 长轮询接收/发送消息"),
        })
    })
    .into()
}

#[sabi_extern_fn]
fn create(host: HostApi, init: PluginInitInfo) -> RResult<AbiPluginBox, AbiError> {
    catch_panic(|| {
        // Parse config from init.toml or fall back to default
        let config = if init.config_toml.as_str().trim().is_empty() {
            config::TelegramConfig::default()
        } else {
            toml::from_str(init.config_toml.as_str()).unwrap_or_default()
        };

        Ok(AbiPlugin_TO::from_value(
            TelegramPlugin::new(Arc::new(host), config)?,
            TD_Opaque,
        ))
    })
    .into()
}