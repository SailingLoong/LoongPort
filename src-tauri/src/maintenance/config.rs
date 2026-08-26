use std::time::Duration;

pub const UPDATE_CHECK_STARTUP_DELAY: Duration = Duration::from_secs(5);
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const APP_UPDATE_CHECKED_EVENT: &str = "app-update-checked";
pub const MODELS_DEV_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const MODELS_DEV_PRICING_UPDATED_EVENT: &str = "models-dev-pricing-updated";
pub const RELAY_PRICING_STARTUP_DELAY: Duration = Duration::ZERO;
pub const RELAY_PRICING_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const RELAY_PRICING_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);
pub const VERIDROP_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_STARTUP_DELAY: Duration = Duration::from_secs(5);
pub const VERIDROP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);
// 站点实测共建的 flush 节奏：端到端「准实时」预算 10–20 分钟里，
// 客户端这段（桶闭合 + flush）占 ≤15 分钟（见 crowd-metrics/README 的时延账）。
pub const CROWD_METRICS_STARTUP_DELAY: Duration = Duration::from_secs(60);
pub const CROWD_METRICS_FLUSH_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const CROWD_METRICS_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);
