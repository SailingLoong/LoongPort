use crate::secrets::transition::fence_test_support::{
    assert_rejected, tree, write_record, CountingStore, Scope, SCOPES,
};
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bootstrap_finalize_protected_scope() {
    for scope in SCOPES {
        let _scope = crate::secrets::testing::TestHome::new().unwrap();
        let f = Fixture::new();
        let root = f.root();
        let snapshot = f.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let prepared =
            prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();
        write_record(&root, scope, &prepared.data.next);
        let before = tree(f.home.path());
        let store = CountingStore::default();
        assert_rejected(finalize(&root, prepared, &store, true), scope);
        assert_eq!(before, tree(f.home.path()));
        assert_eq!(store.calls(), 0);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bootstrap_recover_protected_scope() {
    for scope in SCOPES {
        for inspected in [false, true] {
            let _scope = crate::secrets::testing::TestHome::new().unwrap();
            let f = Fixture::new();
            let root = f.root();
            std::fs::create_dir_all(&root).unwrap();
            let next = VaultContext::generate().unwrap();
            let intent = Intent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                metadata: next.metadata().clone(),
                body: "synthetic-unread-body".into(),
            };
            let bytes = serde_json::to_vec(&intent).unwrap();
            std::fs::write(root.join(INTENT), &bytes).unwrap();
            write_record(&root, scope, &next);
            let before = tree(f.home.path());
            let store = CountingStore::default();
            let result = if inspected {
                recover_inspected(&root, &store, None, &bytes)
            } else {
                recover(&root, &store, None)
            };
            assert_rejected(result, scope);
            assert_eq!(before, tree(f.home.path()));
            assert_eq!(store.calls(), 0);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bootstrap_target_text_without_record() {
    for recover_pending in [false, true] {
        let _scope = crate::secrets::testing::TestHome::new().unwrap();
        let f = Fixture::new();
        let root = f.root();
        std::fs::create_dir_all(&root).unwrap();
        let next = VaultContext::create_generation_with_user_text(
            uuid::Uuid::new_v4().to_string(),
            1,
            "synthetic exact text",
        )
        .unwrap()
        .with_password("remote recovery password")
        .unwrap();
        let store = CountingStore::default();
        if recover_pending {
            let intent = Intent {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                metadata: next.metadata().clone(),
                body: "synthetic-unread-body".into(),
            };
            let bytes = serde_json::to_vec(&intent).unwrap();
            std::fs::write(root.join(INTENT), &bytes).unwrap();
            let before = tree(f.home.path());
            assert_rejected(
                recover_inspected(&root, &store, None, &bytes),
                Scope::Encrypted,
            );
            assert_eq!(before, tree(f.home.path()));
        } else {
            let snapshot = f.snapshot_for_context(20, next);
            let expected = snapshot.manifest.snapshot_id.clone();
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let before = tree(f.home.path());
            assert_rejected(finalize(&root, prepared, &store, true), Scope::Encrypted);
            assert_eq!(before, tree(f.home.path()));
        }
        assert_eq!(store.calls(), 0);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bootstrap_legacy_no_record_restores() {
    let _scope = crate::secrets::testing::TestHome::new().unwrap();
    let f = Fixture::new();
    let root = f.root();
    let snapshot = f.snapshot();
    let expected = snapshot.manifest.snapshot_id.clone();
    let prepared =
        prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();
    let store = CountingStore::default();
    let session = finalize(&root, prepared, &store, true).unwrap();
    let db = Database::init_with_secrets(session).unwrap();
    assert!(db
        .get_provider_by_id("remote-provider", "claude")
        .unwrap()
        .is_some());
    assert!(!root.join("protection.json").exists());
    assert!(!root.join(INTENT).exists());
    assert!(store.calls() > 0);
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_legacy_fence(case: &str) {
    match case {
        "bootstrap_finalize_protected_scope" => bootstrap_finalize_protected_scope(),
        "bootstrap_recover_protected_scope" => bootstrap_recover_protected_scope(),
        "bootstrap_target_text_without_record" => bootstrap_target_text_without_record(),
        "bootstrap_legacy_no_record_restores" => bootstrap_legacy_no_record_restores(),
        _ => panic!("unknown legacy bootstrap fixture: {case}"),
    }
}
