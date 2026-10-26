//! Vault-owned serial claims. Adapted queue behavior from pjpv/zcode-switch
//! f34225686dfef05d84c256a56f868719248f15ff (MIT, licenses/zcode-switch-MIT.txt).
#[cfg(not(feature = "test-hooks"))]
use super::runtime::run_owned;
use super::{
    claim::{Record, Status},
    claim_protocol::{CaptchaConfig, Client, Endpoint, Http, Request},
    claim_queue::{Jobs, Lease},
    library_context::LibraryContext,
    official::{OfficialError, SecretValue, StartJwt},
    transaction::VaultAccountStore,
};
use crate::database::Database;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tauri::{Emitter, Manager};
#[cfg(feature = "test-hooks")]
async fn run_owned<T: Send + 'static>(
    db: Arc<Database>,
    operation: impl FnOnce(&Database) -> Result<T, super::runtime::RuntimeError> + Send + 'static,
) -> Result<T, super::runtime::RuntimeError> {
    let scope = test_support::current();
    if let Some(scope) = &scope {
        scope.assert_db(&db);
    }
    super::runtime::run_owned(db, move |db| match scope {
        Some(scope) => test_support::sync_scope(scope, || operation(db)),
        None => operation(db),
    })
    .await
}
const SETTING: &str = "zcode_claim_v1";
pub(crate) const INTERVAL: Duration = Duration::from_secs(600);
fn jobs() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(Jobs::default)
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    data_root: PathBuf,
    enabled: bool,
    participants: Vec<String>,
    records: BTreeMap<String, Record>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    scopes: BTreeMap<String, Scope>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    enabled: bool,
    participants: Vec<String>,
    records: BTreeMap<String, RecordView>,
    busy: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordView {
    #[serde(flatten)]
    record: Record,
    can_claim: bool,
}
fn load(db: &Database) -> Result<Saved, &'static str> {
    match db.get_setting(SETTING).map_err(|_| "locked")? {
        None => Ok(Saved::default()),
        Some(raw) => serde_json::from_str(&raw).map_err(|_| "unavailable"),
    }
}
fn save(db: &Database, saved: &Saved) -> Result<(), &'static str> {
    let raw = serde_json::to_string(saved).map_err(|_| "unavailable")?;
    db.set_setting(SETTING, &raw).map_err(|_| "locked")
}
fn context(root: Option<&std::path::Path>) -> Result<LibraryContext, &'static str> {
    #[cfg(feature = "test-hooks")]
    if let Some(scope) = test_support::current() {
        return Ok(scope.context(root));
    }
    super::native_context::library_context(root).map_err(|_| "unavailable")
}
fn ids(db: &Database, ctx: &LibraryContext) -> Result<BTreeSet<String>, &'static str> {
    let vault = db.secret_session().read().map_err(|_| "locked")?;
    let store =
        VaultAccountStore::new(db.secret_session().root(), &vault).map_err(|_| "unavailable")?;
    let catalog = store
        .catalog_status(&ctx.cipher().map_err(|_| "unavailable")?)
        .map_err(|_| "changed")?;
    Ok(catalog.profiles.into_iter().map(|p| p.id).collect())
}
fn view(scope: Scope, valid: &BTreeSet<String>) -> View {
    let busy = jobs().busy();
    View {
        enabled: scope.enabled,
        participants: scope
            .participants
            .into_iter()
            .filter(|id| valid.contains(id))
            .collect(),
        records: valid
            .iter()
            .map(|id| {
                let record = scope.records.get(id).cloned().unwrap_or_default();
                let can_claim = !busy && record.may_submit();
                (id.clone(), RecordView { record, can_claim })
            })
            .collect(),
        busy,
    }
}
pub(crate) async fn state(db: Arc<Database>, root: Option<PathBuf>) -> Result<View, &'static str> {
    run_owned(db, move |db| {
        let result = (|| {
            let ctx = context(root.as_deref())?;
            let valid = ids(db, &ctx)?;
            let scope = load(db)?
                .scopes
                .remove(ctx.context_id())
                .unwrap_or_default();
            Ok(view(scope, &valid))
        })();
        Ok(result)
    })
    .await
    .map_err(|_| "locked")?
}
pub(crate) async fn set_auto(
    db: Arc<Database>,
    root: Option<PathBuf>,
    enabled: bool,
    participants: Vec<String>,
) -> Result<View, &'static str> {
    if participants.len() > 100
        || participants.iter().collect::<BTreeSet<_>>().len() != participants.len()
    {
        return Err("invalid");
    }
    if !enabled {
        jobs().cancel();
    }
    run_owned(db, move |db| {
        let result = (|| {
            let ctx = context(root.as_deref())?;
            let valid = ids(db, &ctx)?;
            if participants.iter().any(|id| !valid.contains(id)) {
                return Err("changed");
            }
            let mut saved = load(db)?;
            let scope = saved.scopes.entry(ctx.context_id().into()).or_default();
            scope.data_root = ctx.data_root().into();
            scope.enabled = enabled;
            scope.participants = participants;
            scope.records.retain(|id, _| valid.contains(id));
            let out = view(scope.clone(), &valid);
            save(db, &saved)?;
            Ok(out)
        })();
        Ok(result)
    })
    .await
    .map_err(|_| "unavailable")?
}
struct Prepared {
    ctx: LibraryContext,
    id: String,
    revision: String,
    storage: crate::secrets::storage::StorageIdentity,
    jwt: StartJwt,
    version: String,
}
async fn prepare(
    db: Arc<Database>,
    root: Option<PathBuf>,
    id: String,
) -> Result<Prepared, &'static str> {
    run_owned(db, move |db| {
        let result = (|| {
            #[cfg(feature = "test-hooks")]
            test_support::prepared(&id);
            let ctx = context(root.as_deref())?;
            let vault = db.secret_session().read().map_err(|_| "locked")?;
            let store = VaultAccountStore::new(db.secret_session().root(), &vault)
                .map_err(|_| "unavailable")?;
            let native = ctx.cipher().map_err(|_| "unavailable")?;
            let status = store.catalog_status(&native).map_err(|_| "changed")?;
            let catalog = store
                .import_catalog(&native, &status.revision)
                .map_err(|_| "changed")?;
            let snapshot = catalog
                .profiles()
                .find(|s| s.identity().opaque_id() == id)
                .ok_or("changed")?;
            let jwt =
                super::session_checks::claim_jwt(&native, snapshot).map_err(|_| "loginExpired")?;
            #[cfg(not(feature = "test-hooks"))]
            let version = super::native_context::library_app_version(ctx.data_root())
                .ok_or("appVersionUnknown")?;
            #[cfg(feature = "test-hooks")]
            let version = if test_support::current().is_some() {
                Some("3.11.2".into())
            } else {
                super::native_context::library_app_version(ctx.data_root())
            }
            .ok_or("appVersionUnknown")?;

            Ok(Prepared {
                ctx,
                id,
                revision: status.revision,
                storage: vault.identity(),
                jwt,
                version,
            })
        })();
        Ok(result)
    })
    .await
    .map_err(|_| "unavailable")?
}
fn check(db: &Database, p: &Prepared, lease: &Lease, automatic: bool) -> Result<(), OfficialError> {
    if lease.cancelled() {
        return Err(OfficialError::Cancelled);
    }
    let vault = db
        .secret_session()
        .read()
        .map_err(|_| OfficialError::Cancelled)?;

    if vault.identity() != p.storage {
        return Err(OfficialError::Cancelled);
    }
    let store = VaultAccountStore::new(db.secret_session().root(), &vault)
        .map_err(|_| OfficialError::Cancelled)?;
    let catalog = store
        .import_catalog(
            &p.ctx.cipher().map_err(|_| OfficialError::Cancelled)?,
            &p.revision,
        )
        .map_err(|_| OfficialError::Cancelled)?;
    if !catalog.profiles().any(|s| s.identity().opaque_id() == p.id) {
        return Err(OfficialError::Cancelled);
    }
    drop(vault);
    if automatic {
        let saved = load(db).map_err(|_| OfficialError::Cancelled)?;
        let scope = saved
            .scopes
            .get(p.ctx.context_id())
            .ok_or(OfficialError::Cancelled)?;
        if !scope.enabled || !scope.participants.contains(&p.id) {
            return Err(OfficialError::Cancelled);
        }
    }
    Ok(())
}
async fn write_record(
    db: Arc<Database>,
    ctx_id: String,
    root: PathBuf,
    id: String,
    record: Record,
) -> Result<(), &'static str> {
    run_owned(db, move |db| {
        let result = (|| {
            let ctx = context(Some(&root))?;
            if ctx.context_id() != ctx_id || !ids(db, &ctx)?.contains(&id) {
                return Err("changed");
            }
            let mut saved = load(db)?;
            let scope = saved.scopes.entry(ctx_id).or_default();
            scope.data_root = root;
            #[cfg(feature = "test-hooks")]
            let saved_id = id.clone();
            scope.records.insert(id, record);
            #[cfg(feature = "test-hooks")]
            {
                save(db, &saved)?;
                test_support::persisted(&saved_id);
                Ok(())
            }
            #[cfg(not(feature = "test-hooks"))]
            save(db, &saved)
        })();
        Ok(result)
    })
    .await
    .map_err(|_| "unavailable")?
}
async fn record(db: Arc<Database>, p: &Prepared, r: &Record) -> Result<(), &'static str> {
    write_record(
        db,
        p.ctx.context_id().into(),
        p.ctx.data_root().into(),
        p.id.clone(),
        r.clone(),
    )
    .await
}
async fn previous(db: Arc<Database>, p: &Prepared) -> Result<Record, &'static str> {
    let scope = p.ctx.context_id().to_owned();
    let id = p.id.clone();
    run_owned(db, move |db| {
        load(db)
            .map(|s| {
                s.scopes
                    .get(&scope)
                    .and_then(|s| s.records.get(&id))
                    .cloned()
                    .unwrap_or_default()
            })
            .map_err(|_| super::runtime::RuntimeError::TaskFailed)
    })
    .await
    .map_err(|_| "locked")
}
fn request(p: &Prepared, endpoint: Endpoint) -> Request {
    Request {
        endpoint,
        version: p.version.clone(),
        jwt: p.jwt.clone(),
        plan: None,
        captcha: None,
        region: None,
    }
}
fn data(value: serde_json::Value) -> Result<serde_json::Value, OfficialError> {
    super::claim_protocol::read_data(value)
}
fn active_plan<'a>(balance: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    let now = balance
        .get("server_time")
        .and_then(serde_json::Value::as_i64);
    balance.get("plans")?.as_array()?.iter().find(|plan| {
        plan.get("plan_id").and_then(serde_json::Value::as_str) == Some(id)
            && plan.get("status").and_then(serde_json::Value::as_str) == Some("active")
            && plan
                .get("ends_at")
                .and_then(serde_json::Value::as_i64)
                .zip(now)
                .is_none_or(|(end, now)| end > now)
    })
}
fn discovery_must_stop(error: &OfficialError) -> bool {
    matches!(
        error,
        OfficialError::Unauthorized | OfficialError::Cancelled | OfficialError::Http(429)
    )
}
fn merge_plan_sources(
    preview: Result<Vec<super::claim::Plan>, OfficialError>,
    marketing: Result<Vec<super::claim::Plan>, OfficialError>,
) -> Result<(Vec<super::claim::Plan>, bool), OfficialError> {
    for error in [preview.as_ref().err(), marketing.as_ref().err()]
        .into_iter()
        .flatten()
    {
        if discovery_must_stop(error) {
            return Err(*error);
        }
    }
    let complete = preview.is_ok() && marketing.is_ok();
    let mut plans = match (preview, marketing) {
        (Ok(mut plans), Ok(delivered)) => {
            for plan in delivered {
                if !plans.iter().any(|existing| existing.id == plan.id) {
                    plans.push(plan);
                }
            }
            plans
        }
        (Ok(plans), Err(error)) | (Err(error), Ok(plans)) => {
            log::warn!("ZCode plan discovery incomplete: {error}");
            plans
        }
        (Err(error), Err(_)) => return Err(error),
    };
    plans.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
    Ok((plans, complete))
}
async fn one(
    app: &tauri::AppHandle,
    db: Arc<Database>,
    p: &Prepared,
    lease: &Lease,
    preview_only: bool,
    automatic: bool,
) -> Result<(), &'static str> {
    let device_mid = super::device_settings::current(&db, &p.ctx)?;
    let client = Client(
        Http::new()
            .map_err(|_| "network")?
            .with_data_root(p.ctx.data_root())
            .with_device_mid(device_mid.as_deref()),
    );
    let checked = || check(&db, p, lease, automatic);
    let mut r = previous(db.clone(), p).await?;
    r.checked_at = Some(chrono::Utc::now().timestamp().max(0) as u64);
    // A durable may-have-sent marker admits only a read, including after restart.
    let balance = client
        .request(request(p, Endpoint::Balance), &checked)
        .await
        .and_then(data);
    if r.status == Status::ResultPending {
        if let Ok(balance) = balance {
            r.reconcile(&balance);
        }
        record(db, p, &r).await?;
        return Ok(());
    }
    let balance = match balance {
        Ok(balance) => balance,
        Err(error) => {
            failure(&mut r, error);
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let prior_plan_ids = match super::claim::prior_plan_ids(&balance) {
        Some(ids) => ids,
        None => {
            // A malformed account response must not abort the remaining accounts.
            // Failure to persist the result still stops the round safely.
            failure(&mut r, OfficialError::InvalidResponse);
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let preview = client
        .request(request(p, Endpoint::Preview), &checked)
        .await
        .and_then(|value| {
            if matches!(super::claim::response_code(&value), Some(401 | 403)) {
                Err(OfficialError::Unauthorized)
            } else {
                super::claim::preview(&value).map_err(|_| OfficialError::InvalidResponse)
            }
        });
    if preview.as_ref().err().is_some_and(discovery_must_stop) {
        failure(&mut r, preview.err().unwrap());
        record(db, p, &r).await?;
        return Ok(());
    }
    let marketing = client
        .request(request(p, Endpoint::Marketing), &checked)
        .await
        .and_then(|value| {
            if matches!(super::claim::response_code(&value), Some(401 | 403)) {
                Err(OfficialError::Unauthorized)
            } else {
                super::claim::marketing_plans(&value).map_err(|_| OfficialError::InvalidResponse)
            }
        });
    let (plans, complete) = match merge_plan_sources(preview, marketing) {
        Ok(discovery) => discovery,
        Err(error) => {
            failure(&mut r, error);
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let plan = plans
        .iter()
        .find(|plan| active_plan(&balance, &plan.id).is_none())
        .cloned();
    r.plans = plans;
    r.prior_plan_ids = prior_plan_ids;
    r.reason = None;
    let Some(plan) = plan else {
        if !complete {
            r.status = Status::Unknown;
            r.reason = Some("previewUnavailable".into());
            record(db, p, &r).await?;
            return Ok(());
        }
        r.status = Status::NoClaim;
        if let Some(plan) = r.plans.first().and_then(|p| active_plan(&balance, &p.id)) {
            r.plan_id = plan
                .get("plan_id")
                .and_then(serde_json::Value::as_str)
                .map(String::from);
            r.plan_name = plan
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(String::from);
            r.starts_at = plan.get("starts_at").and_then(serde_json::Value::as_i64);
            r.ends_at = plan.get("ends_at").and_then(serde_json::Value::as_i64);
            r.reason = Some("notDue".into());
        }
        record(db, p, &r).await?;
        return Ok(());
    };
    r.plan_id = Some(plan.id);
    r.plan_name = plan.name;
    r.starts_at = None;
    r.ends_at = None;
    r.status = Status::Claimable;
    record(db.clone(), p, &r).await?;
    if preview_only {
        return Ok(());
    }
    let mut req = request(p, Endpoint::Claim);
    req.plan = r.plan_id.clone();
    submit_claim(
        &client,
        req,
        &mut r,
        &checked,
        |r| {
            let db = db.clone();
            async move { record(db, p, &r).await }
        },
        |cfg| {
            let db = db.clone();
            async move { await_captcha(app, db, p, lease, automatic, cfg).await }
        },
    )
    .await
}

// The official client sends no verification headers by default. A confirmed
// 3007 rejection alone admits the existing human verification path, once.
async fn submit_claim<T, P, PF, C, CF>(
    client: &Client<T>,
    mut req: Request,
    r: &mut Record,
    checked: &super::official::RequestCheck<'_>,
    mut persist: P,
    mut captcha: C,
) -> Result<(), &'static str>
where
    T: super::claim_protocol::Transport,
    P: FnMut(Record) -> PF,
    PF: std::future::Future<Output = Result<(), &'static str>> + Send,
    C: FnMut(CaptchaConfig) -> CF,
    CF: std::future::Future<Output = Result<(SecretValue, String), &'static str>> + Send,
{
    if !r.may_submit() {
        return Err("confirmResult");
    }
    let mut read = req.clone();
    read.plan = None;
    read.captcha = None;
    read.region = None;
    for attempt in 0..2 {
        checked().map_err(|_| "cancelled")?;
        r.mark_submitted();
        // Failure to durably publish pending prevents either POST.
        persist(r.clone()).await?;
        match client.request(req.clone(), checked).await {
            Ok(reply) => r.apply_reply(&reply),
            Err(OfficialError::Unauthorized) => r.status = Status::LoginExpired,
            Err(_) => {
                r.status = Status::ResultPending;
                r.reason = Some("confirmResult".into());
            }
        }
        if r.status == Status::VerificationRequired && attempt == 0 {
            persist(r.clone()).await?;
            read.endpoint = Endpoint::Config;
            let cfg = match client
                .request(read.clone(), checked)
                .await
                .and_then(|v| super::claim_protocol::captcha_config(&v))
            {
                Ok(cfg) if cfg.enabled => cfg,
                Err(OfficialError::Unauthorized) => {
                    r.status = Status::LoginExpired;
                    r.reason = Some("loginExpired".into());
                    return persist(r.clone()).await;
                }
                _ => {
                    r.reason = Some("captchaUnavailable".into());
                    return persist(r.clone()).await;
                }
            };
            match captcha(cfg).await {
                Ok((param, region)) => {
                    req.captcha = Some(param);
                    req.region = Some(region);
                }
                Err(_) => {
                    if checked().is_err() {
                        r.status = Status::Cancelled;
                    }
                    return persist(r.clone()).await;
                }
            }
            continue;
        }
        if r.status == Status::ResultPending {
            read.endpoint = Endpoint::Balance;
            if let Ok(balance) = client.request(read.clone(), checked).await.and_then(data) {
                r.reconcile(&balance);
            }
        }
        return persist(r.clone()).await;
    }
    unreachable!("the verified attempt always returns")
}

fn failure(r: &mut Record, error: OfficialError) {
    log::warn!("ZCode claim check failed: {error}");
    r.status = if error == OfficialError::Unauthorized {
        Status::LoginExpired
    } else if error == OfficialError::Cancelled {
        Status::Cancelled
    } else {
        Status::Unknown
    };
    r.reason = Some(
        match error {
            OfficialError::Timeout => "timeout",
            OfficialError::Unauthorized => "loginExpired",
            OfficialError::Cancelled => "cancelled",
            OfficialError::InvalidResponse => "invalidResponse",
            _ => "network",
        }
        .into(),
    );
}

pub(crate) async fn start(
    app: tauri::AppHandle,
    db: Arc<Database>,
    root: Option<PathBuf>,
    selected: Vec<String>,
    preview_only: bool,
) -> Result<View, &'static str> {
    if selected.is_empty()
        || selected.len() > 100
        || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
    {
        return Err("invalid");
    }
    let lease = jobs().begin()?;
    let ctx = context(root.as_deref())?;
    let current = state(db.clone(), Some(ctx.data_root().into())).await?;
    if selected.iter().any(|id| !current.records.contains_key(id)) {
        return Err("changed");
    }
    let task = round(
        app,
        db,
        Some(ctx.data_root().into()),
        selected,
        preview_only,
        false,
        lease,
    );
    #[cfg(feature = "test-hooks")]
    let task = test_support::scoped_round(task);
    tauri::async_runtime::spawn(task);
    Ok(current)
}
async fn round(
    app: tauri::AppHandle,
    db: Arc<Database>,
    root: Option<PathBuf>,
    selected: Vec<String>,
    preview_only: bool,
    automatic: bool,
    lease: Lease,
) {
    for id in selected {
        if lease.cancelled() {
            break;
        }
        match prepare(db.clone(), root.clone(), id.clone()).await {
            Ok(p) => {
                if one(&app, db.clone(), &p, &lease, preview_only, automatic)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(reason) => {
                let root = root.clone();
                let persisted = run_owned(db.clone(), move |db| {
                    #[cfg(feature = "test-hooks")]
                    test_support::preflight_worker();
                    let result = (|| {
                        let ctx = context(root.as_deref())?;
                        if !ids(db, &ctx)?.contains(&id) {
                            return Err("changed");
                        }
                        let mut saved = load(db)?;
                        let scope = saved.scopes.entry(ctx.context_id().into()).or_default();
                        scope.data_root = ctx.data_root().into();
                        scope
                            .records
                            .entry(id)
                            .or_default()
                            .preflight_failed(reason);
                        #[cfg(feature = "test-hooks")]
                        test_support::preflight_save()?;
                        save(db, &saved)
                    })();
                    Ok(result)
                })
                .await;
                #[cfg(feature = "test-hooks")]
                test_support::preflight_result(
                    persisted.is_err(),
                    matches!(&persisted, Ok(Err(_))),
                );
                // Continue only after the preflight result is durably saved.
                // Both task failure and a nested storage/context error stop admission.
                if !matches!(persisted, Ok(Ok(()))) {
                    break;
                }
            }
        }
    }
    let _ = app.emit("zcode-claim-changed", ());
}
pub(crate) fn shutdown() {
    jobs().shutdown();
}
pub(crate) async fn cancel(db: Arc<Database>, root: Option<PathBuf>) -> Result<View, &'static str> {
    jobs().cancel();
    state(db, root).await
}
pub(crate) async fn tick(app: tauri::AppHandle, db: Arc<Database>) {
    if jobs().busy() {
        return;
    }
    let scopes = run_owned(db.clone(), |db| {
        load(db)
            .map(|s| {
                s.scopes
                    .into_values()
                    .filter(|s| s.enabled && !s.participants.is_empty())
                    .collect::<Vec<_>>()
            })
            .map_err(|_| super::runtime::RuntimeError::TaskFailed)
    })
    .await;
    let Ok(scopes) = scopes else {
        return;
    };
    for scope in scopes {
        let Ok(lease) = jobs().begin() else {
            return;
        };
        round(
            app.clone(),
            db.clone(),
            Some(scope.data_root),
            scope.participants,
            false,
            true,
            lease,
        )
        .await;
    }
}

struct Pending {
    nonce: String,
    config: CaptchaConfig,
    sender: tokio::sync::oneshot::Sender<(SecretValue, String)>,
}
fn pending() -> &'static Mutex<Option<Pending>> {
    static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaptchaView {
    nonce: String,
    config: CaptchaConfig,
}
pub(crate) fn captcha_context() -> Result<CaptchaView, &'static str> {
    let pending = pending().lock().map_err(|_| "unavailable")?;
    let p = pending.as_ref().ok_or("cancelled")?;
    Ok(CaptchaView {
        nonce: p.nonce.clone(),
        config: p.config.clone(),
    })
}
pub(crate) fn submit(nonce: String, param: String) -> Result<(), &'static str> {
    let param = zeroize::Zeroizing::new(param);
    let mut pending = pending().lock().map_err(|_| "unavailable")?;
    if pending.as_ref().is_none_or(|p| p.nonce != nonce) {
        return Err("cancelled");
    }
    let param = SecretValue::new(&param).map_err(|_| "invalid")?;
    let p = pending.take().ok_or("cancelled")?;
    p.sender
        .send((param, p.config.region))
        .map_err(|_| "cancelled")
}
pub(crate) fn interactive(app: &tauri::AppHandle) -> Result<(), &'static str> {
    let w = app
        .get_webview_window("zcode-claim-captcha")
        .ok_or("cancelled")?;
    w.show().map_err(|_| "unavailable")?;
    let _ = w.set_focus();
    Ok(())
}
async fn await_captcha(
    app: &tauri::AppHandle,
    db: Arc<Database>,
    p: &Prepared,
    lease: &Lease,
    automatic: bool,
    config: CaptchaConfig,
) -> Result<(SecretValue, String), &'static str> {
    check(&db, p, lease, automatic).map_err(|_| "cancelled")?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let (sender, mut rx) = tokio::sync::oneshot::channel();
    *pending().lock().map_err(|_| "unavailable")? = Some(Pending {
        nonce,
        config,
        sender,
    });
    let w = tauri::WebviewWindowBuilder::new(
        app,
        "zcode-claim-captcha",
        tauri::WebviewUrl::App("zcode-claim-captcha.html".into()),
    )
    .title("ZCode verification")
    .inner_size(400., 340.)
    .resizable(false)
    .on_web_resource_request(|request, response| {
        if request.uri().path().ends_with("zcode-claim-captcha.html") {
            response.headers_mut().insert(
                "Content-Security-Policy",
                tauri::http::HeaderValue::from_static(CAPTCHA_CSP),
            );
        }
    })
    .visible(!automatic)
    .on_navigation(|url| {
        matches!(url.scheme(), "tauri" | "http" | "https")
            && matches!(url.host_str(), Some("tauri.localhost" | "localhost"))
            && url.path().ends_with("zcode-claim-captcha.html")
    })
    .build()
    .map_err(|_| "unavailable")?;
    w.on_window_event(|event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
            jobs().cancel();
        }
    });
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut guard = tokio::time::interval(Duration::from_millis(250));
    let result = loop {
        tokio::select! {result=&mut rx=>break result.map_err(|_|"cancelled"),_=&mut deadline=>break Err("verificationRequired"),_=guard.tick()=>{if check(&db,p,lease,automatic).is_err(){break Err("cancelled");}}}
    };
    *pending().lock().map_err(|_| "unavailable")? = None;
    let _ = w.destroy();
    result
}

