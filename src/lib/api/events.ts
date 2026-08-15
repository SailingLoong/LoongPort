/**
 * 跨语言 Tauri 事件名的**唯一定义**（前端侧）。
 *
 * Rust 侧有一份逐字一致的副本：`src-tauri/src/events.rs`，其一致性闸用
 * `include_str!` 读本文件逐个比对 —— 两边分叉会**测试红**，不会静默失效。
 *
 * ⚠️ **改这里的值必须同步改 `src-tauri/src/events.rs` 对应常量**，否则 Rust 侧
 * 的 `events::consistency_tests::frontend_copies_match` 会红。
 *
 * ## 为什么单独一个文件（而不是散在各 api 模块）
 *
 * 事件名是跨文件、跨语言的契约：Rust 侧 emit、前端 listen，两边的字符串一旦分叉，
 * 界面就不刷新 / 弹错窗口，且编译过、测试绿、没有任何报错（CLAUDE.md §三点六）。
 * 历史教训：`provider-switched` 曾散在 7 处 Rust emit + 3 处前端监听，全是裸字面量。
 * 收进这里后，加新事件 = 两端各加一个常量 + 闸自动守。
 *
 * ## 什么时候该进这个文件
 *
 * **只在 Rust 侧 emit、前端不监听**的事件名不要进 —— 单侧事实没有分叉风险
 * （尺子 2）。判断标准：跨语言 = Rust emit + 前端 listen 两侧都有。
 */

/** 当前供应商切换后通知前端（`providersApi.onSwitched` / `RelaySection` 监听）。 */
export const PROVIDER_SWITCHED = "provider-switched";
/** 项目应用完成后的统一收尾事件（`App.tsx` / `PromptPanel` 监听）。 */
export const PROFILE_APPLIED = "profile-applied";
/** 用量缓存写入后通知前端 React Query 失效（`useUsageCacheBridge` 监听）。 */
export const USAGE_CACHE_UPDATED = "usage-cache-updated";
/** 使用日志写入后通知前端（`useUsageEventBridge` 监听）。 */
export const USAGE_LOG_RECORDED = "usage-log-recorded";
/** deeplink 导入请求（`DeepLinkImportDialog` 监听）。 */
export const DEEPLINK_IMPORT = "deeplink-import";
/** 统一供应商同步完成（`App.tsx` 监听）。 */
export const UNIVERSAL_PROVIDER_SYNCED = "universal-provider-synced";
/** WebDAV 云同步状态变化（`App.tsx` 监听）。 */
export const WEBDAV_SYNC_STATUS_UPDATED = "webdav-sync-status-updated";
/** S3 云同步状态变化（`App.tsx` 监听）。 */
export const S3_SYNC_STATUS_UPDATED = "s3-sync-status-updated";
/** 充值窗关闭（`RelaySection` 监听）。原在 `@/lib/api/relay`。 */
export const PURCHASE_CLOSED = "relay-purchase-closed";
/** 官网直连登录窗凭据解析失败（`RelaySection` 监听）。原在 `@/lib/api/vendor`。 */
export const VENDOR_LOGIN_ERROR = "vendor-login-error";
/** 官网账号行集合变化（`RelaySection` 监听；登录入口是 App 级页面，够不到本地状态）。 */
export const VENDOR_ACCOUNTS_CHANGED = "vendor-accounts-changed";
/** 主动模型验证任务进度变化（模型验证弹窗监听）。 */
export const MODEL_VERIFICATION_PROGRESS = "model-verification-progress";
/** 模型验证持久化结果变化（档位行与模型验证弹窗监听）。 */
export const MODEL_VERIFICATION_CHANGED = "model-verification-changed";
