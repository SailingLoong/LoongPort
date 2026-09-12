//! 供应商路由器模块
//!
//! 负责选择和管理代理目标供应商，实现智能故障转移

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::circuit_breaker::{AllowResult, CircuitBreaker, CircuitBreakerConfig};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Codex Official requests carry the selected account's native Authorization
/// header. Reusing that request against another account card would cross the
/// account boundary, so these cards must never participate in provider retry.
pub(crate) fn provider_supports_failover(app_type: &str, provider: &Provider) -> bool {
    provider_supports_proxy_routing(app_type, provider)
        && (app_type != AppType::Codex.as_str()
            || !crate::proxy::providers::is_codex_official_provider(provider))
}

pub(crate) fn provider_supports_proxy_routing(app_type: &str, provider: &Provider) -> bool {
    let Ok(app) = AppType::from_str(app_type) else {
        return false;
    };
    app.supports_local_proxy()
        && (provider.category.as_deref() != Some("official")
            || crate::services::provider::official_provider_supports_proxy_takeover(&app, provider))
}

/// 账号级熔断键：与档位键（`app_type:provider_id`）同表不同命名空间，
/// `account:` 段保证不与任何 provider id 撞键。
///
/// app 维度与档位熔断一致（跨 app 不联动）：熔断器配置按 app 读取，
/// 且同站同账号在不同 app 的档位走不同链路，跨 app 连坐会放大误伤。
fn account_circuit_key(app_type: &str, provider: &Provider) -> Option<String> {
    provider
        .failover_account_key()
        .map(|key| format!("{app_type}:account:{key}"))
}

/// 供应商路由器
pub struct ProviderRouter {
    /// 数据库连接
    db: Arc<Database>,
    /// 熔断器管理器 - key 格式: "app_type:provider_id"
    circuit_breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
}

impl ProviderRouter {
    /// 创建新的供应商路由器
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn record_attempt(&self, app_type: &str, provider_id: &str, success: bool) {
        if let Err(error) = self
            .db
            .record_provider_attempt(app_type, provider_id, success)
        {
            log::warn!("Could not record provider attempt outcome: {error}");
        }
    }

    pub fn preferred_model(&self, app_type: &str, provider: &Provider) -> Option<String> {
        super::application_routing::model_for_provider(&self.db, app_type, provider)
    }

    /// Keep explicit selection first, then try only later persistent priorities.
    pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, AppError> {
        use super::application_routing;
        let ordered = application_routing::ordered_providers(&self.db, app_type)?;
        let current_id = application_routing::current_provider_id(&self.db, app_type);
        let current = ordered
            .iter()
            .find(|p| Some(p.id.as_str()) == current_id.as_deref());
        let enabled = self
            .db
            .get_proxy_config_for_app(app_type)
            .await?
            .auto_failover_enabled;
        let mut result = Vec::new();
        if let Some(current) = current.filter(|p| provider_supports_proxy_routing(app_type, p)) {
            result.push(current.clone());
            if !enabled || !provider_supports_failover(app_type, current) {
                return Ok(result);
            }
        } else if !enabled {
            return Err(AppError::NoProvidersConfigured);
        }
        let start = current
            .and_then(|p| ordered.iter().position(|entry| entry.id == p.id))
            .map_or(0, |index| index + 1);
        for provider in ordered.into_iter().skip(start) {
            if application_routing::fallback_exclusion(&self.db, app_type, &provider).is_none()
                && self.tier_and_account_available(app_type, &provider).await
            {
                result.push(provider);
            }
        }
        if result.is_empty() {
            return Err(AppError::NoProvidersConfigured);
        }
        Ok(result)
    }

    /// 档位与其所属账号的熔断器是否都可用（选路阶段判断，不占探测名额）。
    async fn tier_and_account_available(&self, app_type: &str, provider: &Provider) -> bool {
        let circuit_key = format!("{app_type}:{}", provider.id);
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        if !breaker.is_available().await {
            return false;
        }
        if let Some(account_key) = account_circuit_key(app_type, provider) {
            let account_breaker = self.get_or_create_circuit_breaker(&account_key).await;
            if !account_breaker.is_available().await {
                return false;
            }
        }
        true
    }

