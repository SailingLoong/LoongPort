//! Claim-specific synthetic assembly, compiled only with the existing test-hooks feature.
use super::super::official::{OfficialResponse, TransportFuture};
use super::super::{
    core::{AccountIdentity, CredentialDocument, OAuthFamily},
    transaction::LoginProfileSave,
};
use super::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) struct Script {
    ctx: LibraryContext,
    root: PathBuf,
    db: Arc<Database>,
    steps: Mutex<
        VecDeque<(
            &'static str,
            Endpoint,
            Result<serde_json::Value, OfficialError>,
        )>,
    >,
    trace: Mutex<Vec<String>>,
    events: Mutex<Vec<String>>,
    selected: Vec<String>,
    fault: &'static str,
    fired: AtomicUsize,
    finished: tokio::sync::Notify,
}
tokio::task_local! { static ACTIVE: Arc<Script>; }
pub(crate) fn current() -> Option<Arc<Script>> {
    ACTIVE.try_with(Arc::clone).ok()
}
impl Script {
    pub(crate) fn send(&self, request: Request) -> TransportFuture<'_> {
        let result = (|| {
            assert_ne!(
                request.endpoint,
                Endpoint::Claim,
                "synthetic suite forbids all POSTs"
            );
            let mut steps = self.steps.lock().unwrap();
            let (account, endpoint, response) = steps
                .pop_front()
                .expect("unexpected request; no live fallback");
            assert_eq!(request.endpoint, endpoint);
            assert_eq!(request.jwt.expose(), format!("synthetic-{account}-jwt"));
            if account == "b" && matches!(self.fault, "malformed" | "pending") {
                assert!(
                    self.events
                        .lock()
                        .unwrap()
                        .contains(&format!("saved:{}", self.selected[0])),
                    "A must be durably saved before B sends"
                );
            }
            self.trace
                .lock()
                .unwrap()
                .push(format!("{account}:{endpoint:?}"));
            response.map(|value| OfficialResponse {
                status: 200,
                body: zeroize::Zeroizing::new(serde_json::to_vec(&value).unwrap()),
            })
        })();
        Box::pin(async move { result })
    }
}

