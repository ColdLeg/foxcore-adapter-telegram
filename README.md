# foxcore-adapter-telegram

FoxCore Telegram Bot API 动态适配器插件。

通过 HTTP 长轮询接入 Telegram Bot API，将 Telegram 消息转换为 FoxCore 的
`IncomingMessage`，并将核心生成的 `OutgoingMessage` 发回 Telegram。

## 配置

插件配置文件为 `config/plugins/foxcore-adapter-telegram.toml`，默认内容见
`default-config.toml`。首次加载时若文件不存在，host 会自动写入默认配置。

## 许可

与 FoxCore Plugin SDK 相同的双重授权。
