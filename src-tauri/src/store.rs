use crate::database::Database;
use crate::relay::browser_bridge::BrowserBridge;
use crate::relay::model_verification::coordinator::ModelVerificationCoordinator;
use crate::services::{ProxyService, UsageCache};
use std::sync::Arc;

/// 全局应用状态
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
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(db: Arc<Database>) -> Self {
        let proxy_service = ProxyService::new(db.clone());
        let model_verification = Arc::new(ModelVerificationCoordinator::new(db.clone()));

        Self {
            db,
            proxy_service,
            usage_cache: Arc::new(UsageCache::new()),
            model_verification,
            browser_bridge: Arc::new(BrowserBridge::default()),
        }
    }
}
