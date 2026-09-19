//! 订阅限额的「重置窗口」：一个档位在哪些时间窗内各有多少额度、什么时候重置。
//!
//! ## 数据从哪来（零新增请求）
//!
//! 分组限额在 `GET /groups/available` 的响应里（`daily/weekly/monthly_limit_usd`），
//! 用量与窗口起点在 `GET /keys` 的响应里（`usage_5h/1d/7d` + `window_*_start`）——
//! 两者 provision 本来就要拉，这里只是把字段接出来。
//!
//! ## 限额的裁决序：key 覆盖分组
//!
//! key 的 `rate_limit_*` 大于 0 时覆盖分组限额（服务端语义：key 级限额优先），
//! 为 0 表示沿用分组。5 小时窗只有 key 级限额；月限额只有分组级（key 侧没有
//! 月窗用量跟踪）。
//!
//! ## 重置时刻怎么算（不猜服务端的锚点策略）
//!
//! 服务端给的是「当前窗口的起点」：`reset_at = window_start + 窗口时长`。
//! 窗口还没开始过（起点 null、用量 0）就没有可算的重置——如实返回 `None`，
//! 不按「自然日边界」之类的假设编一个。月窗没有窗口跟踪，重置时刻同样
//! `None`（限额照常显示）。
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

use super::sub2api::{ApiKey, Group};

/// 一条时间窗（serde camelCase：进 settings JSON 与前端 DTO 用同一形状）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionWindow {
    pub kind: WindowKind,
    /// 窗口限额（美元）。
    pub limit_usd: f64,
    /// 已用（美元）。`None` = 该窗口没有用量跟踪（月窗）。
    pub used_usd: Option<f64>,
    /// 重置时刻（epoch 秒）。`None` = 窗口未开始 / 无窗口跟踪，算不出来。
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

const FIVE_HOURS_SECS: i64 = 5 * 3600;
const DAY_SECS: i64 = 24 * 3600;
const WEEK_SECS: i64 = 7 * DAY_SECS;

/// 分组 × 那把 key 的用量 → 该档位的窗口列表（无限额的窗口不出现）。
pub fn windows_for(group: &Group, key: &ApiKey) -> Vec<SubscriptionWindow> {
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
    push(
        &mut windows,
        WindowKind::FiveHour,
        key.rate_limit_5h,
        Some(key.usage_5h),
        key.window_5h_start.map(|s| (s as i64) + FIVE_HOURS_SECS),
    );
    push(
        &mut windows,
        WindowKind::Daily,
        if key.rate_limit_1d > 0.0 {
            key.rate_limit_1d
        } else {
            group.daily_limit_usd.unwrap_or(0.0)
        },
        Some(key.usage_1d),
        key.window_1d_start.map(|s| (s as i64) + DAY_SECS),
    );
    push(
        &mut windows,
        WindowKind::Weekly,
        if key.rate_limit_7d > 0.0 {
            key.rate_limit_7d
        } else {
            group.weekly_limit_usd.unwrap_or(0.0)
        },
        Some(key.usage_7d),
        key.window_7d_start.map(|s| (s as i64) + WEEK_SECS),
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

    /// 真实订阅分组的形状：分组给 日/周/月 限额、key 无覆盖、窗口已开。
    #[test]
    fn subscription_group_yields_daily_weekly_monthly() {
        let g = group(Some(500.0), Some(2000.0), Some(5000.0));
        let mut k = key();
        k.usage_1d = 31.0;
        k.window_1d_start = Some(1_789_000_000.0);
        let windows = windows_for(&g, &k);
        assert_eq!(
            windows.iter().map(|w| w.kind).collect::<Vec<_>>(),
            vec![WindowKind::Daily, WindowKind::Weekly, WindowKind::Monthly]
        );
        let daily = &windows[0];
        assert_eq!(daily.limit_usd, 500.0);
        assert_eq!(daily.used_usd, Some(31.0));
        assert_eq!(daily.reset_at, Some(1_789_000_000 + DAY_SECS));
        // 周窗没开始 → 重置算不出；月窗永远没有用量跟踪。
        assert_eq!(windows[1].reset_at, None);
        assert_eq!(windows[2].used_usd, None);
        // 下次重置取最早：只有日窗可算。
        assert_eq!(next_reset_at(&windows), Some(1_789_000_000 + DAY_SECS));
    }

    /// key 级限额覆盖分组；5 小时窗只有 key 级。
    #[test]
    fn key_limits_override_group() {
        let g = group(Some(100.0), None, None);
        let mut k = key();
        k.rate_limit_5h = 5.0;
        k.rate_limit_1d = 20.0;
        k.usage_5h = 3.0;
        k.window_5h_start = Some(1_789_000_000.0);
        let windows = windows_for(&g, &k);
        let five = &windows[0];
        assert_eq!(five.limit_usd, 5.0);
        assert_eq!(five.reset_at, Some(1_789_000_000 + FIVE_HOURS_SECS));
        let daily = &windows[1];
        assert_eq!(daily.limit_usd, 20.0, "key 覆盖分组的 100");
        // 最早的 = 5 小时窗。
        assert_eq!(
            next_reset_at(&windows),
            Some(1_789_000_000 + FIVE_HOURS_SECS)
        );
    }

    /// 非订阅分组（限额全空/0）→ 无窗口、无下次重置。
    #[test]
    fn plain_group_yields_no_windows() {
        let g = group(None, None, Some(0.0));
        assert!(windows_for(&g, &key()).is_empty());
        assert_eq!(next_reset_at(&[]), None);
    }
}
