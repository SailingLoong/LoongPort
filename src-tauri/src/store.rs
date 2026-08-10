use crate::database::Database;
use crate::relay::model_verification::coordinator::ModelVerificationCoordinator;
use crate::services::{ProxyService, UsageCache};
use std::sync::Arc;

/// 全局应用状态
pub struct AppState {
    pub db: Arc<Database>,
    pub proxy_service: ProxyService,
    pub usage_cache: Arc<UsageCache>,
    pub model_verification: Arc<ModelVerificationCoordinator>,
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
        }
    }
}
