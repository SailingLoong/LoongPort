//! Synthetic private owner fixtures; no official key provenance or account consumption proof.
use super::{model::*, store};
use crate::secrets::{session::SecretSession, VaultContext};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
struct Fixture {
    root: tempfile::TempDir,
    session: std::sync::Arc<SecretSession>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        Self { root, session }
    }
    fn put(&self, rows: Vec<Value>) {
        let file = crate::secrets::owned_file::OwnedFile::registered(store::FILE).unwrap();
        let v = self.session.read().unwrap();
        let bytes = file
            .encode_storage(&v, &serde_json::to_vec(&json!({"accounts":rows})).unwrap())
            .unwrap();
        std::fs::write(self.root.path().join(store::FILE), bytes).unwrap();
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.root.path().join(store::FILE)).unwrap()
    }
}
fn api() -> SavedAccount {
    SavedAccount::new(Account {
        uid: "synthetic-api".into(),
        domain: Domain::WorkBuddy,
        label: "API fixture".into(),
        credential: "synthetic-api-secret".into(),
    })
}
fn native() -> Value {
    let mut row = serde_json::to_value(api()).unwrap();
    let bytes=br#"{"account":{"uid":"synthetic-native","type":"personal"},"auth":{"domain":"www.workbuddy.cn","accessToken":{"$wbEncrypted":1,"envelope":{"v":1,"wrapped":"synthetic-opaque"}}},"accounts":[],"allAccounts":[]}"#;
    row["account"] = json!({"uid":"synthetic-native","domain":"workBuddy","label":"Native fixture","native":{"version":1,"context":{"product":"workbuddy","region":"cn","realm":"www.workbuddy.cn","source":"synthetic-caller-bound-fixture-not-authentication"},"snapshot":STANDARD.encode(bytes)}});
    row
}
#[test]
fn native_owner_encrypted_mixed_roundtrip_and_original_delete() {
    let f = Fixture::new();
    let old = serde_json::to_value(api()).unwrap();
    f.put(vec![old.clone(), native()]);
    let binding = store::binding(&f.session).unwrap();
    let mut rows =
        store::load(&f.session, &binding).expect("native-only must load through original owner");
    assert_eq!(rows.len(), 2);
    assert_eq!(serde_json::to_value(&rows[0]).unwrap(), old);
    let native_id = rows[1].account.id(binding.store_id());

    store::save(&f.session, &binding, rows.clone()).unwrap();
    let reopened = store::load(&f.session, &binding).unwrap();
    assert_eq!(native_id, reopened[1].account.id(binding.store_id()));
    assert_eq!(serde_json::to_value(&reopened[1]).unwrap(), native());
    rows.retain(|r| r.account.id(binding.store_id()) != native_id);
    store::save(&f.session, &binding, rows).unwrap();
    assert_eq!(store::load(&f.session, &binding).unwrap().len(), 1);
    assert!(!String::from_utf8_lossy(&f.bytes()).contains("synthetic-opaque"));
}
#[test]
fn native_owner_dto_grants_no_http_actions_or_secret() {
    let f = Fixture::new();
    f.put(vec![native()]);
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).expect("native row");
    let view = serde_json::to_value(rows[0].view(b.store_id(), "2026-10-29")).unwrap();
    assert_eq!(view["canRefresh"], false);
    assert_eq!(view["canClaim"], false);
    assert_eq!(view["canSetAutomatic"], false);
    let text = serde_json::to_string(&view).unwrap();
    assert!(
        !text.contains("synthetic-opaque")
            && !text.contains("snapshot")
            && !text.contains("source")
    );
}
#[test]
fn native_owner_rejects_unknown_missing_or_ambiguous_version_without_write() {
    for case in [
        "unknown",
        "missing",
        "both",
        "none",
        "bad_snapshot",
        "foreign_uid",
        "empty_api_promotion",
        "null_token",
        "null_native",
        "unknown_account_version",
    ] {
        let f = Fixture::new();
        let mut row = native();
        match case {
            "unknown" => row["account"]["native"]["version"] = json!(99),
            "missing" => {
                row["account"]["native"]
                    .as_object_mut()
                    .unwrap()
                    .remove("version");
            }
            "both" => row["account"]["token"] = json!("synthetic-api-secret"),
            "none" => {
                row["account"].as_object_mut().unwrap().remove("native");
            }
            "bad_snapshot" => row["account"]["native"]["snapshot"] = json!("not base64"),
            "empty_api_promotion" => {
                row["account"]["token"] = json!("");
                row["account"]["credential_version"] = json!(1);
            }
            "null_token" => row["account"]["token"] = Value::Null,
            "null_native" => row["account"]["native"] = Value::Null,
            "unknown_account_version" => row["account"]["credential_version"] = json!(99),
            _ => row["account"]["uid"] = json!("foreign"),
        };
        f.put(vec![row]);
        let before = f.bytes();
        let b = store::binding(&f.session).unwrap();
        assert!(store::load(&f.session, &b).is_err(), "accepted {case}");
        assert!(f.bytes() == before);
    }
}
#[test]
fn native_owner_legacy_api_wire_remains_lossless() {
    let f = Fixture::new();
    let old = serde_json::to_value(api()).unwrap();
    f.put(vec![old.clone()]);
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).unwrap();
    assert_eq!(serde_json::to_value(&rows[0]).unwrap(), old);
    store::save(&f.session, &b, rows).unwrap();
    assert_eq!(
        serde_json::to_value(&store::load(&f.session, &b).unwrap()[0]).unwrap(),
        old
    );
}
#[test]
fn native_owner_duplicate_identity_is_not_silently_persisted() {
    for variant in [
        "native-native",
        "api-native",
        "native-api",
        "hybrid-api",
        "api-hybrid",
    ] {
        let f = Fixture::new();
        let b = store::binding(&f.session).unwrap();
        let n = native();
        let mut a = serde_json::to_value(api()).unwrap();
        a["account"]["uid"] = n["account"]["uid"].clone();
        let mut hybrid = n.clone();
        hybrid["account"]["token"] = json!("synthetic-api-secret");
        hybrid["account"]["credential_version"] = json!(1);
        let pair = match variant {
            "native-native" => vec![n.clone(), n],
            "api-native" => vec![a, n],
            "native-api" => vec![n, a],
            "hybrid-api" => vec![hybrid, a],
            _ => vec![a, hybrid],
        };
        store::save(&f.session, &b, vec![api()]).unwrap();
        let before = f.bytes();
        let rows = serde_json::from_value(json!(pair)).unwrap();
        assert!(
            store::save(&f.session, &b, rows).is_err(),
            "accepted {variant}"
        );
        assert_eq!(f.bytes(), before);
        f.put(pair);
        let before = f.bytes();
        assert!(store::load(&f.session, &b).is_err(), "loaded {variant}");
        assert!(store::save(&f.session, &b, vec![]).is_err());
        assert_eq!(f.bytes(), before);
    }
}