    /// 请求执行前获取熔断器“放行许可”
    ///
    /// - Closed：直接放行
    /// - Open：超时到达后切到 HalfOpen 并放行一次探测
    /// - HalfOpen：按限流规则放行探测
    ///
    /// 注意：调用方必须在请求结束后通过 `record_result()` 释放 HalfOpen 名额，
    /// 否则会导致该 Provider 长时间无法进入探测状态。
    pub async fn allow_provider_request(&self, provider_id: &str, app_type: &str) -> AllowResult {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.allow_request().await
    }

    /// 记录供应商请求结果
    pub async fn record_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
        success: bool,
        error_msg: Option<String>,
    ) -> Result<(), AppError> {
        // 1. 按应用独立获取熔断器配置
        let failure_threshold = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => app_config.circuit_failure_threshold,
            Err(_) => 5, // 默认值
        };

        // 2. 更新熔断器状态
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

        if success {
            breaker.record_success(used_half_open_permit).await;
            // 账号级联动（成功解封）：账号熔断只由致命失败打开，也只有成功
            // 能闭合它 —— 普通失败不碰账号维度。半开探测成功攒够阈值自然回落
            // Closed，与档位熔断同一套恢复语义。
            if let Some(provider) = self
                .db
                .get_provider_by_id(provider_id, app_type)
                .ok()
                .flatten()
            {
                if let Some(account_key) = account_circuit_key(app_type, &provider) {
                    let breakers = self.circuit_breakers.read().await;
                    // 只记录已存在的账号熔断器：没打开过就不为记账凭空建条目
                    if let Some(account_breaker) = breakers.get(&account_key) {
                        account_breaker.record_success(false).await;
                    }
                }
            }
        } else {
            let tripped = breaker.record_failure(used_half_open_permit).await;
            // 非致命跳闸落 crowd 事件表（站点侧信号；致命跳闸是用户自身凭证
            // 问题，对齐 crowd errors 口径不计）。失败只打日志——计数是尽力而为
            // 的统计，不能反过来影响转发主链路。
            if tripped {
                if let Err(e) = crate::crowd::events::record_breaker_trip(
                    self.db.as_ref(),
                    provider_id,
                    app_type,
                ) {
                    log::debug!("[{app_type}] 记录跳闸事件失败: {e}");
                }
            }
        }

        // 3. 更新数据库健康状态（使用配置的阈值）
        self.db
            .update_provider_health_with_threshold(
                provider_id,
                app_type,
                success,
                error_msg.clone(),
                failure_threshold,
            )
            .await?;

        Ok(())
    }

    /// 记录供应商请求结果（致命失败变体）
    ///
    /// 与 [`record_result`](Self::record_result) 的失败分支同步更新 DB 健康度，
    /// 区别只在熔断器：致命失败一次即 Open 且用长冷却
    /// （[`CircuitBreaker::record_fatal_failure`](crate::proxy::circuit_breaker::CircuitBreaker::record_fatal_failure)）。
    /// 什么时候算「致命」由调用方分类（forwarder 按上游状态码 401/402/403）。
    ///
    /// 致命 ⇒ 账号级升级：凭证与余额是账号级事实，同站同账号的其他分组
    /// 必然同样 401/402 —— 账号熔断打开后，整个账号的档位在选路阶段被
    /// 排除（[`Self::select_providers`]），不再逐组撞墙。
    pub async fn record_fatal_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
        error_msg: Option<String>,
    ) -> Result<(), AppError> {
        // 1. 按应用独立获取熔断器配置
        let failure_threshold = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => app_config.circuit_failure_threshold,
            Err(_) => 5, // 默认值
        };

        // 2. 更新熔断器状态（致命失败）
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        let _fatal_tripped = breaker.record_fatal_failure(used_half_open_permit).await;

        // 3. 账号级熔断同步打开（致命一次即开，长冷却）
        if let Some(provider) = self
            .db
            .get_provider_by_id(provider_id, app_type)
            .ok()
            .flatten()
        {
            if let Some(account_key) = account_circuit_key(app_type, &provider) {
                let account_breaker = self.get_or_create_circuit_breaker(&account_key).await;
                let _ = account_breaker.record_fatal_failure(false).await;
            }
        }

        // 4. 更新数据库健康状态（与普通失败同一张表）
        self.db
            .update_provider_health_with_threshold(
                provider_id,
                app_type,
                false,
                error_msg.clone(),
                failure_threshold,
            )
            .await?;

        Ok(())
    }

    /// 重置熔断器（手动恢复）
    pub async fn reset_circuit_breaker(&self, circuit_key: &str) {
        let breakers = self.circuit_breakers.read().await;
        if let Some(breaker) = breakers.get(circuit_key) {
            breaker.reset().await;
        }
    }

    /// 批量读熔断器快照（看板「熔断/自动重试倒计时」用）。Closed（含从未
    /// 记录过结果的）不进 map —— 只上报当前不正常的。
    ///
    /// 档位自身的熔断优先；档位自身 Closed 但**账号级熔断**打开时上报账号
    /// 快照 —— 致命错误按账号升级后，同账号其他分组确实整段不可用，看板
    /// 必须如实显示，否则「这家好好的为什么不接流量」无从解释。
    pub async fn breaker_states(
        &self,
        app_type: &str,
        providers: &[Provider],
    ) -> HashMap<String, crate::proxy::circuit_breaker::BreakerSnapshot> {
        let mut result = HashMap::new();
        for provider in providers {
            let circuit_key = format!("{app_type}:{}", provider.id);
            let breaker = self
                .circuit_breakers
                .read()
                .await
                .get(&circuit_key)
                .cloned();
            let mut snapshot = match breaker {
                Some(breaker) => breaker.snapshot().await,
                None => None,
            };
            {
                if let Some(account_key) = account_circuit_key(app_type, provider) {
                    // 只读既有条目：账号从未熔断过就不为看板凭空建熔断器
                    let account_breaker = {
                        let breakers = self.circuit_breakers.read().await;
                        breakers.get(&account_key).cloned()
                    };
                    if let Some(account_breaker) = account_breaker {
                        let account_snapshot = account_breaker.snapshot().await;
                        if snapshot.is_none()
                            || account_snapshot.as_ref().is_some_and(|state| {
                                state.reopen_in_secs.is_some_and(|secs| secs > 0)
                            })
                        {
                            snapshot = account_snapshot;
                        }
                    }
                }
            }
            if let Some(snapshot) = snapshot {
                result.insert(provider.id.clone(), snapshot);
            }
        }
        result
    }

    /// 重置指定供应商的熔断器
    ///
    /// 账号级连带重置：余额/凭证是账号级事实，用户点「重新启用」（通常已
    /// 充值/换好凭证）就该放开整个账号 —— 只重置单档会让同账号其他档位仍
    /// 被账号熔断挡着，看起来像「点了没生效」。
    pub async fn reset_provider_breaker(&self, provider_id: &str, app_type: &str) {
        let circuit_key = format!("{app_type}:{provider_id}");
        self.reset_circuit_breaker(&circuit_key).await;
        if let Some(provider) = self
            .db
            .get_provider_by_id(provider_id, app_type)
            .ok()
            .flatten()
        {
            if let Some(account_key) = account_circuit_key(app_type, &provider) {
                self.reset_circuit_breaker(&account_key).await;
            }
        }
    }

    /// 仅释放 HalfOpen permit，不影响健康统计（neutral 接口）
    ///
    /// 用于整流器等场景：请求结果不应计入 Provider 健康度，
    /// 但仍需释放占用的探测名额，避免 HalfOpen 状态卡死
    pub async fn release_permit_neutral(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
    ) {
        if !used_half_open_permit {
            return;
        }
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.release_half_open_permit();
    }

    /// 更新所有熔断器的配置（热更新）
    pub async fn update_all_configs(&self, config: CircuitBreakerConfig) {
        let breakers = self.circuit_breakers.read().await;
        for breaker in breakers.values() {
            breaker.update_config(config.clone()).await;
        }
    }

    /// 更新指定应用已创建熔断器的配置（热更新）
    pub async fn update_app_configs(&self, app_type: &str, config: CircuitBreakerConfig) {
        let prefix = format!("{app_type}:");
        let breakers = self.circuit_breakers.read().await;
        for (key, breaker) in breakers.iter() {
            if key.starts_with(&prefix) {
                breaker.update_config(config.clone()).await;
            }
        }
    }

    /// 获取熔断器状态
    #[allow(dead_code)]
    pub async fn get_circuit_breaker_stats(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Option<crate::proxy::circuit_breaker::CircuitBreakerStats> {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breakers = self.circuit_breakers.read().await;

        if let Some(breaker) = breakers.get(&circuit_key) {
            Some(breaker.get_stats().await)
        } else {
            None
        }
    }

    /// 获取或创建熔断器
    async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
        // 先尝试读锁获取
        {
            let breakers = self.circuit_breakers.read().await;
            if let Some(breaker) = breakers.get(key) {
                return breaker.clone();
            }
        }

        // 如果不存在，获取写锁创建
        let mut breakers = self.circuit_breakers.write().await;

        // 双重检查，防止竞争条件
        if let Some(breaker) = breakers.get(key) {
            return breaker.clone();
        }

        // 从 key 中提取 app_type (格式: "app_type:provider_id")
        let app_type = key.split(':').next().unwrap_or("claude");

        // 按应用独立读取熔断器配置
        let config = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => crate::proxy::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: app_config.circuit_failure_threshold,
                success_threshold: app_config.circuit_success_threshold,
                timeout_seconds: app_config.circuit_timeout_seconds as u64,
                error_rate_threshold: app_config.circuit_error_rate_threshold,
                min_requests: app_config.circuit_min_requests,
            },
            Err(_) => crate::proxy::circuit_breaker::CircuitBreakerConfig::default(),
        };

        let breaker = Arc::new(CircuitBreaker::new(config));
        breakers.insert(key.to_string(), breaker.clone());

        breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    fn managed_codex_official(id: &str, account_id: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            "OpenAI Official".to_string(),
            json!({ "auth": {}, "config": "" }),
            None,
        );
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some(account_id.to_string()),
            }),
            ..Default::default()
        });
        provider
    }

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload settings");

            Self {
                dir,
                original_home,
                original_userprofile,
                original_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    /// breaker_states：致命失败打开的熔断器带长冷却快照；Closed/未记录的不进 map。
    #[tokio::test]
    #[serial]
    async fn application_routing_migrates_once_and_toggle_preserves_selection() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        for id in ["a", "b", "c"] {
            db.save_provider(
                "claude",
                &Provider::with_id(id.into(), id.into(), json!({}), None),
            )
            .unwrap();
        }
        db.set_current_provider("claude", "b").unwrap();
        super::super::auto_strategy::set_manual_order(&db, "claude", &["c".into(), "b".into()])
            .unwrap();
        super::super::auto_strategy::set_enabled(&db, "claude", true).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();
        let _state = crate::store::AppState::new(db.clone());
        assert!(super::super::application_routing::failover_enabled(&db, "claude").unwrap());
        assert!(db.get_failover_queue("claude").unwrap().is_empty());
        assert!(!super::super::auto_strategy::is_auto_mode_enabled(
            &db, "claude"
        ));
        let ids = || {
            super::super::application_routing::ordered_providers(&db, "claude")
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(), vec!["c", "b", "a"]);
        super::super::application_routing::set_failover(&db, "claude", false)
            .await
            .unwrap();
        assert_eq!(
            db.get_current_provider("claude").unwrap().as_deref(),
            Some("b")
        );
        assert!(
            super::super::application_routing::set_failover(&db, "claude", true)
                .await
                .is_err()
        );
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();
        super::super::application_routing::set_failover(&db, "claude", true)
            .await
            .unwrap();
        assert_eq!(
            db.get_current_provider("claude").unwrap().as_deref(),
            Some("b")
        );
        super::super::application_routing::migrate(&db, "claude").unwrap();
        assert_eq!(ids(), vec!["c", "b", "a"]);
        let before = db.conn.lock().unwrap().total_changes();
        assert!(super::super::application_routing::set_order(
            &db,
            "claude",
            &["a".into(), "a".into()]
        )
        .is_err());
        assert!(
            super::super::application_routing::set_order(&db, "claude", &["missing".into()])
                .is_err()
        );
        assert_eq!(db.conn.lock().unwrap().total_changes(), before);
        assert_eq!(ids(), vec!["c", "b", "a"]);
    }

    #[tokio::test]
    #[serial]
    async fn application_routing_manual_selection_ignores_model_and_breaker() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        for id in ["a", "b", "c"] {
            db.save_provider(
                "claude",
                &Provider::with_id(
                    id.into(),
                    id.into(),
                    json!({"env": {"ANTHROPIC_MODEL": "actual-model"}}),
                    None,
                ),
            )
            .unwrap();
        }
        db.set_current_provider("claude", "b").unwrap();
        super::super::auto_strategy::set_model_pref(&db, "claude", Some("selected-model")).unwrap();
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();
        let router = ProviderRouter::new(db.clone());
        router
            .record_fatal_result("b", "claude", false, Some("401".into()))
            .await
            .unwrap();
        let providers = router.select_providers("claude").await.unwrap();
        assert_eq!(
            providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
        assert!(
            super::super::application_routing::set_model(&db, "claude", Some("other-model"))
                .is_err()
        );
        assert_eq!(
            super::super::auto_strategy::get_model_pref(&db, "claude").as_deref(),
            Some("selected-model")
        );
        assert_eq!(
            db.get_current_provider("claude").unwrap().as_deref(),
            Some("b")
        );
    }

    #[cfg(feature = "gui")]
    #[tokio::test]
    #[serial]
    async fn application_routing_getter_is_pure_and_shows_all_apps_and_errors() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let state = crate::store::AppState::new(db.clone());
        for app in ["claude", "pi"] {
            for id in ["a", "b"] {
                db.save_provider(
                    app,
                    &Provider::with_id(id.into(), id.into(), json!({}), None),
                )
                .unwrap();
            }
            db.set_current_provider(app, "b").unwrap();
            db.update_provider_health_with_threshold(
                "a",
                app,
                false,
                Some("upstream failure".into()),
                5,
            )
            .await
            .unwrap();
            let before = db.conn.lock().unwrap().total_changes();
            let board = crate::commands::application_routing_impl(&state, app)
                .await
                .unwrap();
            assert_eq!(
                db.conn.lock().unwrap().total_changes(),
                before,
                "view read must not mutate the database"
            );
            assert_eq!(board.tiers.len(), 2);
            assert_eq!(
                board.tiers[0].skip_reason, None,
                "recorded error is not a routing exclusion"
            );
            assert_eq!(board.tiers[0].error_rate, None);
            assert!(
                board.model_options.is_empty(),
                "native mode must not expose proxy-only model controls"
            );
            assert_eq!(
                board.tiers[0].tier.last_error.as_deref(),
                Some("upstream failure")
            );
            assert!(board.tiers[1].tier.is_current);
            for (request, status, age) in
                [("ok", 200, 30), ("fail", 500, 60), ("old", 500, 8 * 86400)]
            {
                db.conn.lock().unwrap().execute(
                    "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, latency_ms, status_code, created_at) VALUES (?1, 'a', ?2, 'model', 10, ?3, ?4)",
                    rusqlite::params![format!("{app}-{request}"), app, status, chrono::Utc::now().timestamp() - age],
                ).unwrap();
            }
            let with_usage = crate::commands::application_routing_impl(&state, app)
                .await
                .unwrap();
            assert_eq!(
                with_usage.tiers[0].error_rate, None,
                "request history cannot backfill missing attempt outcomes"
            );
            assert_eq!(with_usage.tiers[1].error_rate, None);
            super::super::application_routing::set_order(&db, app, &["b".into(), "a".into()])
                .unwrap();
            let board = crate::commands::application_routing_impl(&state, app)
                .await
                .unwrap();
            assert_eq!(board.tiers[0].tier.provider_id, "b");
            if app == "claude" {
                let mut selected = db.get_provider_by_id("b", app).unwrap().unwrap();
                selected.settings_config = json!({
                    "env": {"ANTHROPIC_MODEL": "native-model"},
                    "modelCatalog": {"models": [{"model": "preferred-model"}]}
                });
                db.save_provider(app, &selected).unwrap();
                super::super::auto_strategy::set_model_pref(&db, app, Some("preferred-model"))
                    .unwrap();
                let dormant = crate::commands::application_routing_impl(&state, app)
                    .await
                    .unwrap();
                assert!(!dormant.routing_active);
                assert!(dormant.model_options.is_empty());
                assert_eq!(
                    dormant.tiers[0].tier.effective_model.as_deref(),
                    Some("native-model")
                );
            }
            if app == "pi" {
                assert!(!board.auto_failover_enabled);
                assert!(board.tiers.iter().all(|tier| !tier.can_failover));
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn application_routing_honors_current_and_only_later_priorities() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        for (index, id) in ["a", "b", "c"].iter().enumerate() {
            let mut provider = Provider::with_id(id.to_string(), id.to_string(), json!({}), None);
            provider.sort_index = Some(index);
            db.save_provider("claude", &provider).unwrap();
            db.add_to_failover_queue("claude", id).unwrap();
        }
        db.set_current_provider("claude", "b").unwrap();
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();
        let router = ProviderRouter::new(db.clone());
        let selected = router.select_providers("claude").await.unwrap();
        assert_eq!(
            selected.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        db.set_current_provider("claude", "c").unwrap();
        let selected = router.select_providers("claude").await.unwrap();
        assert_eq!(
            selected.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["c"]
        );
    }

    #[tokio::test]
    #[serial]
    async fn breaker_states_report_open_only_for_unhealthy_breakers() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let dead = Provider::with_id("dead".to_string(), "Dead".to_string(), json!({}), None);
        let fine = Provider::with_id("fine".to_string(), "Fine".to_string(), json!({}), None);
        db.save_provider("claude", &dead).unwrap();
        db.save_provider("claude", &fine).unwrap();

        let router = ProviderRouter::new(db.clone());
        router
            .record_fatal_result("dead", "claude", false, Some("403".to_string()))
            .await
            .unwrap();

        let states = router.breaker_states("claude", &[dead, fine]).await;
        let snapshot = states.get("dead").expect("致命打开的熔断器必须上报");
        assert!(!snapshot.half_open);
        let remaining = snapshot.reopen_in_secs.expect("Open 带倒计时");
        assert!(
            remaining > 1790 && remaining <= 1800,
            "长冷却 1800：{remaining}"
        );
        assert!(!states.contains_key("fine"), "正常（Closed）熔断器不上报");
    }

    /// 托管档（中转站 tier）：`website_url`=站点 origin、meta 带账号身份 ——
    /// 形状对齐 provision 建档路径（commands/relay.rs 落库字段）。
    fn managed_tier(site: &str, account: i64, group: i64) -> Provider {
        let id = crate::relay::provision::provider_id_for(site, Some(account), group);
        let mut provider = Provider::with_id(
            id,
            format!("{site} · group {group}"),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": format!("{site}/v1"),
                    "ANTHROPIC_AUTH_TOKEN": "tok",
                }
            }),
            Some(site.to_string()),
        );
        provider.meta = Some(ProviderMeta {
            loongport_account_id: Some(account),
            ..ProviderMeta::default()
        });
        provider
    }

    /// 致命失败（凭证/余额级）按账号升级：同站同账号的其他分组在选路阶段被
    /// 账号级熔断排除 —— 余额是账号级事实，逐组撞 402 只是浪费用户请求。
    #[tokio::test]
    #[serial]
    async fn fatal_failure_excludes_sibling_tiers_of_the_same_account() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let a1 = managed_tier("https://a.example", 1, 11);
        let a2 = managed_tier("https://a.example", 1, 22);
        let b = managed_tier("https://b.example", 2, 33);
        for provider in [&a1, &a2, &b] {
            db.save_provider("claude", provider).unwrap();
        }
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        assert_eq!(
            router.select_providers("claude").await.unwrap().len(),
            3,
            "基线：无熔断时全部档位都在候选"
        );

        router
            .record_fatal_result(&a1.id, "claude", false, Some("402".to_string()))
            .await
            .unwrap();

        let selected: Vec<String> = router
            .select_providers("claude")
            .await
            .unwrap()
            .into_iter()
            .map(|provider| provider.id)
            .collect();
        assert!(!selected.contains(&a1.id), "肇事档位被自身熔断排除");
        assert!(
            !selected.contains(&a2.id),
            "同账号兄弟档位被账号级熔断排除（自身熔断器还是 Closed）"
        );
        assert!(selected.contains(&b.id), "其他账号不受连坐");
    }

    /// 「重新启用」连带放开整个账号：用户充值后点重置，同账号其余档位必须
    /// 一起回来 —— 只重置单档会让账号熔断继续挡着兄弟档位，看起来像没生效。
    #[tokio::test]
    #[serial]
    async fn reset_provider_breaker_releases_the_whole_account() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let a1 = managed_tier("https://a.example", 1, 11);
        let a2 = managed_tier("https://a.example", 1, 22);
        for provider in [&a1, &a2] {
            db.save_provider("claude", provider).unwrap();
        }
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        router
            .record_fatal_result(&a1.id, "claude", false, Some("402".to_string()))
            .await
            .unwrap();
        router.reset_provider_breaker(&a1.id, "claude").await;

        assert_eq!(
            router.select_providers("claude").await.unwrap().len(),
            2,
            "重置必须连带放开账号级熔断"
        );
    }

    /// 看板如实显示账号级熔断：档位自身 Closed 但账号熔断打开时，兄弟档位
    /// 上报账号快照（Open + 长冷却），不能显示成「健康」。
    #[tokio::test]
    #[serial]
    async fn breaker_states_surface_account_open_on_sibling_tiers() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let a1 = managed_tier("https://a.example", 1, 11);
        let a2 = managed_tier("https://a.example", 1, 22);
        let b = managed_tier("https://b.example", 2, 33);
        for provider in [&a1, &a2, &b] {
            db.save_provider("claude", provider).unwrap();
        }

        let router = ProviderRouter::new(db.clone());
        router
            .record_fatal_result(&a1.id, "claude", false, Some("402".to_string()))
            .await
            .unwrap();

        let (a1_id, a2_id, b_id) = (a1.id.clone(), a2.id.clone(), b.id.clone());
        let states = router.breaker_states("claude", &[a1, a2, b]).await;
        assert!(states.contains_key(&a1_id), "肇事档位：自身熔断 Open");
        let sibling = states
            .get(&a2_id)
            .expect("兄弟档位必须上报账号级熔断，不能显示成健康");
        assert!(!sibling.half_open);
        let remaining = sibling.reopen_in_secs.expect("Open 带倒计时");
        assert!(
            remaining > 1790 && remaining <= 1800,
            "账号级致命长冷却 1800：{remaining}"
        );
        assert!(!states.contains_key(&b_id), "其他账号不上报");
    }

    #[tokio::test]
    #[serial]
    async fn test_provider_router_creation() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let router = ProviderRouter::new(db);

        let breaker = router.get_or_create_circuit_breaker("claude:test").await;
        assert!(breaker.allow_request().await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_disabled_uses_current_provider() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_honors_current_after_earlier_priority() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 设置 sort_index 来控制顺序：b=1, a=2
        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.sort_index = Some(2);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        db.add_to_failover_queue("claude", "b").unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_honors_current_outside_legacy_queue() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        // 只把 b 加入故障转移队列（模拟“当前供应商不在队列里”的常见配置）
        db.add_to_failover_queue("claude", "b").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "a");
    }

    /// 队列里的托管档位必须能被选路（自动模式选路的地基）。
    ///
    /// 历史上托管档位被挡在队列外（见 `relay/managed.rs` 的说明：守卫防的是
    /// 非接管态下跳过 ChatGPT 编排，而这条链的切换点都先验证接管态，场景不可达，
    /// 2026-08-15 修根移除）。这条测试钉住移除后的契约：托管 id 进队列后，
    /// `select_providers` 照常返回它，后续熔断/热切换链路对它一视同仁。
    #[tokio::test]
    #[serial]
    async fn select_providers_serves_managed_tiers_in_failover_queue() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let managed_id =
            crate::relay::provision::provider_id_for("https://bestapi.store", Some(1), 42);
        assert!(
            crate::relay::is_managed(&managed_id),
            "fixture 必须是托管形状的 id"
        );

        let provider = Provider::with_id(
            managed_id.clone(),
            "Managed Tier".to_string(),
            json!({}),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        db.add_to_failover_queue("claude", &managed_id).unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, managed_id);
    }

    /// 自动模式开启时：候选 = 该应用全部托管档位，按策略排序（默认 cheapest），
    /// 与故障转移队列无关（队列里有别的 provider 也不掺进来）。
    #[tokio::test]
    #[serial]
    async fn select_providers_auto_mode_ranks_managed_tiers_by_strategy() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let cheap = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let expensive = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        db.save_provider(
            "claude",
            &Provider::with_id(expensive.clone(), "Expensive".to_string(), json!({}), None),
        )
        .unwrap();
        db.save_provider(
            "claude",
            &Provider::with_id(cheap.clone(), "Cheap".to_string(), json!({}), None),
        )
        .unwrap();
        db.set_tier_rate_multiplier("claude", &cheap, Some(0.5))
            .unwrap();
        db.set_tier_rate_multiplier("claude", &expensive, Some(2.0))
            .unwrap();

        // 队列里塞一个非托管 provider —— 自动模式下必须被无视
        db.save_provider(
            "claude",
            &Provider::with_id(
                "vendor-1".to_string(),
                "Vendor".to_string(),
                json!({}),
                None,
            ),
        )
        .unwrap();
        db.add_to_failover_queue("claude", "vendor-1").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(
            providers.len(),
            3,
            "all providers participate regardless of account origin"
        );
        assert!(providers.iter().any(|p| p.id == cheap));
        assert!(providers.iter().any(|p| p.id == expensive));
        assert!(providers.iter().any(|p| p.id == "vendor-1"));
    }

    /// 自动模式开启但没有托管档位：回退常规选路（故障转移队列），
    /// 不能因为开了自动模式就让请求无供应商可用。
    #[tokio::test]
    #[serial]
    async fn select_providers_auto_mode_without_tiers_falls_back_to_failover() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        db.save_provider(
            "claude",
            &Provider::with_id(
                "vendor-1".to_string(),
                "Vendor".to_string(),
                json!({}),
                None,
            ),
        )
        .unwrap();
        db.add_to_failover_queue("claude", "vendor-1").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "vendor-1");
    }

    #[tokio::test]
    #[serial]
    async fn codex_official_current_stays_single_route_when_failover_is_stale() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let official = managed_codex_official("official-a", "account-a");
        let fallback = Provider::with_id(
            "fallback".to_string(),
            "Fallback".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &official).unwrap();
        db.save_provider("codex", &fallback).unwrap();
        db.set_current_provider("codex", &official.id).unwrap();
        db.add_to_failover_queue("codex", &fallback.id).unwrap();

        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let state = crate::store::AppState::new(db.clone());
        let board = crate::commands::application_routing_impl(&state, "codex")
            .await
            .unwrap();
        let relay_row = board
            .tiers
            .iter()
            .find(|p| p.tier.provider_id == fallback.id)
            .unwrap();
        assert!(
            relay_row.can_failover,
            "current account does not change relay capability"
        );
        assert_eq!(
            relay_row.skip_reason.as_deref(),
            Some("current_official_account")
        );
        let providers = ProviderRouter::new(db)
            .select_providers("codex")
            .await
            .unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, official.id);
    }

    #[tokio::test]
    #[serial]
    async fn stale_codex_official_queue_entries_are_not_retry_targets() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let current = Provider::with_id(
            "third-party".to_string(),
            "Third Party".to_string(),
            json!({}),
            None,
        );
        let official = managed_codex_official("official-a", "account-a");
        let fallback = Provider::with_id(
            "fallback".to_string(),
            "Fallback".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &current).unwrap();
        db.save_provider("codex", &official).unwrap();
        db.save_provider("codex", &fallback).unwrap();
        db.set_current_provider("codex", &current.id).unwrap();
        db.add_to_failover_queue("codex", &official.id).unwrap();
        db.add_to_failover_queue("codex", &fallback.id).unwrap();

        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let providers = ProviderRouter::new(db)
            .select_providers("codex")
            .await
            .unwrap();
        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            vec!["third-party"]
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_select_providers_does_not_consume_half_open_permit() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();

        db.add_to_failover_queue("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        router
            .record_result("b", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        let providers = router.select_providers("claude").await.unwrap();
        assert_eq!(providers.len(), 2);

        assert!(router.allow_provider_request("b", "claude").await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_release_permit_neutral_frees_half_open_slot() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 配置熔断器：1 次失败即熔断，0 秒超时立即进入 HalfOpen
        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        db.save_provider("claude", &provider_a).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 触发熔断：1 次失败
        router
            .record_result("a", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        // 第一次请求：获取 HalfOpen 探测名额
        let first = router.allow_provider_request("a", "claude").await;
        assert!(first.allowed);
        assert!(first.used_half_open_permit);

        // 第二次请求应被拒绝（名额已被占用）
        let second = router.allow_provider_request("a", "claude").await;
        assert!(!second.allowed);

        // 使用 release_permit_neutral 释放名额（不影响健康统计）
        router
            .release_permit_neutral("a", "claude", first.used_half_open_permit)
            .await;

        // 第三次请求应被允许（名额已释放）
        let third = router.allow_provider_request("a", "claude").await;
        assert!(third.allowed);
        assert!(third.used_half_open_permit);
    }
}