#[cfg(target_os = "macos")]
fn fixture(case: &'static str) -> (Arc<Script>, Vec<String>) {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("loongport-pr584-native-{}", std::process::id()));
    std::fs::create_dir(&base)
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(e)
            }
        })
        .unwrap();
    assert!(!std::fs::symlink_metadata(&base)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(base.canonicalize().unwrap(), base);
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.join(case);
    std::fs::create_dir(&root).expect("requires fresh case root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let ctx = LibraryContext::from_os_identity(
        base.to_str().unwrap(),
        "synthetic-user",
        &base.join("native-data"),
    )
    .unwrap();
    let session = crate::SecretSession::from_context(
        root.clone(),
        crate::secrets::VaultContext::generate().unwrap(),
    );
    // The original SQLite owner/schema and encrypted settings are used on disk.
    let db = Arc::new(Database::init_with_secrets(session).unwrap());
    let mut selected = Vec::new();
    {
        let native = ctx.cipher().unwrap();
        let vault = db.secret_session().read().unwrap();
        let store = VaultAccountStore::new(&root, &vault).unwrap();
        for account in ["a", "b"] {
            let identity =
                AccountIdentity::new(ctx.context_id(), OAuthFamily::Zai, account).unwrap();
            let mut data = BTreeMap::new();
            for (index, key) in identity.credential_keys().into_iter().enumerate() {
                if account == "a" && matches!(case, "outer" | "inner") && matches!(index, 4 | 6) {
                    continue;
                }
                let value = match index {
                    0 => "zai".to_owned(),
                    3 => serde_json::json!({"id":account,"username":account,"displayName":account})
                        .to_string(),
                    4 | 6 => format!("synthetic-{account}-jwt"),
                    _ => format!("synthetic-{account}-value"),
                };
                data.insert(key, native.encrypt(&value).unwrap());
            }
            let doc = CredentialDocument::parse(&serde_json::to_vec(&data).unwrap()).unwrap();
            let snapshot = native.inspect(&doc).unwrap();
            let id = snapshot.identity().opaque_id();
            let revision = store.catalog_status(&native).unwrap().revision;
            store
                .save_login_profile_for(
                    &native,
                    LoginProfileSave {
                        revision: &revision,
                        request_id: &uuid::Uuid::new_v4().to_string(),
                        snapshot,
                        evidence: None,
                        update_duplicate: false,
                        completion: None,
                    },
                )
                .unwrap();
            selected.push(id);
        }
    }
    let mut saved = Saved::default();
    let scope = saved.scopes.entry(ctx.context_id().into()).or_default();
    scope.data_root = ctx.data_root().into();
    scope.records.insert(
        selected[0].clone(),
        Record {
            status: if case == "pending" {
                Status::ResultPending
            } else {
                Status::Claimable
            },
            plan_id: Some("old-plan".into()),
            plan_name: Some("Synthetic old plan".into()),
            starts_at: Some(10),
            ends_at: Some(20),
            prior_plan_ids: vec!["old-plan".into()],
            ..Record::default()
        },
    );
    save(&db, &saved).unwrap();
    let mut steps = VecDeque::new();
    match case {
        "malformed" => steps.push_back((
            "a",
            Endpoint::Balance,
            Ok(serde_json::json!({"code":0,"data":{"plans":{}}})),
        )),
        "pending" => steps.push_back(("a", Endpoint::Balance, Err(OfficialError::Transport))),
        "outer" | "inner" => {}
        _ => unreachable!(),
    }
    if matches!(case, "malformed" | "pending") {
        steps.push_back((
            "b",
            Endpoint::Balance,
            Ok(serde_json::json!({"code":0,"data":{"plans":[]}})),
        ));
        steps.push_back((
            "b",
            Endpoint::Preview,
            Ok(serde_json::json!({"code":0,"data":{"plans":[]}})),
        ));
        steps.push_back((
            "b",
            Endpoint::Marketing,
            Ok(serde_json::json!({"code":0,"data":{"deliveries":[]}})),
        ));
    }
    (
        Arc::new(Script {
            ctx,
            root,
            db,
            steps: Mutex::new(steps),
            trace: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            selected: selected.clone(),
            fault: case,
            fired: AtomicUsize::new(0),
            finished: tokio::sync::Notify::new(),
        }),
        selected,
    )
}

