use crate::database::Database;
use crate::relay::model_verification::coordinator::ModelVerificationCoordinator;
use crate::relay::model_verification::passive::VerificationIngress;
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
        let (ingress, receiver) = VerificationIngress::channel();
        let proxy_service = ProxyService::new_with_verification(db.clone(), ingress.clone());
        let verifier = Arc::new(
            crate::relay::model_verification::active::BalancedActiveVerifier::new(db.clone()),
        );
        let model_verification = Arc::new(ModelVerificationCoordinator::with_passive_ingress(
            db.clone(),
            verifier,
            Arc::new(crate::relay::model_verification::coordinator::TauriEventSink::default()),
            ingress,
            receiver,
        ));

        Self {
            db,
            proxy_service,
            usage_cache: Arc::new(UsageCache::new()),
            model_verification,
        }
    }
}