// The SDK policy is scoped to this verification document, never the main window.
const CAPTCHA_CSP:&str="default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' https://o.alicdn.com https://*.alicdn.com https://*.aliyuncs.com; style-src 'self' 'unsafe-inline' https://*.alicdn.com https://*.aliyuncs.com; img-src 'self' data: blob: https:; font-src 'self' data: https://o.alicdn.com https://*.alicdn.com; connect-src ipc: http://ipc.localhost https://*.aliyuncs.com https://*.aliyun.com https://ynuf.aliapp.org https://o.alicdn.com https://*.alicdn.com; frame-src 'self' about: blob: https://*.aliyuncs.com https://*.aliyun.com https://*.alicdn.com; worker-src 'self' blob:; object-src 'none'";

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::super::{core::OAuthFamily, native::tests::native_document_with_context};
    use super::*;
    #[test]
    fn plan_discovery_preserves_independent_results_and_unknown_completeness() {
        let plan = super::super::claim::Plan {
            id: "synthetic-plan".into(),
            name: None,
            description: None,
            priority: 1,
            units: None,
            grants: vec![],
        };
        let (plans, complete) =
            merge_plan_sources(Ok(vec![plan.clone()]), Err(OfficialError::Timeout)).unwrap();
        assert_eq!(plans, vec![plan.clone()]);
        assert!(!complete);
        let (plans, complete) =
            merge_plan_sources(Err(OfficialError::Http(500)), Ok(vec![plan.clone()])).unwrap();
        assert_eq!(plans, vec![plan.clone()]);
        assert!(!complete);
        let (plans, complete) =
            merge_plan_sources(Ok(vec![]), Err(OfficialError::Transport)).unwrap();
        assert!(plans.is_empty() && !complete);
        let (plans, complete) =
            merge_plan_sources(Ok(vec![plan.clone()]), Ok(vec![plan.clone()])).unwrap();
        assert_eq!(plans.len(), 1);
        assert!(complete);
        for error in [
            OfficialError::Unauthorized,
            OfficialError::Cancelled,
            OfficialError::Http(429),
        ] {
            assert_eq!(
                merge_plan_sources(Ok(vec![plan.clone()]), Err(error)),
                Err(error)
            );
        }
    }

    #[test]
    fn invalid_balance_failure_is_persisted_without_erasing_the_prior_snapshot() {
        let db = Database::memory().unwrap();
        let mut record = Record {
            status: Status::Claimable,
            plan_id: Some("synthetic-plan".into()),
            prior_plan_ids: vec!["existing-plan".into()],
            ..Record::default()
        };
        failure(&mut record, OfficialError::InvalidResponse);
        let mut saved = Saved::default();
        saved
            .scopes
            .entry("synthetic-context".into())
            .or_default()
            .records
            .insert("account-a".into(), record);
        save(&db, &saved).unwrap();
        let restored = load(&db).unwrap();
        let record = &restored.scopes["synthetic-context"].records["account-a"];
        assert_eq!(record.status, Status::Unknown);
        assert_eq!(record.reason.as_deref(), Some("invalidResponse"));
        assert_eq!(record.prior_plan_ids, vec!["existing-plan"]);
        assert!(!record.may_submit());
    }

    #[test]
    fn default_auto_is_off_and_restart_keeps_encrypted_pending_results() {
        let db = Database::memory().unwrap();
        let ctx = "synthetic-context".to_string();
        let mut saved = Saved::default();
        let scope = saved.scopes.entry(ctx.clone()).or_default();
        assert!(!scope.enabled);
        assert!(scope.participants.is_empty());
        scope.data_root = super::super::synthetic_test_path("claim-data");
        scope.participants = vec!["account-a".into()];
        scope.enabled = true;
        let mut record = Record {
            status: Status::Claimable,
            plan_id: Some("synthetic-plan".into()),
            ..Record::default()
        };
        record.mark_submitted();
        scope.records.insert("account-a".into(), record);
        save(&db, &saved).unwrap();
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key=?1", [SETTING], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!raw.contains("synthetic-plan"));
        assert!(!raw.contains("account-a"));
        let restored = load(&db).unwrap();
        assert!(restored.scopes[&ctx].enabled);
        assert_eq!(
            restored.scopes[&ctx].records["account-a"].status,
            Status::ResultPending
        );
        assert!(!restored.scopes[&ctx].records["account-a"].may_submit());
        db.secret_session().set_blocked(true);
        assert!(load(&db).is_err());
    }
    #[test]
    fn deleted_accounts_disappear_from_results_and_auto_participation() {
        let scope = Scope {
            enabled: true,
            participants: vec!["deleted".into(), "kept".into()],
            records: BTreeMap::from([("deleted".into(), Record::default())]),
            ..Scope::default()
        };
        let out = view(scope, &BTreeSet::from(["kept".into()]));
        assert_eq!(out.participants, vec!["kept"]);
        assert!(!out.records.contains_key("deleted"));
    }
    #[test]
    #[cfg(unix)]
    fn live_admission_stops_on_lock_cancel_and_catalog_deletion() {
        use std::os::unix::fs::PermissionsExt;
        // The production store requires a canonical, private directory. macOS
        // temporary paths may contain the /var -> /private/var alias.
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let session = crate::secrets::session::SecretSession::from_context(
            directory.path().canonicalize().unwrap(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        let db =
            Database::from_connection(rusqlite::Connection::open_in_memory().unwrap(), session);
        let root = super::super::synthetic_test_path("native-data");
        let home = super::super::synthetic_test_path("home");
        let home = home.to_str().unwrap();
        let ctx = LibraryContext::from_os_identity(home, "synthetic-user", &root).unwrap();
        let native = ctx.cipher().unwrap();
        let secret = format!("zcode-credential-fallback:darwin:{home}:synthetic-user");
        let doc = native_document_with_context(
            ctx.context_id(),
            &secret,
            OAuthFamily::Zai,
            "account-a",
            "v1",
        );
        let snapshot = native.inspect(&doc).unwrap();
        let id = snapshot.identity().opaque_id();
        let (revision, storage) = {
            let vault = db.secret_session().read().unwrap();
            let store = VaultAccountStore::new(db.secret_session().root(), &vault).unwrap();
            let old = store.catalog_status(&native).unwrap();
            store
                .save_login_profile(
                    &native,
                    &old.revision,
                    &uuid::Uuid::new_v4().to_string(),
                    snapshot,
                    None,
                    false,
                )
                .unwrap();
            let revision = store.catalog_status(&native).unwrap().revision;
            (revision, vault.identity())
        };
        let p = Prepared {
            ctx,
            id,
            revision,
            storage,
            jwt: StartJwt::new("synthetic-jwt").unwrap(),
            version: "3.11.2".into(),
        };
        let owner = Jobs::default();
        let lease = owner.begin().unwrap();
        assert!(check(&db, &p, &lease, false).is_ok());
        db.secret_session().set_blocked(true);
        assert_eq!(check(&db, &p, &lease, false), Err(OfficialError::Cancelled));
        db.secret_session().set_blocked(false);
        std::fs::remove_file(
            db.secret_session()
                .root()
                .join(super::super::checkpoint::PROFILE_FILE),
        )
        .unwrap();
        assert_eq!(check(&db, &p, &lease, false), Err(OfficialError::Cancelled));
        owner.cancel();
        assert!(lease.cancelled());
    }
    #[test]
    fn unexpired_or_pending_plan_cannot_be_claimed_again() {
        let balance = serde_json::json!({"server_time":100,"plans":[{"plan_id":"plan","status":"active","starts_at":120,"ends_at":200}]});
        assert!(active_plan(&balance, "plan").is_some());
        let expired = serde_json::json!({"server_time":201,"plans":[{"plan_id":"plan","status":"active","ends_at":200}]});
        assert!(active_plan(&expired, "plan").is_none());
        assert_eq!(INTERVAL, Duration::from_secs(600));
    }
}

#[cfg(test)]
#[path = "claim_optional_tests.rs"]
mod optional_tests;

#[cfg(feature = "test-hooks")]
#[path = "claim_runtime_test_support.rs"]
pub(crate) mod test_support;