#[cfg(target_os = "macos")]
pub(crate) async fn verify(app: tauri::AppHandle) {
    for case in ["malformed", "pending", "outer", "inner"] {
        let (script, selected) = fixture(case);
        if case == "malformed" {
            scope_negatives(script.clone()).await;
        }
        let before = serde_json::to_value(load(&script.db).unwrap()).unwrap();
        ACTIVE
            .scope(script.clone(), async {
                // start is the real Native queue entry, with an actual Wry handle.
                start(
                    app.clone(),
                    script.db.clone(),
                    Some(script.ctx.data_root().into()),
                    selected.clone(),
                    true,
                )
                .await
                .expect("synthetic start must admit both accounts");
                tokio::time::timeout(Duration::from_secs(20), script.finished.notified())
                    .await
                    .expect("real round completion");
                assert!(!jobs().busy(), "round must release its lease");
                assert!(
                    script.steps.lock().unwrap().is_empty(),
                    "complete exact request script"
                );
                let saved = load(&script.db).unwrap();
                let a = &saved.scopes[script.ctx.context_id()].records[&selected[0]];
                assert_eq!(a.prior_plan_ids, vec!["old-plan"]);
                assert_eq!(a.plan_id.as_deref(), Some("old-plan"));
                assert_eq!(a.plan_name.as_deref(), Some("Synthetic old plan"));
                assert_eq!((a.starts_at, a.ends_at), (Some(10), Some(20)));
                match case {
                    "malformed" => {
                        assert_eq!(a.status, Status::Unknown);
                        assert_eq!(a.reason.as_deref(), Some("invalidResponse"));
                    }
                    "pending" => {
                        assert_eq!(a.status, Status::ResultPending);
                        assert!(!a.may_submit());
                    }
                    "outer" | "inner" => {
                        assert_eq!(
                            serde_json::to_value(&saved).unwrap(),
                            before,
                            "entire persisted pre-fault state must remain unchanged"
                        );
                        assert_eq!(a.status, Status::Claimable);
                        assert_eq!(script.fired.load(Ordering::SeqCst), 1);
                        assert!(script.trace.lock().unwrap().is_empty());
                        assert!(
                            script
                                .events
                                .lock()
                                .unwrap()
                                .contains(&format!("result:{case}")),
                            "actual nested result layer"
                        );
                        assert!(
                            !script
                                .events
                                .lock()
                                .unwrap()
                                .contains(&format!("prepare:{}", selected[1])),
                            "B must not be prepared after persistence failure"
                        );
                    }
                    _ => unreachable!(),
                }
                if matches!(case, "malformed" | "pending") {
                    assert_eq!(
                        saved.scopes[script.ctx.context_id()].records[&selected[1]].status,
                        Status::NoClaim
                    );
                }
                let native_view = state(script.db.clone(), Some(script.ctx.data_root().into()))
                    .await
                    .unwrap();
                assert!(!native_view.busy);
                assert_eq!(native_view.records[&selected[0]].record.status, a.status);
                // Reopen the physical SQLite database through the existing owner.
                let session = crate::SecretSession::from_context(
                    script.root.clone(),
                    script.db.secret_session().read().unwrap().clone(),
                );
                let reopened = Database::init_with_secrets(session).unwrap();
                assert_eq!(
                    serde_json::to_value(load(&reopened).unwrap()).unwrap(),
                    serde_json::to_value(&saved).unwrap()
                );
                let raw = std::fs::read(script.root.join(crate::config::DB_FILE_NAME)).unwrap();
                assert!(!raw.windows(b"old-plan".len()).any(|w| w == b"old-plan"));
                println!(
                    "CLAIM_NATIVE_CASE_PASS {case} trace={:?} root={}",
                    script.trace.lock().unwrap(),
                    script.root.display()
                );
            })
            .await;
        if matches!(case, "outer" | "inner") {
            benign_after_fault(&app, &script, &selected[1], case).await;
        }
    }
}

pub(crate) fn sync_scope<T>(scope: Arc<Script>, work: impl FnOnce() -> T) -> T {
    ACTIVE.sync_scope(scope, work)
}
pub(crate) fn scoped_round(
    task: impl std::future::Future<Output = ()> + Send + 'static,
) -> impl std::future::Future<Output = ()> + Send {
    let scope = current();
    async move {
        match scope {
            Some(scope) => {
                ACTIVE.scope(scope.clone(), task).await;
                scope.finished.notify_one();
            }
            None => task.await,
        }
    }
}
impl Script {
    pub(crate) fn assert_db(&self, db: &Arc<Database>) {
        assert!(Arc::ptr_eq(&self.db, db), "fixture DB scope mismatch");
    }
    pub(crate) fn context(&self, root: Option<&std::path::Path>) -> LibraryContext {
        assert_eq!(
            root,
            Some(self.ctx.data_root()),
            "explicit synthetic context required"
        );
        self.ctx.clone()
    }
}
pub(crate) fn prepared(id: &str) {
    if let Some(scope) = current() {
        scope.events.lock().unwrap().push(format!("prepare:{id}"));
    }
}
pub(crate) fn persisted(id: &str) {
    if let Some(scope) = current() {
        scope.events.lock().unwrap().push(format!("saved:{id}"));
    }
}
pub(crate) fn preflight_worker() {
    if let Some(scope) = current() {
        if scope.fault == "outer" && scope.fired.fetch_add(1, Ordering::SeqCst) == 0 {
            // Outside the inner result, inside the real run_owned blocking worker.
            panic!("synthetic PR584 outer worker fault");
        }
    }
}
pub(crate) fn preflight_save() -> Result<(), &'static str> {
    if let Some(scope) = current() {
        if scope.fault == "inner" && scope.fired.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err("locked");
        }
    }
    Ok(())
}

