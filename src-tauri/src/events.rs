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
//! `relay-login-error`）**不要进** —— 单侧事实没有分叉风险，收进来只是噪音
//! （尺子 2：不为不存在的跨语言契约预建）。判断标准：跨语言 = Rust emit + 前端 listen
//! 两侧都有。

use serde::Serialize;
use tauri::Emitter;

use crate::app_config::AppType;
use crate::relay::model_verification::passive::AnomalyFingerprint;

/// 当前供应商切换后通知前端（`providersApi.onSwitched` / `RelaySection` 监听）。
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
/// 充值窗关闭（`RelaySection` 监听）。原名 `commands::relay::PURCHASE_CLOSED_EVENT`。
pub const PURCHASE_CLOSED: &str = "relay-purchase-closed";
/// 官网直连登录窗凭据解析失败（`RelaySection` 监听）。原名
/// `commands::vendor::LOGIN_ERROR_EVENT`。
pub const VENDOR_LOGIN_ERROR: &str = "vendor-login-error";
/// 主动模型验证任务进度变化（模型验证弹窗监听）。
pub const MODEL_VERIFICATION_PROGRESS: &str = "model-verification-progress";
/// 模型验证持久化结果变化（档位行与模型验证弹窗监听）。
pub const MODEL_VERIFICATION_CHANGED: &str = "model-verification-changed";
pub const MODEL_VERIFICATION_ANOMALY: &str = "model-verification-anomaly";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelVerificationAnomalyEvent {
    pub provider_id: String,
    pub app_type: String,
    pub model: String,
    pub fingerprint: AnomalyFingerprint,
}

/// 广播「当前供应商变了」。
///
/// ## 为什么必须发（2026-08-04 修的 bug）
///
/// 上游只在**托盘快切 / 故障转移 / 应用项目**三处发 `provider-switched`，
/// 而 `switch_provider`（provider 页那个「启用」按钮）**不发** —— 上游没事，
/// 因为它的 provider 页自己就是 mutation 的调用方、`onSuccess` 里 invalidate 掉缓存就够了。
///
/// **但 LoongPort 多了一个消费者**：`RelaySection`（供应商页顶部那一区）用自己的
/// `useState` + 手工 `reload()`（不走 react-query），所以那次 invalidate 与它无关。症状是：
///
/// 1. 用户在中转站区看到某个托管档位是「当前使用中」；
/// 2. 他在同一页下方启用了一个 cc-switch 自建的 sk；
/// 3. 中转站区那边 —— **那个档位仍显示「使用中」，中转站行的删除按钮仍是灰的**，
///    `title` 还写着「要先切走」。他明明已经切走了，却删不掉这一行。
///
/// 而后端其实是对的：`ProviderService::switch` 已经更新了 current，
/// 重开窗口就正常了。坏的只有「不重开就看不到」这一段 —— 属**静默的界面陈旧**，
/// 不报错、不崩，用户会以为是删除功能坏了。
///
/// ## 为什么补在这里，而不是让前端去 refresh
///
/// 让 `useSwitchProviderMutation` 的 `onSuccess` 直接调 relay 的刷新，等于在
/// react-query 与非 react-query 两套状态之间私接一根线，且每多一个消费者就要再接一根。
/// 发事件是**上游已有的机制**（三个发射点 + `providersApi.onSwitched` 封装都在），
/// 缺的只是这一个发射点 —— 补它同时让将来任何监听者都能收到。
///
/// payload 形状照前端 `ProviderSwitchEvent` 的契约（`appType` + `providerId`）。
/// 上游那三处各带了些额外字段（`proxyEnabled` / `source`…），前端一个都没用，
/// 这里不跟着带 —— 多带的字段是没人消费的噪音。
///
/// ⚠️ 放在 events.rs（而不是某个命令模块）是因为它是**跨模块**的：`commands::provider`
/// 是私有模块，deeplink（crate 级模块）访问不到它。凡「改了 current 就要通知界面」的
/// 路径都从这里发，别在别处复制第二遍（CLAUDE.md §1.4）。
pub fn emit_provider_switched(
    app_handle: &tauri::AppHandle,
    app_type: &AppType,
    provider_id: &str,
) {
    let payload = serde_json::json!({
        "appType": app_type.as_str(),
        "providerId": provider_id,
    });
    if let Err(e) = app_handle.emit(PROVIDER_SWITCHED, payload) {
        // 发不出去只是界面不刷新（用户重开面板就好），不该让切换本身失败 ——
        // 配置已经写进去了，报错会让用户以为没切成功而再切一次。
        log::warn!("发射 {PROVIDER_SWITCHED} 事件失败: {e}");
    }
}

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
            (
                "MODEL_VERIFICATION_PROGRESS",
                super::MODEL_VERIFICATION_PROGRESS,
            ),
            (
                "MODEL_VERIFICATION_CHANGED",
                super::MODEL_VERIFICATION_CHANGED,
            ),
            (
                "MODEL_VERIFICATION_ANOMALY",
                super::MODEL_VERIFICATION_ANOMALY,
            ),
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
