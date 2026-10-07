//! Commands compose the established account/vault owner with owned login work.
use super::{
    api::PublicError,
    core::{AccountSnapshot, OAuthFamily},
    key_intent::{FreshIntent, KeyIntent, KeyScope, Reservation},
    library_context::LibraryContext,
    oauth::{
        FlowError, FlowStage, LoginFlowStore, LoginProgress, VaultBinding, WorkKind, WorkLease,
    },
    oauth_service::{CatalogView, LoginPersistence, LoginService, StoreFailure, StoreFuture},
    official::PollToken,
    official_http::ReqwestOfficialTransport,
    runtime::{run_owned, RuntimeError},
    transaction::{CaptureCommitOutcome, CatalogStatus, TransactionError, VaultAccountStore},
};
use crate::database::Database;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

struct Persistence {
    db: Arc<Database>,
}
fn binding(db: &Database) -> Result<VaultBinding, RuntimeError> {
    let session = db.secret_session();
    let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
    let meta = vault.metadata();
    Ok(VaultBinding {
        root: session.root().into(),
        vault_id: meta.vault_id.clone(),
        key_id: meta.key_id.clone(),
        revision: meta.revision,
    })
}
fn check_binding(db: &Database, expected: &VaultBinding) -> Result<(), RuntimeError> {
    if &binding(db)? == expected {
        Ok(())
    } else {
        Err(TransactionError::SourceChanged.into())
    }
}
impl Persistence {
    async fn access<R: Send + 'static>(
        &self,
        expected: VaultBinding,
        operation: impl FnOnce(&VaultAccountStore<'_>) -> Result<R, TransactionError> + Send + 'static,
    ) -> Result<R, StoreFailure> {
        run_owned(Arc::clone(&self.db), move |db| {
            check_binding(db, &expected)?;
            let session = db.secret_session();
            let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
            operation(&VaultAccountStore::new(session.root(), &vault)?).map_err(Into::into)
        })
        .await
        .map_err(|error| match error {
            RuntimeError::Transaction(
                TransactionError::CatalogChanged | TransactionError::SourceChanged,
            ) => StoreFailure::Changed,
            RuntimeError::VaultUnavailable => StoreFailure::Unavailable,
            RuntimeError::Transaction(TransactionError::Checkpoint(
                super::checkpoint::CheckpointError::ResourceLimit,
            )) => StoreFailure::Capacity,
            _ => StoreFailure::Unknown,
        })
    }
}
impl LoginPersistence for Persistence {
    fn check(&self, expected: &VaultBinding) -> Result<(), StoreFailure> {
        check_binding(&self.db, expected).map_err(|_| StoreFailure::Changed)
    }
    fn catalog<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        snapshot: &'a AccountSnapshot,
    ) -> StoreFuture<'a, CatalogView> {
        let binding = binding.clone();
        let context = context.clone();
        let id = snapshot.identity().clone();
        Box::pin(async move {
            self.access(binding, move |store| {
                let native = context.cipher().map_err(TransactionError::Admission)?;
                let status = store.catalog_status(&native)?;
                let catalog = store.import_catalog(&native, &status.revision)?;
                Ok(CatalogView {
                    revision: status.revision,
                    duplicate: catalog.get(&id).is_some(),
                })
            })
            .await
        })
    }
    fn reserve<'a>(
        &'a self,
        binding: &'a VaultBinding,
        scope: KeyScope,
    ) -> StoreFuture<'a, Reservation> {
        Box::pin(async move {
            self.access(binding.clone(), move |store| {
                store.reserve_key_intent(scope)
            })
            .await
        })
    }
    fn intent<'a>(
        &'a self,
        binding: &'a VaultBinding,
        scope: &'a KeyScope,
    ) -> StoreFuture<'a, Option<KeyIntent>> {
        let scope = scope.clone();
        Box::pin(async move {
            self.access(binding.clone(), move |store| store.key_intent(&scope))
                .await
        })
    }
    fn mark_created<'a>(
        &'a self,
        binding: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            self.access(binding.clone(), move |store| store.mark_key_created(&grant))
                .await
        })
    }
    fn clear_unsubmitted<'a>(
        &'a self,
        binding: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            self.access(binding.clone(), move |store| {
                store.clear_unsubmitted_key(&grant)
            })
            .await
        })
    }
    fn clear_resolved<'a>(
        &'a self,
        binding: &'a VaultBinding,
        intent: &'a KeyIntent,
    ) -> StoreFuture<'a, ()> {
        let intent = intent.clone();
        Box::pin(async move {
            self.access(binding.clone(), move |store| {
                store.clear_resolved_key(&intent)
            })
            .await
        })
    }
    fn saved_receipt<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        request_id: &'a str,
    ) -> StoreFuture<'a, Option<super::checkpoint::LoginReceipt>> {
        let context = context.clone();
        let request_id = request_id.to_owned();
        Box::pin(async move {
            self.access(binding.clone(), move |store| {
                store.login_receipt(
                    &context.cipher().map_err(TransactionError::Admission)?,
                    &request_id,
                )
            })
            .await
        })
    }
    fn save<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        revision: &'a str,
        request_id: &'a str,
        snapshot: &'a AccountSnapshot,
        evidence: Option<&'a super::session_checks::SessionCheckReport>,
        update_duplicate: bool,
    ) -> StoreFuture<'a, CaptureCommitOutcome> {
        let context = context.clone();
        let revision = revision.to_owned();
        let request_id = request_id.to_owned();
        let snapshot = snapshot.clone();
        let evidence = evidence.cloned();
        Box::pin(async move {
            self.access(binding.clone(), move |store| {
                store.save_login_profile(
                    &context.cipher().map_err(TransactionError::Admission)?,
                    &revision,
                    &request_id,
                    snapshot,
                    evidence,
                    update_duplicate,
                )
            })
            .await
        })
    }
}
type Service = LoginService<ReqwestOfficialTransport, Persistence>;
fn flows() -> &'static LoginFlowStore {
    static FLOWS: OnceLock<LoginFlowStore> = OnceLock::new();
    FLOWS.get_or_init(LoginFlowStore::default)
}
fn service(db: Arc<Database>) -> Result<Arc<Service>, PublicError> {
    let transport = ReqwestOfficialTransport::new()
        .map_err(|_| PublicError::new("zcode.account.official_unavailable", "queryOriginal"))?;
    let mut service = Service::new(transport, Persistence { db });
    service.flows = flows().clone();
    Ok(Arc::new(service))
}
fn flow_error(_: FlowError) -> PublicError {
    PublicError::new("zcode.account.login_changed", "queryOriginal")
}
fn progress(id: &str) -> Result<LoginProgress, PublicError> {
    flows().progress(id, Instant::now()).map_err(flow_error)
}
async fn poll_owned(service: Arc<Service>, mut lease: WorkLease, id: String) {
    loop {
        let draft = match lease.draft() {
            Ok((_, draft)) => draft,
            Err(_) => return,
        };
        let interval = draft
            .init
            .as_ref()
            .map(|init| init.poll_interval_sec)
            .unwrap_or(1);
        let ticket = lease.ticket();
        match service.poll(lease).await {
            Ok(true) => {
                prepare_owned(Arc::clone(&service), id).await;
                return;
            }
            Err(_) => {
                let _ = service.flows.fail_abandoned(&ticket);
                return;
            }
            Ok(false) => {}
        }
        if service
            .flows
            .status(&id, Instant::now())
            .map_or(true, |status| status.stage != FlowStage::Waiting)
        {
            return;
        }
        let remaining = draft
            .poll_deadline
            .map(|end| end.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        let interval = if service
            .flows
            .progress(&id, Instant::now())
            .is_ok_and(|progress| progress.error.is_some())
        {
            interval.max(15)
        } else {
            interval
        };
        tokio::time::sleep(Duration::from_secs(interval).min(remaining)).await;
        lease = match service.flows.acquire(&id, WorkKind::Poll, Instant::now()) {
            Ok(lease) => lease,
            Err(_) => return,
        };
    }
}
async fn prepare_owned(service: Arc<Service>, id: String) {
    let mut delay = 5;
    loop {
        let lease = match service
            .flows
            .acquire(&id, WorkKind::Prepare, Instant::now())
        {
            Ok(lease) => lease,
            Err(_) => return,
        };
        let ticket = lease.ticket();
        if service.prepare(lease).await.is_err() {
            let _ = service.flows.fail_abandoned(&ticket);
            return;
        }
        if service
            .flows
            .status(&id, Instant::now())
            .map_or(true, |status| status.stage != FlowStage::Preparing)
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
pub(crate) async fn begin(
    db: Arc<Database>,
    family: &str,
    data_root: Option<PathBuf>,
) -> Result<LoginProgress, PublicError> {
    let family = match family {
        "bigmodel" => OAuthFamily::BigModel,
        "zai" => OAuthFamily::Zai,
        _ => {
            return Err(PublicError::new(
                "zcode.account.unsupported_scope",
                "chooseContext",
            ))
        }
    };
    let (bound, context, app_version) = run_owned(Arc::clone(&db), move |db| {
        let context = super::native_context::library_context(data_root.as_deref())?;
        let bound = binding(db)?;
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        VaultAccountStore::new(session.root(), &vault)?.catalog_status(&context.cipher()?)?;
        let app_version = super::native_context::library_app_version(context.data_root());
        Ok((bound, context, app_version))
    })
    .await
    .map_err(PublicError::from)?;
    let service = service(db)?;
    let token = PollToken::new(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| flow_error(FlowError::Capacity))?;
    let id = service
        .flows
        .begin(bound, family, token, Instant::now())
        .map_err(flow_error)?;
    // Acquire synchronously before handing any work to a task.
    let lease = service
        .flows
        .acquire(&id, WorkKind::Init, Instant::now())
        .map_err(flow_error)?;
    let expiry = service.flows.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(600)).await;
        let _ = expiry.expire_due(Instant::now());
        tokio::time::sleep(Duration::from_secs(600)).await;
        let _ = expiry.expire_due(Instant::now());
    });
    let worker_id = id.clone();
    tokio::spawn(async move {
        let ticket = lease.ticket();
        if service
            .initialize(lease, context, app_version)
            .await
            .is_err()
        {
            let _ = service.flows.fail_abandoned(&ticket);
        }
        if let Ok(poll) = service
            .flows
            .acquire(&worker_id, WorkKind::Poll, Instant::now())
        {
            tokio::spawn(poll_owned(Arc::clone(&service), poll, worker_id));
        }
    })
    .await
    .map_err(|_| flow_error(FlowError::Poisoned))?;
    progress(&id)
}
pub(crate) async fn query(db: Arc<Database>, id: String) -> Result<LoginProgress, PublicError> {
    if let Some(kind) = flows().recovery_kind(&id).map_err(flow_error)? {
        let service = service(db)?;
        if let Ok(lease) = service.flows.acquire(&id, kind, Instant::now()) {
            tokio::spawn(async move {
                let ticket = lease.ticket();
                if service.recover(lease).await.is_err() {
                    let _ = service.flows.fail_abandoned(&ticket);
                }
            })
            .await
            .map_err(|_| flow_error(FlowError::Poisoned))?;
        }
    }
    progress(&id)
}
pub(crate) async fn confirm(
    db: Arc<Database>,
    id: String,
    organization: String,
    project: String,
) -> Result<LoginProgress, PublicError> {
    let service = service(db)?;
    let lease = service
        .flows
        .acquire(&id, WorkKind::CreateKey, Instant::now())
        .map_err(flow_error)?;
    tokio::spawn(async move {
        let ticket = lease.ticket();
        if service
            .create_key(lease, &organization, &project)
            .await
            .is_err()
        {
            let _ = service.flows.fail_abandoned(&ticket);
        }
    })
    .await
    .map_err(|_| flow_error(FlowError::Poisoned))?;
    progress(&id)
}
pub(crate) async fn decline(db: Arc<Database>, id: String) -> Result<LoginProgress, PublicError> {
    let status = flows().status(&id, Instant::now()).map_err(flow_error)?;
    if status.stage != FlowStage::KeyRequired || status.key_may_exist {
        return progress(&id);
    }
    let service = service(db)?;
    let lease = service
        .flows
        .acquire(&id, WorkKind::Prepare, Instant::now())
        .map_err(flow_error)?;
    tokio::spawn(async move {
        let ticket = lease.ticket();
        if service.decline_key(lease).await.is_err() {
            let _ = service.flows.fail_abandoned(&ticket);
        }
    })
    .await
    .map_err(|_| flow_error(FlowError::Poisoned))?;
    progress(&id)
}
pub(crate) async fn save(
    db: Arc<Database>,
    id: String,
    update_duplicate: bool,
) -> Result<LoginProgress, PublicError> {
    let service = service(db)?;
    let lease = service
        .flows
        .acquire(&id, WorkKind::Save, Instant::now())
        .map_err(flow_error)?;
    tokio::spawn(async move {
        let ticket = lease.ticket();
        if service.save(lease, update_duplicate).await.is_err() {
            let _ = service.flows.fail_abandoned(&ticket);
        }
    })
    .await
    .map_err(|_| flow_error(FlowError::Poisoned))?;
    progress(&id)
}
pub(crate) fn cancel(id: String) -> Result<LoginProgress, PublicError> {
    flows().cancel(&id, Instant::now()).map_err(flow_error)?;
    progress(&id)
}
pub(crate) async fn catalog(
    db: Arc<Database>,
    data_root: Option<PathBuf>,
) -> Result<CatalogStatus, PublicError> {
    run_owned(db, move |db| {
        let context = super::native_context::library_context(data_root.as_deref())?;
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        VaultAccountStore::new(session.root(), &vault)?
            .catalog_status(&context.cipher()?)
            .map_err(Into::into)
    })
    .await
    .map_err(Into::into)
}
