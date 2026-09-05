//! 熔断器模块
//!
//! 实现熔断器模式，用于防止向不健康的供应商发送请求

use super::log_codes::cb as log_cb;
use super::types::AppProxyConfig;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// 熔断器状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// 关闭状态 - 正常工作
    Closed,
    /// 打开状态 - 熔断激活，拒绝请求
    Open,
    /// 半开状态 - 尝试恢复，允许部分请求通过
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// 熔断器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerConfig {
    /// 失败阈值 - 连续失败多少次后打开熔断器
    pub failure_threshold: u32,
    /// 成功阈值 - 半开状态下成功多少次后关闭熔断器
    pub success_threshold: u32,
    /// 超时时间 - 熔断器打开后多久尝试半开（秒）
    pub timeout_seconds: u64,
    /// 错误率阈值 - 错误率超过此值时打开熔断器 (0.0-1.0)
    pub error_rate_threshold: f64,
    /// 最小请求数 - 计算错误率前的最小请求数
    pub min_requests: u32,
}

impl From<&AppProxyConfig> for CircuitBreakerConfig {
    fn from(config: &AppProxyConfig) -> Self {
        Self {
            failure_threshold: config.circuit_failure_threshold,
            success_threshold: config.circuit_success_threshold,
            timeout_seconds: config.circuit_timeout_seconds as u64,
            error_rate_threshold: config.circuit_error_rate_threshold,
            min_requests: config.circuit_min_requests,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 4,
            success_threshold: 2,
            timeout_seconds: 60,
            error_rate_threshold: 0.6,
            min_requests: 10,
        }
    }
}

/// 致命失败（凭证/余额级，如上游 401/402/403）打开熔断后的冷却时长。
///
/// 这类错误短期不会自愈 —— 凭证不会被刷新、余额不会自己回来 —— 按普通
/// `timeout_seconds`（默认 60s）每分钟探测一次只是拿用户的请求反复撞墙。
/// 不做成配置项：致命分级本身就是策略，暴露成旋钮只会让人把它拧回 60s。
const FATAL_OPEN_COOLDOWN_SECONDS: u64 = 1800;

/// 熔断器实例
pub struct CircuitBreaker {
    /// 当前状态
    state: Arc<RwLock<CircuitState>>,
    /// 连续失败计数
    consecutive_failures: Arc<AtomicU32>,
    /// 连续成功计数（半开状态）
    consecutive_successes: Arc<AtomicU32>,
    /// 总请求计数
    total_requests: Arc<AtomicU32>,
    /// 失败请求计数
    failed_requests: Arc<AtomicU32>,
    /// 上次打开时间
    last_opened_at: Arc<RwLock<Option<Instant>>>,
    /// 本次 Open 是否由致命失败触发（决定恢复冷却用长冷却还是配置超时）
    fatal_open: Arc<std::sync::atomic::AtomicBool>,
    /// 配置（支持热更新）
    config: Arc<RwLock<CircuitBreakerConfig>>,
    /// 半开状态已放行的请求数（用于限流）
    half_open_requests: Arc<AtomicU32>,
}

/// 熔断器对外的只读快照（看板「熔断/自动重试倒计时」用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerSnapshot {
    /// `true` = HalfOpen（放行探测中）；`false` = Open。
    pub half_open: bool,
    /// Open 状态下距自动转 HalfOpen 的剩余秒数；HalfOpen 无倒计时。
    pub reopen_in_secs: Option<u64>,
}

/// 熔断器放行结果
///
/// `used_half_open_permit` 表示本次放行是否占用了 HalfOpen 探测名额。
/// 调用方应在请求结束后把该值传回 `record_success` / `record_failure` 用于正确释放名额。
#[derive(Debug, Clone, Copy)]
pub struct AllowResult {
    pub allowed: bool,
    pub used_half_open_permit: bool,
}

