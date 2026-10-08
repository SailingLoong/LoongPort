//! Time belongs to the engine context, never to the renderer.
pub(crate) trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
#[cfg(test)]
pub(crate) struct FixedClock(pub i64);
#[cfg(test)]
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
