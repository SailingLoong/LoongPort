# 诊断日志规范

LoongPort 的诊断日志必须同时满足两个目标：**发生故障时能还原阶段与根因**，以及
**任何凭据和请求/响应正文都不能进入日志文件**。本页是新增日志和排查日志缺口时的
维护约定。

## 日志位置与保留

Rust 后端和 WebView 前端统一写入：

```text
<app_config_dir>/logs/loongport.log
```

崩溃时另写 `<app_config_dir>/crash.log`，其 panic message 同样复用统一脱敏策略。

默认单文件上限 20 MiB，保留 4 个轮转归档；当前文件与归档合计最多约 100 MiB。
日志级别由应用设置动态控制，默认 `info`。

## 唯一安全出口

- Rust：`src-tauri/src/diagnostics.rs` 的 `format_log_line` 是
  `tauri-plugin-log` 的最终 formatter。所有 Rust 日志以及从 WebView 进入插件的日志，
  在写入 stdout 和文件之前都会统一脱敏、截断。
- 前端：`src/lib/frontendLogger.ts` 负责结构化序列化、属性级脱敏和长度限制。
  `src/main.tsx` 启动时安装 console bridge，因此遗留和第三方 `console.*` 也会进入
  持久化日志，并且 DevTools 只显示脱敏后的文本。
- 最终 formatter 是兜底，不是允许调用方记录敏感数据的理由。新代码应在产生日志时
  就只保留安全元数据。

当前安全出口会隐藏 URL userinfo/query、认证与 Cookie 头、常见 Token/密钥形状、私钥、
命名的 secret/key/token/password/cookie 字段、请求/响应正文和 payload，并限制单条日志
长度。新增认证机制或新的敏感字段形状时，必须同时补 Rust 与前端脱敏测试。

## 结构化事件

跨阶段或需要长期检索的诊断使用稳定的单行 JSON 事件：

```rust
log::warn!(
    "{}",
    DiagnosticEvent::new("relay.browser_probe", "unmatched")
        .field_display("site", crate::url_for_log(site))
        .field("probe", safe_probe_summary)
);
```

```ts
reportFrontendLog("warn", "settings.save", "failed", {
  section: "proxy",
});
```

字段约定：

- `event`：稳定的领域动作，使用点号分层，例如 `proxy.takeover.rollback`；
- `outcome`：稳定结果，例如 `started`、`completed`、`failed`、`timeout`、`skipped`；
- 其它字段只放排查所需的安全上下文，例如 `phase`、`app`、状态码、content type、字节数；
- Rust 错误支持 `std::error::Error` 时使用 `format_error_chain`，不要只记最外层文案；
- URL 必须先走 `url_for_log`，即使最终 formatter 仍会再次兜底脱敏。

不要把事件名、字段名或 outcome 塞进自由文本；稳定字段才能被 grep 和脚本可靠聚合。

## 必须记录的 best-effort 失败

“主流程继续执行”不等于“错误可以静默”。以下失败即使不向用户返回错误，也必须记录：

- 数据库状态、迁移标志或同步状态无法持久化；
- 配置接管、恢复、回滚和备份清理失败；
- 后台任务未启动、任务崩溃、通道关闭或最终执行失败；
- 用户需要看到的安全/认证警告无法发送；
- 写入 Live 配置失败。

Rust `Result` 在这些路径使用 `ResultLogExt::warn_on_err` 或 `error_on_err`，并提供
`DiagnosticEvent` 的阶段和对象上下文。

以下情况可以不升为 warning：窗口 show/focus、已关闭窗口的事件发送、预期的编辑中
JSON/TOML 解析 fallback、用于合并高频变更的通道 Full。若这类跳过会导致后台 worker
永久失效或数据状态不一致，则不再属于“预期”，必须记录。

## 严禁写入日志

- `Authorization`、Cookie、Bearer/API key、OAuth token、密码、私钥；
- 请求正文、响应正文、Cloudflare/验证页 HTML、完整 payload；
- 完整 Provider/Settings 对象；
- 含敏感 query 的原始 URL；
- 为“方便排查”而输出 localStorage、认证头或 WebView 会话数据。

协议探针只记录候选 ID、HTTP 状态、content type、正文长度、是否 JSON-like 和安全错误分类；
Bearer token 只在目标 origin 的 WebView 请求内使用，不回传 Rust，也不进入日志。

## 新日志的验收

至少检查：

1. 正常、失败、超时/跳过三个关键结果能否区分；
2. 日志是否包含动作阶段、对象和完整安全错误链；
3. 测试是否覆盖新增敏感形状，且断言原文不出现；
4. `cargo test diagnostics::tests --lib` 与 `tests/lib/frontendLogger*.test.ts` 通过；
5. `cargo clippy --all-targets --all-features -- -D warnings`、TypeScript、Prettier 和完整
   Vitest 通过。
