//! 订阅限额的「重置窗口」：一个档位在哪些时间窗内各有多少额度、什么时候重置。
//!
//! ## 两个数据源，谁是真源（2026-09-20 真站实测定调）
//!
//! - **订阅型分组**：用量与窗口起点在 `GET /api/v1/subscriptions`（服务端
//!   `user_subscriptions` 表，按 用户×分组 跟踪日/周/月三窗）。api_keys 上的
//!   `usage_*` / `window_*_start` **不是**这一路的数据——它们只服务 key 级限额
//!   （没有人给 key 设独立限额的站点上恒为零）。
//! - **key 级限额**（`rate_limit_* > 0`）：用 api_keys 自己的窗口字段。
//!
//! 两者都来自 provision 本来就要拉的响应之外**多一个** `/subscriptions` 请求，
//! 与专属倍率（`/groups/rates`）同款的一次 provision 一个请求的量级。
//!
//! ## 重置时刻怎么算（不猜服务端的锚点策略）
//!
//! 服务端给的是「当前窗口的起点」：`reset_at = window_start + 窗口时长`。
//! 窗口还没开始过（起点 null）就没有可算的重置——如实返回 `None`，
//! 不按「自然日边界」之类的假设编一个。月窗的时长是**一个日历月**
//! （`checked_add_months`），起点随订阅锚定漂移。
//!
//! ## 横向扩展位：别的站点后端
//!
//! 这是后端无关的投影：`newapi` 等没有时间窗语义的后端不产窗口（返回空），
//! 将来某个后端有了，加一个「从它的数据构造 [`SubscriptionWindow`]」的实现即可，
//! 下游（落库、DTO、排序）不关心来源。
//!
//! 排序用的「下次重置」= 所有窗口里**最早**的 `reset_at`（用户要优先消耗的正是
//! 最先作废的那笔额度）；没有窗口的档位是 `None`，排序时排最后（稳定）。

use serde::{Deserialize, Serialize};

use super::sub2api::{ApiKey, Group, UserSubscription};

/// 一条时间窗（serde camelCase：进 settings JSON 与前端 DTO 用同一形状）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionWindow {
    pub kind: WindowKind,
    /// 窗口限额（美元）。
    pub limit_usd: f64,
    /// 已用（美元）。`None` = 该窗口没有用量跟踪。
    pub used_usd: Option<f64>,
    /// 重置时刻（epoch 秒）。`None` = 窗口未开始，算不出来。
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowKind {
    FiveHour,
    Daily,
    Weekly,
    Monthly,
}

/// 分组 × key × 用户订阅 → 该档位的窗口列表（无限额的窗口不出现）。
///
/// 裁决序：**订阅行在就用订阅行**（订阅型分组的真源）；否则落回 key 自己的
/// 窗口字段（只有设了 key 级限额的站点会有值）。5 小时窗只有 key 级一种，
/// 两种情况下都看 key 限额。
pub fn windows_for(
    group: &Group,
    key: &ApiKey,
    subscription: Option<&UserSubscription>,
) -> Vec<SubscriptionWindow> {
    let mut windows = Vec::new();
    let push = |windows: &mut Vec<SubscriptionWindow>,
                kind: WindowKind,
                limit: f64,
                used: Option<f64>,
                reset_at: Option<i64>| {
        // 0 / 负限额 = 该窗口没有约束，不产窗口（宁可少说不少说）。
        if limit > 0.0 {
            windows.push(SubscriptionWindow {
                kind,
                limit_usd: limit,
                used_usd: used,
                reset_at,
            });
        }
    };

    // 5 小时窗：key 级限额独有。
    push(
        &mut windows,
        WindowKind::FiveHour,
        key.rate_limit_5h,
        Some(key.usage_5h),
        key.window_5h_start.map(|s| (s as i64) + 5 * 3600),
    );

    let group_limit = |key_limit: f64, group_limit: Option<f64>| {
        if key_limit > 0.0 {
            key_limit
        } else {
            group_limit.unwrap_or(0.0)
        }
    };

    if let Some(sub) = subscription {
        // 订阅行：三窗的用量与起点都在这里。
        push(
            &mut windows,
            WindowKind::Daily,
            group_limit(key.rate_limit_1d, group.daily_limit_usd),
            Some(sub.daily_usage_usd),
            sub.daily_window_start
                .map(|start| (start + chrono::Duration::hours(24)).timestamp()),
        );
        push(
            &mut windows,
            WindowKind::Weekly,
            group_limit(key.rate_limit_7d, group.weekly_limit_usd),
            Some(sub.weekly_usage_usd),
            sub.weekly_window_start
                .map(|start| (start + chrono::Duration::hours(7 * 24)).timestamp()),
        );
        push(
            &mut windows,
            WindowKind::Monthly,
            group.monthly_limit_usd.unwrap_or(0.0),
            Some(sub.monthly_usage_usd),
            sub.monthly_window_start
                // 一个日历月，不是 30 天：订阅的月窗随订阅锚定（9-10 开的订阅，
                // 10-10 重置），30 天会在月末附近漂移。
                .and_then(|start| {
                    start
                        .checked_add_months(chrono::Months::new(1))
                        .map(|end| end.timestamp())
                }),
        );
        return windows;
    }

    // 没有订阅行：key 自己的窗口字段（只有 key 级限额的站点会跟踪它们）。
    push(
        &mut windows,
        WindowKind::Daily,
        group_limit(key.rate_limit_1d, group.daily_limit_usd),
        Some(key.usage_1d),
        key.window_1d_start.map(|s| (s as i64) + 24 * 3600),
    );
    push(
        &mut windows,
        WindowKind::Weekly,
        group_limit(key.rate_limit_7d, group.weekly_limit_usd),
        Some(key.usage_7d),
        key.window_7d_start.map(|s| (s as i64) + 7 * 24 * 3600),
    );
    push(
        &mut windows,
        WindowKind::Monthly,
        group.monthly_limit_usd.unwrap_or(0.0),
        None,
        None,
    );
    windows
}