pub(crate) fn preflight_result(outer: bool, inner: bool) {
    if let Some(scope) = current() {
        assert_ne!(
            outer, inner,
            "preflight failure must have exactly one error layer"
        );
        scope
            .events
            .lock()
            .unwrap()
            .push(format!("result:{}", if outer { "outer" } else { "inner" }));
    }
}

#[cfg(target_os = "macos")]
fn separate_script(
    source: &Script,
    fault: &'static str,
    steps: VecDeque<(
        &'static str,
        Endpoint,
        Result<serde_json::Value, OfficialError>,
    )>,
) -> Arc<Script> {
    Arc::new(Script {
        ctx: source.ctx.clone(),
        root: source.root.clone(),
        db: source.db.clone(),
        steps: Mutex::new(steps),
        trace: Mutex::new(Vec::new()),
        events: Mutex::new(Vec::new()),
        selected: source.selected.clone(),
        fault,
        fired: AtomicUsize::new(0),
        finished: tokio::sync::Notify::new(),
    })
}
#[cfg(target_os = "macos")]
async fn scope_negatives(source: Arc<Script>) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    assert!(current().is_none());
    ACTIVE
        .scope(source.clone(), async {
            assert!(
                tauri::async_runtime::spawn(async { current().is_none() })
                    .await
                    .unwrap(),
                "scopes never implicitly leak to other tasks"
            );
            assert!(catch_unwind(AssertUnwindSafe(
                || source.context(Some(std::path::Path::new("/private/tmp/wrong-claim-scope")))
            ))
            .is_err());
        })
        .await;
    assert!(current().is_none());
    for endpoint in [Endpoint::Claim, Endpoint::Preview] {
        let negative = separate_script(&source, "negative", VecDeque::new());
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _response = negative.send(Request {
                    endpoint,
                    version: "3.11.2".into(),
                    jwt: StartJwt::new("synthetic-a-jwt").unwrap(),
                    plan: None,
                    captcha: None,
                    region: None,
                });
            }))
            .is_err(),
            "POST and unexpected request must fail before any live fallback"
        );
    }
    println!("CLAIM_NATIVE_SCOPE_NEGATIVES_PASS");
}
#[cfg(target_os = "macos")]
async fn benign_after_fault(app: &tauri::AppHandle, source: &Script, id: &str, case: &str) {
    let followup = separate_script(
        source,
        "benign",
        VecDeque::from([
            (
                "b",
                Endpoint::Balance,
                Ok(serde_json::json!({"code":0,"data":{"plans":[]}})),
            ),
            (
                "b",
                Endpoint::Preview,
                Ok(serde_json::json!({"code":0,"data":{"plans":[]}})),
            ),
            (
                "b",
                Endpoint::Marketing,
                Ok(serde_json::json!({"code":0,"data":{"deliveries":[]}})),
            ),
        ]),
    );
    ACTIVE
        .scope(followup.clone(), async {
            start(
                app.clone(),
                followup.db.clone(),
                Some(followup.ctx.data_root().into()),
                vec![id.into()],
                true,
            )
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(20), followup.finished.notified())
                .await
                .unwrap();
            assert!(!jobs().busy());
            assert!(followup.steps.lock().unwrap().is_empty());
            assert_eq!(
                load(&followup.db).unwrap().scopes[followup.ctx.context_id()].records[id].status,
                Status::NoClaim
            );
            assert_eq!(followup.fired.load(Ordering::SeqCst), 0);
            println!(
                "CLAIM_NATIVE_BENIGN_PASS {case} trace={:?}",
                followup.trace.lock().unwrap()
            );
        })
        .await;
}
