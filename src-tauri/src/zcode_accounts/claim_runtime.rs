//! Vault-owned serial claims. Adapted queue behavior from pjpv/zcode-switch
//! f34225686dfef05d84c256a56f868719248f15ff (MIT, licenses/zcode-switch-MIT.txt).
use super::{
    claim::{Record, Status},
    claim_protocol::{CaptchaConfig, Client, Endpoint, Http, Request},
    claim_queue::{Jobs, Lease},
    library_context::LibraryContext,
    official::{OfficialError, SecretValue, StartJwt},
    runtime::run_owned,
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
    vault_id: String,
    key_id: String,
    vault_revision: u64,
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
            let version = super::native_context::library_app_version(ctx.data_root())
                .ok_or("appVersionUnknown")?;
            let meta = vault.metadata();
            Ok(Prepared {
                ctx,
                id,
                revision: status.revision,
                vault_id: meta.vault_id.clone(),
                key_id: meta.key_id.clone(),
                vault_revision: meta.revision,
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
    let meta = vault.metadata();
    if meta.vault_id != p.vault_id || meta.key_id != p.key_id || meta.revision != p.vault_revision {
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
            scope.records.insert(id, record);
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
async fn one(
    app: &tauri::AppHandle,
    db: Arc<Database>,
    p: &Prepared,
    lease: &Lease,
    preview_only: bool,
    automatic: bool,
) -> Result<(), &'static str> {
    let client = Client(Http::new().map_err(|_| "network")?);
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
    let raw = match client
        .request(request(p, Endpoint::Preview), &checked)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            failure(&mut r, error);
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let plans = match super::claim::preview(&raw) {
        Ok(plans) => plans,
        Err(_) => {
            if raw.get("code").and_then(serde_json::Value::as_i64) == Some(401) {
                r.status = Status::LoginExpired;
            } else {
                r.status = Status::Unknown;
                r.reason = Some("previewUnavailable".into());
            }
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let plan = plans
        .iter()
        .find(|plan| active_plan(&balance, &plan.id).is_none())
        .cloned();
    r.plans = plans;
    r.prior_plan_ids = balance
        .get("plans")
        .and_then(serde_json::Value::as_array)
        .ok_or("invalidResponse")?
        .iter()
        .filter_map(|p| {
            p.get("user_plan_id")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
        .collect();
    r.reason = None;
    let Some(plan) = plan else {
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
    let cfg = match client
        .request(request(p, Endpoint::Config), &checked)
        .await
        .and_then(|value| super::claim_protocol::captcha_config(&value))
    {
        Ok(cfg) if cfg.enabled => cfg,
        Err(OfficialError::Unauthorized) => {
            r.status = Status::LoginExpired;
            r.reason = Some("loginExpired".into());
            record(db, p, &r).await?;
            return Ok(());
        }
        _ => {
            r.status = Status::VerificationRequired;
            r.reason = Some("captchaUnavailable".into());
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    let input = await_captcha(app, db.clone(), p, lease, automatic, cfg).await;
    let (param, region) = match input {
        Ok(input) => input,
        Err(_) => {
            r.status = if lease.cancelled() {
                Status::Cancelled
            } else {
                Status::VerificationRequired
            };
            record(db, p, &r).await?;
            return Ok(());
        }
    };
    checked().map_err(|_| "cancelled")?;
    // Publish before the only POST. Failure to save prevents sending the request.
    r.mark_submitted();
    record(db.clone(), p, &r).await?;
    let mut req = request(p, Endpoint::Claim);
    req.plan = r.plan_id.clone();
    req.captcha = Some(param);
    req.region = Some(region);
    match client.request(req, &checked).await {
        Ok(reply) => r.apply_reply(&reply),
        Err(OfficialError::Unauthorized) => r.status = Status::LoginExpired,
        Err(_) => {
            r.status = Status::ResultPending;
            r.reason = Some("confirmResult".into());
        }
    }
    if r.status == Status::ResultPending {
        if let Ok(value) = client
            .request(request(p, Endpoint::Balance), &checked)
            .await
            .and_then(data)
        {
            r.reconcile(&value);
        }
    }
    record(db, p, &r).await
}
fn failure(r: &mut Record, error: OfficialError) {
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
    tauri::async_runtime::spawn(round(
        app,
        db,
        Some(ctx.data_root().into()),
        selected,
        preview_only,
        false,
        lease,
    ));
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
                let _ = run_owned(db.clone(), move |db| {
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
                        save(db, &saved)
                    })();
                    Ok(result)
                })
                .await;
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
        let (revision, vault_id, key_id, vault_revision) = {
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
            let m = vault.metadata();
            (revision, m.vault_id.clone(), m.key_id.clone(), m.revision)
        };
        let p = Prepared {
            ctx,
            id,
            revision,
            vault_id,
            key_id,
            vault_revision,
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
