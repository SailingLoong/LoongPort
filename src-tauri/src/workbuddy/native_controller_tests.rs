//! Vertical production functions and actual temporary files; source/stop proof is explicit fixture port only.
use super::*;
use crate::workbuddy::{
    native_source::{Observation, Receipt, SelectionKey},
    native_transaction::{FileIo, Files},
    store,
};
use crate::{
    app_store::isolated_scope::{capture_selection_at, SelectionSnapshot},
    secrets::{session::SecretSession, storage::StorageIdentity, VaultContext},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use zeroize::Zeroizing;
struct Fixture {
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
    selection: SelectionSnapshot,
    client: PathBuf,
    context: NativeContext,
    receipt: Receipt,
}
fn image(uid: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"account":{"uid":uid,"type":"personal","domain":"www.workbuddy.cn"},"auth":{"domain":"www.workbuddy.cn","accessToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":format!("opaque-{uid}")}}},"accounts":[{"uid":uid,"type":"personal","domain":"www.workbuddy.cn"}],"allAccounts":[{"uid":uid,"type":"personal","domain":"www.workbuddy.cn"}],"preserve":{"original":uid,"setting":17}})).unwrap()
}
impl Fixture {
    async fn new() -> Self {
        let root = crate::secrets::testing::tempdir().unwrap();
        let data = root.path().join("data");
        let device = root.path().join("device");
        let control = root.path().join("control");
        let client = root.path().join("native");
        for p in [&data, &device, &control, &client] {
            std::fs::create_dir(p).unwrap();
        }
        let selection =
            capture_selection_at(&control.join("app_paths.json"), &data, &device).unwrap();
        let session = SecretSession::from_context(data, VaultContext::generate().unwrap());
        let context = NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "explicit-fixture-install-profile-key-context".into(),
        };
        std::fs::write(client.join("workbuddy-desktop.info"), image("B")).unwrap();
        let obs = Observation::read(&client.join("workbuddy-desktop.info"), &context).unwrap();
        let binding = store::binding(&session).unwrap();
        let catalog = Catalog {
            session: &session,
            identity: &binding,
            epoch: session.lifecycle_epoch(),
            selection: &selection,
        };
        let receipt = catalog.save(&obs, None, "B").await.unwrap();
        std::fs::write(client.join("workbuddy-desktop.info"), image("A")).unwrap();
        std::fs::write(
            client.join("workbuddy-desktop.info.logged-out"),
            b"original-logout-marker",
        )
        .unwrap();
        Self {
            _root: root,
            session,
            selection,
            client,
            context,
            receipt,
        }
    }
    fn key(&self) -> SelectionKey {
        self.receipt.selection_key().unwrap()
    }
    fn binding(&self) -> StorageIdentity {
        store::binding(&self.session).unwrap()
    }
    fn controller<'a>(&'a self, id: &'a StorageIdentity) -> Controller<'a> {
        Controller {
            catalog: Catalog {
                session: &self.session,
                identity: id,
                epoch: self.session.lifecycle_epoch(),
                selection: &self.selection,
            },
        }
    }
    fn port(&self) -> FixtureSource {
        FixtureSource {
            root: self.client.clone(),
            root_pin: std::fs::File::open(&self.client).unwrap(),
            context: self.context.clone(),
            expected: self.context.clone(),
            stopped: Rc::new(Cell::new(true)),
            verified: true,
        }
    }
    fn auth(&self) -> PathBuf {
        self.client.join("workbuddy-desktop.info")
    }
}
struct FixtureSource {
    root: PathBuf,
    root_pin: std::fs::File,
    context: NativeContext,
    expected: NativeContext,
    stopped: Rc<Cell<bool>>,
    verified: bool,
}
impl sealed::Sealed for FixtureSource {}
impl ControlledSource for FixtureSource {
    fn verify_unchanged(&mut self) -> Result<(), Failure> {
        self.verify_stopped()
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn context(&self) -> &NativeContext {
        &self.context
    }
    fn verify_stopped(&mut self) -> Result<(), Failure> {
        use std::os::unix::fs::MetadataExt;
        let expected = self
            .root_pin
            .metadata()
            .map_err(|_| Failure::SourceChanged)?;
        let actual = std::fs::symlink_metadata(&self.root).map_err(|_| Failure::SourceChanged)?;
        if !actual.is_dir()
            || actual.file_type().is_symlink()
            || (actual.dev(), actual.ino(), actual.uid())
                != (expected.dev(), expected.ino(), expected.uid())
            || !self.verified
            || !self.stopped.get()
            || self.context != self.expected
        {
            Err(Failure::SourceChanged)
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn reopened_owner_selects_persistent_target_and_applies_original_four_blocks() {
    let f = Fixture::new().await;
    let original = std::fs::read(f.auth()).unwrap();
    let reopened = {
        let v = f.session.read().unwrap();
        match &*v {
            crate::secrets::storage::StorageContext::Encrypted(v) => {
                VaultContext::from_key(v.metadata().clone(), v.export_key()).unwrap()
            }
            _ => unreachable!(),
        }
    };
    let session = SecretSession::from_context(f.session.root().into(), reopened);
    let b = store::binding(&session).unwrap();
    let c = Controller {
        catalog: Catalog {
            session: &session,
            identity: &b,
            epoch: session.lifecycle_epoch(),
            selection: &f.selection,
        },
    };
    let selected = c
        .select(
            &serde_json::from_slice::<SelectionKey>(&serde_json::to_vec(&f.key()).unwrap())
                .unwrap(),
        )
        .await
        .expect("persistent selection after reopen, no old in-memory receipt");
    let mut port = f.port();
    let mut io = Files;
    let plan = c.prepare(&selected, &mut port, &mut io).await.unwrap();
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
    assert!(matches!(
        c.apply(&plan, &mut port, &mut io).await,
        Ok(Outcome::Applied)
    ));
    let after: Value = serde_json::from_slice(&std::fs::read(f.auth()).unwrap()).unwrap();
    let before: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(after["account"]["uid"], "B");
    assert_eq!(
        after["auth"]["accessToken"]["envelope"]["wrapped"],
        "opaque-B"
    );
    assert_eq!(after["preserve"], before["preserve"]);
    assert_eq!(after["accounts"], after["allAccounts"]);
    assert_eq!(after["accounts"].as_array().unwrap().len(), 2);
    assert!(after["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["uid"] == "A"));
    assert!(after["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["uid"] == "B"));
    assert!(!f.client.join("workbuddy-desktop.info.logged-out").exists());
    assert!(matches!(
        c.apply(&plan, &mut port, &mut io).await,
        Ok(Outcome::AlreadyApplied)
    ));
}
struct Interrupted {
    files: Files,
    stopped: Rc<Cell<bool>>,
    written: bool,
}
impl FileIo for Interrupted {
    fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        self.files.read(p)
    }
    fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
        self.files.write(p, b)?;
        if !self.written && p.file_name().unwrap() == "workbuddy-desktop.info" {
            self.written = true;
            self.stopped.set(false);
        }
        Ok(())
    }
    fn remove(&mut self, p: &Path) -> Result<(), Failure> {
        self.files.remove(p)
    }
}
#[tokio::test]
async fn interrupted_real_write_reopens_and_uses_original_recovery() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let original = std::fs::read(f.auth()).unwrap();
    let selected = c.select(&f.key()).await.unwrap();
    let mut port = f.port();
    let plan = c.prepare(&selected, &mut port, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: port.stopped.clone(),
        written: false,
    };
    assert!(matches!(
        c.apply(&plan, &mut port, &mut io).await,
        Err(Failure::RecoveryRequired)
    ));
    assert_ne!(std::fs::read(f.auth()).unwrap(), original);
    assert_eq!(
        crate::workbuddy::native_owner::inspect(f.session.root(), &f.session.read().unwrap())
            .unwrap(),
        crate::workbuddy::native_owner::Disposition::Pending
    );
    let reopened = {
        let v = f.session.read().unwrap();
        match &*v {
            crate::secrets::storage::StorageContext::Encrypted(v) => {
                VaultContext::from_key(v.metadata().clone(), v.export_key()).unwrap()
            }
            _ => unreachable!(),
        }
    };
    let session = SecretSession::from_context(f.session.root().into(), reopened);
    let binding = store::binding(&session).unwrap();
    let restored = Controller {
        catalog: Catalog {
            session: &session,
            identity: &binding,
            epoch: session.lifecycle_epoch(),
            selection: &f.selection,
        },
    };
    let mut source = f.port();
    assert!(matches!(
        restored.recover(&mut source, &mut Files).await,
        Ok(Outcome::Restored)
    ));
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
    assert_eq!(
        std::fs::read(f.client.join("workbuddy-desktop.info.logged-out")).unwrap(),
        b"original-logout-marker"
    );
    assert_eq!(
        crate::workbuddy::native_owner::inspect(session.root(), &session.read().unwrap()).unwrap(),
        crate::workbuddy::native_owner::Disposition::Clear
    );
}
#[tokio::test]
async fn old_selection_refuses_same_image_deleted_and_recreated_lifetime() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let old = c.select(&f.key()).await.unwrap();
    store::save(&f.session, &b, vec![]).unwrap();
    let original = std::fs::read(f.auth()).unwrap();
    let marker = f.client.join("workbuddy-desktop.info.logged-out");
    std::fs::remove_file(&marker).unwrap();
    std::fs::write(f.auth(), image("B")).unwrap();
    let obs = Observation::read(&f.auth(), &f.context).unwrap();
    let fresh = c.catalog.save(&obs, None, "B again").await.unwrap();
    std::fs::write(f.auth(), &original).unwrap();
    std::fs::write(&marker, b"original-logout-marker").unwrap();
    assert_eq!(fresh.id, f.receipt.id);
    assert_eq!(fresh.generation, 0);
    let mut source = f.port();
    assert!(c.prepare(&old, &mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
    let current = c.select(&fresh.selection_key().unwrap()).await.unwrap();
    let plan = c.prepare(&current, &mut source, &mut Files).await.unwrap();
    assert!(matches!(
        c.apply(&plan, &mut source, &mut Files).await,
        Ok(Outcome::Applied)
    ));
}
#[tokio::test]
async fn proof_stopped_and_key_context_are_required_before_any_native_write() {
    for case in ["source", "stopped", "context"] {
        let f = Fixture::new().await;
        let b = f.binding();
        let c = f.controller(&b);
        let selected = c.select(&f.key()).await.unwrap();
        let original = std::fs::read(f.auth()).unwrap();
        let mut source = f.port();
        match case {
            "source" => source.verified = false,
            "stopped" => source.stopped.set(false),
            _ => source.context.source = "unverified-other-key-context".into(),
        };
        assert!(c.prepare(&selected, &mut source, &mut Files).await.is_err());
        assert_eq!(std::fs::read(f.auth()).unwrap(), original);
        assert!(!f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
    }
}

#[tokio::test]
async fn prepared_plan_rechecks_target_and_original_lifecycle_before_apply() {
    for case in [
        "generation",
        "incarnation",
        "credential",
        "epoch",
        "selector",
        "stopped",
        "key-context",
        "recovery-intent",
        "root",
    ] {
        let f = Fixture::new().await;
        let b = f.binding();
        let c = f.controller(&b);
        let selected = c.select(&f.key()).await.unwrap();
        let mut source = f.port();
        let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
        let original = std::fs::read(f.auth()).unwrap();
        let journal = f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        let journal_before = std::fs::read(&journal).unwrap();
        match case {
            "generation" => {
                let mut rows = store::load(&f.session, &b).unwrap();
                rows[0].generation += 1;
                store::save(&f.session, &b, rows).unwrap();
            }
            "incarnation" => {
                let mut value = serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap();
                value[0]["account"]["native"]["incarnation"] =
                    json!("33333333-3333-4333-8333-333333333333");
                store::save(&f.session, &b, serde_json::from_value(value).unwrap()).unwrap();
            }
            "credential" => {
                use base64::Engine;
                let mut value = serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap();
                let encoded = value[0]["account"]["native"]["snapshot"].as_str().unwrap();
                let mut bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap();
                bytes.push(b'\n');
                value[0]["account"]["native"]["snapshot"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                store::save(&f.session, &b, serde_json::from_value(value).unwrap()).unwrap();
            }
            "epoch" => f.session.invalidate_database_leases(),
            "selector" => std::fs::write(
                f._root.path().join("control/app_paths.json"),
                b"untrusted selector replacement",
            )
            .unwrap(),
            "stopped" => source.stopped.set(false),
            "key-context" => source.context.source = "other-key-context".into(),
            "recovery-intent" => std::fs::write(
                f.session.root().join(".vault-rewrap"),
                b"competing lifecycle owner",
            )
            .unwrap(),
            _ => {
                std::fs::rename(&f.client, f._root.path().join("previous-native")).unwrap();
                std::fs::create_dir(&f.client).unwrap();
                std::fs::write(f.auth(), &original).unwrap();
                std::fs::write(
                    f.client.join("workbuddy-desktop.info.logged-out"),
                    b"original-logout-marker",
                )
                .unwrap();
            }
        }
        assert!(
            c.apply(&plan, &mut source, &mut Files).await.is_err(),
            "accepted changed {case}"
        );
        assert_eq!(std::fs::read(f.auth()).unwrap(), original);
        assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
    }
}

#[tokio::test]
async fn physical_source_replacement_and_invalid_native_selection_do_not_initialize_journal() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let mut wrong = f.key();
    wrong.generation = 1;
    assert!(c.select(&wrong).await.is_err());
    wrong = f.key();
    wrong.id = "unknown-account".into();
    assert!(c.select(&wrong).await.is_err());
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let original = std::fs::read(f.auth()).unwrap();
    std::fs::rename(&f.client, f._root.path().join("old-source")).unwrap();
    std::fs::create_dir(&f.client).unwrap();
    std::fs::write(f.auth(), &original).unwrap();
    assert!(c.prepare(&selected, &mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}

#[tokio::test]
async fn persistent_reselection_rejects_same_image_deleted_and_recreated_lifetime() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    // Persist the exact choice before deletion, rather than retaining Selection.
    let old_row = serde_json::to_value(store::load(&f.session, &b).unwrap()).unwrap();
    let persisted = serde_json::to_vec(&json!({
        "id":f.receipt.id,"generation":f.receipt.generation,
        "incarnation":old_row[0]["account"]["native"]["incarnation"]
    }))
    .unwrap();
    store::save(&f.session, &b, vec![]).unwrap();
    let original = std::fs::read(f.auth()).unwrap();
    let marker = f.client.join("workbuddy-desktop.info.logged-out");
    std::fs::remove_file(&marker).unwrap();
    std::fs::write(f.auth(), image("B")).unwrap();
    let obs = Observation::read(&f.auth(), &f.context).unwrap();
    let fresh = c
        .catalog
        .save(&obs, None, "same image new lifetime")
        .await
        .unwrap();
    std::fs::write(f.auth(), &original).unwrap();
    std::fs::write(&marker, b"original-logout-marker").unwrap();
    assert_eq!(fresh.id, f.receipt.id);
    assert_eq!(fresh.generation, 0);
    let disk_before = std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap();
    let old: SelectionKey = serde_json::from_slice(&persisted).unwrap();
    assert!(
        c.select(&old).await.is_err(),
        "stale persisted choice reselected a new lifetime with the same ID and generation"
    );
    assert!(c.catalog.select(&old).await.is_err());
    assert_ne!(old.incarnation, fresh.selection_key().unwrap().incarnation);
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
    assert_eq!(
        std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap(),
        disk_before
    );
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
    let current = c.select(&fresh.selection_key().unwrap()).await.unwrap();
    let mut source = f.port();
    let plan = c.prepare(&current, &mut source, &mut Files).await.unwrap();
    assert!(matches!(
        c.apply(&plan, &mut source, &mut Files).await,
        Ok(Outcome::Applied)
    ));
}

#[tokio::test]
async fn incomplete_persistent_choices_never_fall_back_to_id_and_generation() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let key = serde_json::to_value(f.key()).unwrap();
    let disk = f.session.root().join("workbuddy_accounts.json");
    let before = std::fs::read(&disk).unwrap();
    for case in ["missing", "null", "invalid", "unknown-version"] {
        let mut invalid = key.clone();
        match case {
            "missing" => {
                invalid.as_object_mut().unwrap().remove("incarnation");
            }
            "null" => invalid["incarnation"] = Value::Null,
            "invalid" => invalid["incarnation"] = json!("not-a-row-version"),
            _ => invalid["version"] = json!(9),
        }
        assert!(
            serde_json::from_value::<SelectionKey>(invalid).is_err(),
            "accepted {case}"
        );
    }
    let mut nonexistent = f.key();
    nonexistent.incarnation = uuid::Uuid::nil();
    assert!(c.select(&nonexistent).await.is_err());
    assert_eq!(std::fs::read(&disk).unwrap(), before);
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
    let wire = serde_json::to_string(&f.key()).unwrap();
    assert!(!wire.contains("opaque-B"));
    assert!(!wire.contains(f.client.to_str().unwrap()));
    assert!(!wire.contains("credential"));
}

#[path = "native_controller/process_tests.rs"]
mod process_tests;

#[path = "native_controller/install_tests.rs"]
mod install_tests;
