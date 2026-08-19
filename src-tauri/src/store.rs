use crate::database::Database;
use crate::proxy::providers::codex_oauth_auth::CodexOAuthManager;
use crate::relay::browser_bridge::BrowserBridge;
use crate::relay::model_verification::coordinator::ModelVerificationCoordinator;
use crate::relay::purchase_session::PurchaseSessionCoordinator;
use crate::services::{ProxyService, UsageCache};
use std::sync::Arc;

/// 全局应用状态
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub proxy_service: ProxyService,
    pub usage_cache: Arc<UsageCache>,
    pub model_verification: Arc<ModelVerificationCoordinator>,
    /// 浏览器代拉 API 请求的回传调度器（登录窗代拉防护站时用）。
    ///
    /// 挂 app 级而不是随登录流程走：provision 等流程在登录命令**返回之后**才跑，
    /// 那时登录流程的局部通道早已 drop，只有这里的注册表还活着。
    pub browser_bridge: Arc<BrowserBridge>,
    /// 充值窗口对 NewAPI refresh credential 轮换的独占协调器。
    ///
    /// 挂 app 级：充值窗口与后台续期（`usable_relay`）是两条互不相识的调用链，
    /// 「这个账号的 refresh 轮换权现在归谁」是后端会话协调的事实，不是 React 的
    /// busy 状态 —— 前端刷新/重挂载会丢，而 lease 必须活到窗口真正销毁为止。
    pub purchase_sessions: Arc<PurchaseSessionCoordinator>,
    // 内部已使用细粒度锁（accounts/access_tokens/refresh_locks），所有方法均为
    // `&self`，无需外层 RwLock；避免持有粗粒度锁跨网络刷新导致的连锁阻塞。
    pub codex_oauth_manager: Arc<CodexOAuthManager>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(db: Arc<Database>) -> Self {
        let codex_oauth_manager =
            Arc::new(CodexOAuthManager::new(crate::config::get_app_config_dir()));
        let proxy_service =
            ProxyService::new_with_codex_oauth_manager(db.clone(), codex_oauth_manager.clone());
        let model_verification = Arc::new(ModelVerificationCoordinator::new(db.clone()));

        Self {
            db,
            proxy_service,
            usage_cache: Arc::new(UsageCache::new()),
            model_verification,
            browser_bridge: Arc::new(BrowserBridge::default()),
            purchase_sessions: Arc::new(PurchaseSessionCoordinator::default()),
            codex_oauth_manager,
        }
    }
}