/// 排序键：所有窗口里最早的重置时刻（最先作废的额度）。
pub fn next_reset_at(windows: &[SubscriptionWindow]) -> Option<i64> {
    windows.iter().filter_map(|window| window.reset_at).min()
}

/// Read stored subscription windows; malformed or missing projections have no windows.
pub(crate) fn subscription_windows_from_settings(
    settings: &serde_json::Value,
) -> Vec<SubscriptionWindow> {
    settings
        .get("subscriptionWindows")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(daily: Option<f64>, weekly: Option<f64>, monthly: Option<f64>) -> Group {
        Group {
            id: 28,
            name: "订阅分组".into(),
            platform: "composite".into(),
            rate_multiplier: 1.0,
            status: "active".into(),
            allow_image_generation: false,
            daily_limit_usd: daily,
            weekly_limit_usd: weekly,
            monthly_limit_usd: monthly,
        }
    }

    fn key() -> ApiKey {
        ApiKey::default()
    }

    fn subscription(
        daily_start: Option<&str>,
        weekly_start: Option<&str>,
        monthly_start: Option<&str>,
    ) -> UserSubscription {
        let parse = |value: Option<&str>| {
            value
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
        };
        UserSubscription {
            group_id: 28,
            status: "active".into(),
            daily_usage_usd: 0.0000264,
            weekly_usage_usd: 305.6383104,
            monthly_usage_usd: 335.6282007,
            daily_window_start: parse(daily_start),
            weekly_window_start: parse(weekly_start),
            monthly_window_start: parse(monthly_start),
        }
    }

    /// 真站实测形状（2026-09-20，coding-api 订阅分组）：订阅行三窗齐备，
    /// 月窗锚定订阅起点（+1 日历月），下次重置取最早（日窗）。
    #[test]
    fn subscription_row_drives_windows() {
        let g = group(Some(500.0), Some(2000.0), Some(5000.0));
        let sub = subscription(
            Some("2026-09-20T00:00:00+08:00"),
            Some("2026-09-17T16:59:20.771607+08:00"),
            Some("2026-09-10T16:59:20.771607+08:00"),
        );
        let windows = windows_for(&g, &key(), Some(&sub));
        assert_eq!(
            windows.iter().map(|w| w.kind).collect::<Vec<_>>(),
            vec![WindowKind::Daily, WindowKind::Weekly, WindowKind::Monthly]
        );
        let daily = &windows[0];
        assert_eq!(daily.limit_usd, 500.0);
        assert_eq!(daily.used_usd, Some(0.0000264));
        assert_eq!(
            daily.reset_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-09-21T00:00:00+08:00")
                    .unwrap()
                    .timestamp()
            )
        );
        // 月窗 = 起点 + 1 日历月（9-10 → 10-10），不是 30 天。
        assert_eq!(
            windows[2].reset_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-10T16:59:20.771607+08:00")
                    .unwrap()
                    .timestamp()
            )
        );
        // 下次重置 = 日窗（最早）。
        assert_eq!(next_reset_at(&windows), daily.reset_at);
    }

    /// 没有订阅行：落回 key 窗口字段；月窗无跟踪、used 如实为 None。
    #[test]
    fn key_windows_without_subscription_row() {
        let g = group(Some(100.0), None, Some(300.0));
        let mut k = key();
        k.rate_limit_5h = 5.0;
        k.window_5h_start = Some(1_789_000_000.0);
        k.usage_1d = 31.0;
        k.window_1d_start = Some(1_789_000_000.0);
        let windows = windows_for(&g, &k, None);
        assert_eq!(
            windows.iter().map(|w| w.kind).collect::<Vec<_>>(),
            vec![WindowKind::FiveHour, WindowKind::Daily, WindowKind::Monthly]
        );
        assert_eq!(windows[1].used_usd, Some(31.0));
        assert_eq!(windows[1].reset_at, Some(1_789_000_000 + 24 * 3600));
        assert_eq!(windows[2].used_usd, None, "无订阅行 ⇒ 月窗无用量跟踪");
        // 最早的 = 5 小时窗。
        assert_eq!(next_reset_at(&windows), Some(1_789_000_000 + 5 * 3600));
    }

    /// key 级限额覆盖分组；非订阅分组（限额全空/0）→ 无窗口。
    #[test]
    fn plain_group_yields_no_windows() {
        let g = group(None, None, Some(0.0));
        assert!(windows_for(&g, &key(), None).is_empty());
        assert_eq!(next_reset_at(&[]), None);
    }
}
