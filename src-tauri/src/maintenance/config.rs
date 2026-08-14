use std::time::Duration;

pub const UPDATE_CHECK_STARTUP_DELAY: Duration = Duration::from_secs(5);
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const APP_UPDATE_CHECKED_EVENT: &str = "app-update-checked";
pub const MODELS_DEV_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const MODELS_DEV_PRICING_UPDATED_EVENT: &str = "models-dev-pricing-updated";
pub const VERIDROP_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_STARTUP_DELAY: Duration = Duration::from_secs(5);
pub const VERIDROP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);