impl CircuitBreaker {
    /// 创建新的熔断器
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            consecutive_successes: Arc::new(AtomicU32::new(0)),
            total_requests: Arc::new(AtomicU32::new(0)),
            failed_requests: Arc::new(AtomicU32::new(0)),
            last_opened_at: Arc::new(RwLock::new(None)),
            fatal_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            config: Arc::new(RwLock::new(config)),
            half_open_requests: Arc::new(AtomicU32::new(0)),
        }
    }

    /// 更新熔断器配置（热更新，不重置状态）
    pub async fn update_config(&self, new_config: CircuitBreakerConfig) {
        *self.config.write().await = new_config;
    }

    /// 当前 Open 状态的恢复冷却（秒）：致命失败用长冷却，否则用配置超时
    async fn open_cooldown_secs(&self, config: &CircuitBreakerConfig) -> u64 {
        if self.fatal_open.load(Ordering::SeqCst) {
            FATAL_OPEN_COOLDOWN_SECONDS
        } else {
            config.timeout_seconds
        }
    }

    /// 判断当前 Provider 是否“可被纳入候选链路”
    ///
    /// 这个方法不会占用 HalfOpen 探测名额，仅用于路由选择阶段的“可用性判断”：
    /// - Closed / HalfOpen：可用（返回 true）
    /// - Open：若超时到达则切到 HalfOpen 并返回 true，否则返回 false
    ///
    /// 注意：真正发起请求前仍需调用 `allow_request()` 来获取 HalfOpen 探测名额，
    /// 并在请求结束后通过 `record_success()` / `record_failure()` 释放。
    pub async fn is_available(&self) -> bool {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        match state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(opened_at) = *self.last_opened_at.read().await {
                    if opened_at.elapsed().as_secs() >= self.open_cooldown_secs(&config).await {
                        drop(config); // 释放读锁再转换状态
                        log::info!(
                            "[{}] 熔断器 Open → HalfOpen (超时恢复)",
                            log_cb::OPEN_TO_HALF_OPEN
                        );
                        self.transition_to_half_open().await;
                        return true;
                    }
                }
                false
            }
        }
    }

    /// 检查是否允许请求通过
    pub async fn allow_request(&self) -> AllowResult {
        let state = *self.state.read().await;

        match state {
            CircuitState::Closed => AllowResult {
                allowed: true,
                used_half_open_permit: false,
            },
            CircuitState::Open => {
                let config = self.config.read().await;
                // 检查是否应该尝试半开
                if let Some(opened_at) = *self.last_opened_at.read().await {
                    if opened_at.elapsed().as_secs() >= self.open_cooldown_secs(&config).await {
                        drop(config); // 释放读锁再转换状态
                        log::info!(
                            "[{}] 熔断器 Open → HalfOpen (超时恢复)",
                            log_cb::OPEN_TO_HALF_OPEN
                        );
                        self.transition_to_half_open().await;

                        // 转换后按当前状态决定是否需要获取 HalfOpen 探测名额
                        let current_state = *self.state.read().await;
                        return match current_state {
                            CircuitState::Closed => AllowResult {
                                allowed: true,
                                used_half_open_permit: false,
                            },
                            CircuitState::HalfOpen => self.allow_half_open_probe(),
                            CircuitState::Open => AllowResult {
                                allowed: false,
                                used_half_open_permit: false,
                            },
                        };
                    }
                }

                AllowResult {
                    allowed: false,
                    used_half_open_permit: false,
                }
            }
            CircuitState::HalfOpen => self.allow_half_open_probe(),
        }
    }

    /// 记录成功
    pub async fn record_success(&self, used_half_open_permit: bool) {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        if used_half_open_permit {
            self.release_half_open_permit();
        }

        // 重置失败计数
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.total_requests.fetch_add(1, Ordering::SeqCst);

        if state == CircuitState::HalfOpen {
            let successes = self.consecutive_successes.fetch_add(1, Ordering::SeqCst) + 1;

            if successes >= config.success_threshold {
                drop(config); // 释放读锁再转换状态
                log::info!(
                    "[{}] 熔断器 HalfOpen → Closed (恢复正常)",
                    log_cb::HALF_OPEN_TO_CLOSED
                );
                self.transition_to_closed().await;
            }
        }
    }

    /// 记录失败
    pub async fn record_failure(&self, used_half_open_permit: bool) {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        if used_half_open_permit {
            self.release_half_open_permit();
        }

        // 更新计数器
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.failed_requests.fetch_add(1, Ordering::SeqCst);

        // 重置成功计数
        self.consecutive_successes.store(0, Ordering::SeqCst);

        // 检查是否应该打开熔断器
        match state {
            CircuitState::HalfOpen => {
                // HalfOpen 状态下失败，立即转为 Open
                log::warn!(
                    "[{}] 熔断器 HalfOpen 探测失败 → Open",
                    log_cb::HALF_OPEN_PROBE_FAILED
                );
                drop(config);
                // 沿用上一次打开时的致命标记：曾因欠费打开的档位探测再失败，
                // 说明还是坏的，不该把冷却悄悄降回配置超时。
                let fatal = self.fatal_open.load(Ordering::SeqCst);
                self.transition_to_open(fatal).await;
            }
            CircuitState::Closed => {
                // 检查连续失败次数
                if failures >= config.failure_threshold {
                    log::warn!(
                        "[{}] 熔断器触发: 连续失败 {failures} 次 → Open",
                        log_cb::TRIGGERED_FAILURES
                    );
                    drop(config); // 释放读锁再转换状态
                    let fatal = self.fatal_open.load(Ordering::SeqCst);
                    self.transition_to_open(fatal).await;
                } else {
                    // 检查错误率
                    let total = self.total_requests.load(Ordering::SeqCst);
                    let failed = self.failed_requests.load(Ordering::SeqCst);

                    if total >= config.min_requests {
                        let error_rate = failed as f64 / total as f64;

                        if error_rate >= config.error_rate_threshold {
                            log::warn!(
                                "[{}] 熔断器触发: 错误率 {:.1}% → Open",
                                log_cb::TRIGGERED_ERROR_RATE,
                                error_rate * 100.0
                            );
                            drop(config); // 释放读锁再转换状态
                            let fatal = self.fatal_open.load(Ordering::SeqCst);
                            self.transition_to_open(fatal).await;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// 记录致命失败（凭证/余额级错误，如上游 401/402/403）
    ///
    /// 与 [`record_failure`](Self::record_failure) 的区别只有两点：
    /// - **一次即开**：不等连续失败阈值 —— 第 2、3 次请求结果已可预知，没必要再撞；
    /// - **长冷却**：恢复探测间隔用 [`FATAL_OPEN_COOLDOWN_SECONDS`] 而不是配置的
    ///   `timeout_seconds`。
    ///
    /// 致命分级只在成功（`transition_to_closed`）时清除：半开探测失败会沿用上一次
    /// 打开时的致命冷却 —— 一个曾因欠费打开的档位，探测又失败，说明它还是坏的。
    pub async fn record_fatal_failure(&self, used_half_open_permit: bool) {
        let config = self.config.read().await;

        if used_half_open_permit {
            self.release_half_open_permit();
        }

        self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.failed_requests.fetch_add(1, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);

        let cooldown = config.timeout_seconds.max(FATAL_OPEN_COOLDOWN_SECONDS);
        drop(config);
        log::warn!(
            "[{}] 熔断器致命失败（凭证/余额级）→ Open，冷却 {cooldown}s",
            log_cb::TRIGGERED_FATAL
        );
        self.transition_to_open(true).await;
    }

    /// 只读快照：`None` = Closed（正常，不上板）。
    pub async fn snapshot(&self) -> Option<BreakerSnapshot> {
        let state = *self.state.read().await;
        match state {
            CircuitState::Closed => None,
            CircuitState::HalfOpen => Some(BreakerSnapshot {
                half_open: true,
                reopen_in_secs: None,
            }),
            CircuitState::Open => {
                let cooldown = {
                    let config = self.config.read().await;
                    self.open_cooldown_secs(&config).await
                };
                let elapsed = self
                    .last_opened_at
                    .read()
                    .await
                    .map(|opened_at| opened_at.elapsed().as_secs())
                    .unwrap_or(cooldown);
                Some(BreakerSnapshot {
                    half_open: false,
                    reopen_in_secs: Some(cooldown.saturating_sub(elapsed)),
                })
            }
        }
    }

    /// 获取当前状态
    #[allow(dead_code)]
    pub async fn get_state(&self) -> CircuitState {
        *self.state.read().await
    }

    /// 获取统计信息
    #[allow(dead_code)]
    pub async fn get_stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            state: *self.state.read().await,
            consecutive_failures: self.consecutive_failures.load(Ordering::SeqCst),
            consecutive_successes: self.consecutive_successes.load(Ordering::SeqCst),
            total_requests: self.total_requests.load(Ordering::SeqCst),
            failed_requests: self.failed_requests.load(Ordering::SeqCst),
        }
    }

    /// 重置熔断器（手动恢复）
    #[allow(dead_code)]
    pub async fn reset(&self) {
        log::info!("[{}] 熔断器手动重置 → Closed", log_cb::MANUAL_RESET);
        self.transition_to_closed().await;
    }

    fn allow_half_open_probe(&self) -> AllowResult {
        // 半开状态限流：只允许有限请求通过进行探测
        let max_half_open_requests = 1u32;
        let current = self.half_open_requests.fetch_add(1, Ordering::SeqCst);

        if current < max_half_open_requests {
            AllowResult {
                allowed: true,
                used_half_open_permit: true,
            }
        } else {
            // 超过限额，回退计数，拒绝请求
            self.half_open_requests.fetch_sub(1, Ordering::SeqCst);
            AllowResult {
                allowed: false,
                used_half_open_permit: false,
            }
        }
    }

    /// 仅释放 HalfOpen permit，不影响健康统计
    ///
    /// 用于整流器等场景：请求结果不应计入 Provider 健康度，
    /// 但仍需释放占用的探测名额，避免 HalfOpen 状态卡死
    pub fn release_half_open_permit(&self) {
        let mut current = self.half_open_requests.load(Ordering::SeqCst);
        loop {
            if current == 0 {
                return;
            }

            match self.half_open_requests.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// 转换到打开状态
    ///
    /// `fatal` 决定恢复冷却：普通失败沿用配置超时，致命失败用长冷却。
    /// 普通失败路径传「沿用当前标记」—— 半开探测失败不该把致命冷却悄悄降回 60s。
    async fn transition_to_open(&self, fatal: bool) {
        *self.state.write().await = CircuitState::Open;
        *self.last_opened_at.write().await = Some(Instant::now());
        self.fatal_open.store(fatal, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
    }

    /// 转换到半开状态
    async fn transition_to_half_open(&self) {
        let mut state = self.state.write().await;
        if *state != CircuitState::Open {
            return;
        }

        *state = CircuitState::HalfOpen;
        self.consecutive_successes.store(0, Ordering::SeqCst);
        // 重置半开状态的请求限流计数
        self.half_open_requests.store(0, Ordering::SeqCst);
    }

    /// 转换到关闭状态
    async fn transition_to_closed(&self) {
        *self.state.write().await = CircuitState::Closed;
        self.fatal_open.store(false, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        // 重置计数器
        self.total_requests.store(0, Ordering::SeqCst);
        self.failed_requests.store(0, Ordering::SeqCst);
    }
}

/// 熔断器统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub total_requests: u32,
    pub failed_requests: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_reports_open_with_countdown_and_closed_as_none() {
        // 正常失败打开（阈值 1、冷却 600s）：Open + 剩余 ≈ 600
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 600,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);
        assert!(
            breaker.snapshot().await.is_none(),
            "Closed → None（正常态不上板）"
        );

        breaker.record_failure(false).await;
        let snap = breaker.snapshot().await.expect("Open 必须有快照");
        assert!(!snap.half_open);
        let remaining = snap.reopen_in_secs.expect("Open 必须带倒计时");
        assert!(
            remaining > 590 && remaining <= 600,
            "剩余 {remaining} 应接近 600"
        );

        // 致命失败：长冷却 1800
        let fatal_breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 600,
            ..Default::default()
        });
        fatal_breaker.record_fatal_failure(false).await;
        let snap = fatal_breaker.snapshot().await.expect("致命 Open 有快照");
        let remaining = snap.reopen_in_secs.expect("致命 Open 带长冷却倒计时");
        assert!(
            remaining > 1790 && remaining <= 1800,
            "剩余 {remaining} 应接近 1800"
        );

        // 冷却 0：Open 超时 → is_available 转 HalfOpen → 快照 half_open 无倒计时
        let instant = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        });
        instant.record_failure(false).await;
        assert!(instant.is_available().await, "冷却 0 应立即转 HalfOpen");
        let snap = instant.snapshot().await.expect("HalfOpen 有快照");
        assert!(snap.half_open);
        assert!(snap.reopen_in_secs.is_none(), "HalfOpen 无倒计时");
    }

    #[tokio::test]
    async fn test_circuit_breaker_closed_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 初始状态应该是关闭
        assert_eq!(breaker.get_state().await, CircuitState::Closed);
        assert!(breaker.allow_request().await.allowed);

        // 记录 3 次失败
        for _ in 0..3 {
            breaker.record_failure(false).await;
        }

        // 应该转换到打开状态
        assert_eq!(breaker.get_state().await, CircuitState::Open);
        assert!(!breaker.allow_request().await.allowed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_to_closed() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 打开熔断器
        breaker.record_failure(false).await;
        breaker.record_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);

        // 手动转换到半开状态
        breaker.transition_to_half_open().await;
        assert_eq!(breaker.get_state().await, CircuitState::HalfOpen);

        // 记录 2 次成功
        breaker.record_success(false).await;
        breaker.record_success(false).await;

        // 应该转换到关闭状态
        assert_eq!(breaker.get_state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_half_open_transition_does_not_reset_inflight_permit() {
        let config = CircuitBreakerConfig {
            timeout_seconds: 0,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 进入 Open，然后由于 timeout_seconds=0，allow_request 会立即切换到 HalfOpen 并占用探测名额
        breaker.transition_to_open(false).await;
        let first = breaker.allow_request().await;
        assert!(first.allowed);
        assert!(first.used_half_open_permit);
        assert_eq!(breaker.get_state().await, CircuitState::HalfOpen);

        // 模拟并发下的“重复 HalfOpen 转换调用”，不应重置 in-flight 计数
        breaker.transition_to_half_open().await;

        // 由于名额仍被占用，第二次请求应被拒绝
        let second = breaker.allow_request().await;
        assert!(!second.allowed);
        assert!(!second.used_half_open_permit);
    }

    #[tokio::test]
    async fn test_circuit_breaker_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 打开熔断器
        breaker.record_failure(false).await;
        breaker.record_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);

        // 重置
        breaker.reset().await;
        assert_eq!(breaker.get_state().await, CircuitState::Closed);
        assert!(breaker.allow_request().await.allowed);
    }

    /// 致命失败一次即 Open，且冷却用长冷却而不是配置超时。
    #[tokio::test]
    async fn fatal_failure_opens_immediately_with_long_cooldown() {
        let config = CircuitBreakerConfig {
            failure_threshold: 4,
            timeout_seconds: 60,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 普通失败 4 次才开；致命失败 1 次就开
        breaker.record_fatal_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);
        assert!(!breaker.is_available().await);

        {
            let config = breaker.config.read().await;
            assert_eq!(
                breaker.open_cooldown_secs(&config).await,
                FATAL_OPEN_COOLDOWN_SECONDS
            );
        }
    }

    /// 成功（Closed 恢复）清除致命标记；之后普通失败打开用配置超时。
    #[tokio::test]
    async fn success_clears_fatal_cooldown() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            timeout_seconds: 60,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        breaker.record_fatal_failure(false).await;
        // 手动进入半开并让一次探测成功 → Closed
        breaker.transition_to_half_open().await;
        breaker.record_success(true).await;
        assert_eq!(breaker.get_state().await, CircuitState::Closed);

        // 再普通失败打开：冷却应回到配置超时（致命标记已被成功清除）
        breaker.record_failure(false).await;
        breaker.record_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);
        {
            let config = breaker.config.read().await;
            assert_eq!(breaker.open_cooldown_secs(&config).await, 60);
        }
    }
}