#[tokio::test]
async fn native_owner_http_paths_refuse_before_transport_or_mutation() {
    use super::protocol::{Endpoint, Transport};
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct NeverHttp(AtomicUsize);
    impl Transport for NeverHttp {
        fn request<'a>(
            &'a self,
            _: Endpoint,
            _: Option<&'a Account>,
            _: Option<&'a str>,
            _: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(Failure::Unconfirmed) })
        }
    }
    let f = Fixture::new();
    f.put(vec![native()]);
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).expect("native row must remain owned");
    let id = rows[0].account.id(b.store_id());
    let before = f.bytes();
    for endpoint in [
        Endpoint::Summary,
        Endpoint::Paid,
        Endpoint::Free,
        Endpoint::Status,
        Endpoint::Claim,
        Endpoint::Profile,
        Endpoint::AuthState,
        Endpoint::AuthToken,
    ] {
        assert_eq!(
            super::http::request_url(endpoint, Some(&rows[0].account), Some("synthetic-state")),
            Err(Failure::UnsupportedContext)
        );
    }
    let transport = NeverHttp(AtomicUsize::new(0));
    let clock = super::clock::FixedClock(1793232000000);
    let engine = super::engine::Engine {
        session: &f.session,
        transport: &transport,
        clock: &clock,
    };
    assert!(matches!(
        engine.refresh(&id).await,
        Err(Failure::UnsupportedContext)
    ));
    assert!(matches!(
        engine.claim(&id).await,
        Err(Failure::UnsupportedContext)
    ));
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    assert!(f.bytes() == before);
}

