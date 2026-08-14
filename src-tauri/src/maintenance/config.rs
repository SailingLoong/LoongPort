use std::time::Duration;

pub const VERIDROP_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_STARTUP_DELAY: Duration = Duration::from_secs(5);
pub const VERIDROP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const VERIDROP_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);
