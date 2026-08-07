//! 跨语言 Tauri 事件名的**唯一定义**（Rust 侧）。
//!
//! 前端有一份逐字一致的副本：`src/lib/api/events.ts`。本模块底部的一致性闸用
//! `include_str!` 读那份 TS 文件逐个比对 —— 不一致会**测试红**，不会静默失效。
//!
//! ## 为什么单独一个模块（而不是散在各 emit 点所在的文件）
//!
//! 事件名是**跨文件、跨语言**的契约：Rust 侧要 emit、前端要 listen，两边的字符串
//! 一旦分叉，界面就不刷新 / 弹错窗口，且编译过、测试绿、没有任何报错
//! （CLAUDE.md §三点六 的"同一事实散在多处 = 静默失效"）。
//!
//! 历史教训：`provider-switched` 曾散在 7 处 Rust emit + 3 处前端监听，全是裸字面量，
//! 没有任何东西守住它们一致。收进这里后，加新事件 = 两端各加一个常量 + 闸自动守。
//!
//! ## 什么时候该进这个模块
//!
//! **只在 Rust 侧 emit、前端不监听**的事件名（如 `proxy-flags-changed`、
//! `operator-login-error`）**不要进** —— 单侧事实没有分叉风险，收进来只是噪音
//! （尺子 2：不为不存在的跨语言契约预建）。判断标准：跨语言 = Rust emit + 前端 listen
//! 两侧都有。

/// 当前供应商切换后通知前端（`providersApi.onSwitched` / `OperatorSection` 监听）。
pub const PROVIDER_SWITCHED: &str = "provider-switched";
/// 项目应用完成后的统一收尾事件（`App.tsx` / `PromptPanel` 监听）。
pub const PROFILE_APPLIED: &str = "profile-applied";
/// 用量缓存写入后通知前端 React Query 失效（`useUsageCacheBridge` 监听）。
pub const USAGE_CACHE_UPDATED: &str = "usage-cache-updated";
/// 使用日志写入后通知前端（`useUsageEventBridge` 监听）。原定义在
/// `usage_events::EVENT_USAGE_LOG_RECORDED`，迁入本模块统一管理。
pub const USAGE_LOG_RECORDED: &str = "usage-log-recorded";
/// deeplink 导入请求（`DeepLinkImportDialog` 监听）。
pub const DEEPLINK_IMPORT: &str = "deeplink-import";
/// 统一供应商同步完成（`App.tsx` 监听）。
pub const UNIVERSAL_PROVIDER_SYNCED: &str = "universal-provider-synced";
/// WebDAV 云同步状态变化（`App.tsx` 监听）。
pub const WEBDAV_SYNC_STATUS_UPDATED: &str = "webdav-sync-status-updated";
/// S3 云同步状态变化（`App.tsx` 监听）。
pub const S3_SYNC_STATUS_UPDATED: &str = "s3-sync-status-updated";
/// 充值窗关闭（`OperatorSection` 监听）。原名 `commands::operator::PURCHASE_CLOSED_EVENT`。
pub const PURCHASE_CLOSED: &str = "operator-purchase-closed";
/// 官网直连登录窗凭据解析失败（`OperatorSection` 监听）。原名
/// `commands::vendor::LOGIN_ERROR_EVENT`。
pub const VENDOR_LOGIN_ERROR: &str = "vendor-login-error";

#[cfg(test)]
mod consistency_tests {
    /// 前后端各存一份的事件名**必须逐字一致**。
    ///
    /// 跨语言编译器管不到 `.ts`，不一致时不报错、不崩溃 —— 只是事件发出去前端收不到、
    /// 界面不刷新。这道闸把那类问题从「静默失效」变成「测试红」。
    ///
    /// **新增跨语言事件时往下面的表里加一行。** 判据（CLAUDE.md §三点六）：
    /// 凡「同一事实同时存在于 Rust 与非 Rust 文件」，就该在这里对上。
    /// 形状照 `config.rs::brand_constant_consistency::frontend_copies_match`。
    #[test]
    fn frontend_copies_match() {
        let ts = include_str!("../../src/lib/api/events.ts");

        // (TS 里的常量名, Rust 侧的值)
        let pairs: &[(&str, &str)] = &[
            ("PROVIDER_SWITCHED", super::PROVIDER_SWITCHED),
            ("PROFILE_APPLIED", super::PROFILE_APPLIED),
            ("USAGE_CACHE_UPDATED", super::USAGE_CACHE_UPDATED),
            ("USAGE_LOG_RECORDED", super::USAGE_LOG_RECORDED),
            ("DEEPLINK_IMPORT", super::DEEPLINK_IMPORT),
            (
                "UNIVERSAL_PROVIDER_SYNCED",
                super::UNIVERSAL_PROVIDER_SYNCED,
            ),
            (
                "WEBDAV_SYNC_STATUS_UPDATED",
                super::WEBDAV_SYNC_STATUS_UPDATED,
            ),
            ("S3_SYNC_STATUS_UPDATED", super::S3_SYNC_STATUS_UPDATED),
            ("PURCHASE_CLOSED", super::PURCHASE_CLOSED),
            ("VENDOR_LOGIN_ERROR", super::VENDOR_LOGIN_ERROR),
        ];

        for (ts_name, rust_value) in pairs {
            let expected = format!("{ts_name} = \"{rust_value}\"");
            assert!(
                ts.contains(&expected),
                "src/lib/api/events.ts 的 {ts_name} 与 Rust 侧不一致\n  \
                 Rust 侧的值: {rust_value}\n  \
                 期望 TS 里出现: {expected}"
            );
        }
    }
}