#[test]
fn original_owner_wrong_key_cannot_replace_existing_accounts() {
    let f = Fixture::new();
    f.put(vec![serde_json::to_value(api()).unwrap()]);
    let before = f.bytes();
    let wrong =
        SecretSession::from_context(f.root.path().into(), VaultContext::generate().unwrap());
    let b = store::binding(&wrong).unwrap();
    assert!(store::load(&wrong, &b).is_err());
    assert!(store::save(&wrong, &b, vec![]).is_err());
    assert!(f.bytes() == before);
}
#[test]
fn unknown_native_version_cannot_be_overwritten_by_empty_save() {
    let f = Fixture::new();
    let mut row = native();
    row["account"]["native"]["version"] = json!(99);
    f.put(vec![row]);
    let before = f.bytes();
    let b = store::binding(&f.session).unwrap();
    assert!(store::save(&f.session, &b, vec![]).is_err());
    assert!(f.bytes() == before);
}

pub(super) fn native_row_fixture() -> SavedAccount {
    serde_json::from_value(native()).unwrap()
}

#[tokio::test]
async fn mixed_refresh_uses_only_api_rows_and_keeps_native_snapshot() {
    use super::protocol::{Endpoint, Transport};
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct ApiOnly(AtomicUsize);
    impl Transport for ApiOnly {
        fn request<'a>(
            &'a self,
            _: Endpoint,
            account: Option<&'a Account>,
            _: Option<&'a str>,
            _: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            let account = account.unwrap();
            assert_eq!(account.uid, "synthetic-api");
            assert!(account.api_token().is_ok());
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!({"code":0,"data":{}})) })
        }
    }
    let f = Fixture::new();
    f.put(vec![native(), serde_json::to_value(api()).unwrap()]);
    let b = store::binding(&f.session).unwrap();
    let transport = ApiOnly(AtomicUsize::new(0));
    let clock = super::clock::FixedClock(1793232000000);
    let engine = super::engine::Engine {
        session: &f.session,
        transport: &transport,
        clock: &clock,
    };
    let views = engine.refresh_all().await.unwrap();
    assert_eq!(views.len(), 2);
    assert!(transport.0.load(Ordering::SeqCst) > 0);
    let rows = store::load(&f.session, &b).unwrap();
    assert_eq!(serde_json::to_value(&rows[0]).unwrap(), native());
    let before = f.bytes();
    let mut wrong = b.clone();
    if let crate::secrets::storage::StorageIdentity::Encrypted(ref mut identity) = wrong {
        identity.key_id = uuid::Uuid::new_v4().to_string();
    }
    assert!(store::save(&f.session, &wrong, rows.clone()).is_err());
    assert!(f.bytes() == before);
    store::save(&f.session, &b, rows.clone()).unwrap();
    store::save(&f.session, &b, rows).unwrap();
    assert_eq!(store::load(&f.session, &b).unwrap().len(), 2);
}

#[test]
#[serial_test::serial]
fn native_snapshot_migrates_through_original_generation_owner() {
    let f = crate::secrets::transition::fence_test_support::Fixture::new();
    let session = f.db.secret_session();
    let binding = store::binding(session).unwrap();
    store::save(session, &binding, vec![api(), native_row_fixture()]).unwrap();
    let original = serde_json::to_value(store::load(session, &binding).unwrap()).unwrap();
    crate::secrets::transition::rotate(
        &f.db,
        &f.store,
        crate::secrets::transition::fence_test_support::PASSWORD,
        true,
    )
    .unwrap();
    let next = store::binding(session).unwrap();
    assert_ne!(next, binding);
    assert_eq!(
        serde_json::to_value(store::load(session, &next).unwrap()).unwrap(),
        original
    );
    assert!(store::load(session, &binding).is_err());
}
