use super::*;
use crate::secrets::transition::fence_test_support::{
    assert_rejected, tree, Fixture, Scope, PASSWORD, SCOPES,
};
fn intent(f: &Fixture, next: &VaultContext) -> Vec<u8> {
    let current = f.current();
    let mut saved = read_metadata(f.root()).unwrap();
    saved.metadata = next.metadata().clone();
    saved.automatic_unlock = true;
    let change = PasswordChange {
        previous_automatic_unlock: true,
        next: saved,
    };
    serde_json::to_vec(&PasswordIntent {
        candidate: next.metadata().clone(),
        body: current
            .seal(
                &["local", "password-transition"],
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap(),
    })
    .unwrap()
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn rewrap_change_protected_scope() {
    for scope in SCOPES {
        let f = Fixture::new();
        f.record(scope);
        let before = tree(f.home.path());
        assert_rejected(change_password(&f.db, &f.store, PASSWORD, true), scope);
        assert_eq!(before, tree(f.home.path()));
        assert_eq!(f.store.calls(), 0);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn rewrap_recover_protected_scope() {
    for scope in SCOPES {
        for route in 0..3 {
            let f = Fixture::new();
            let next = f.current().with_password(PASSWORD).unwrap();
            let bytes = intent(&f, &next);
            std::fs::write(f.root().join(INTENT), &bytes).unwrap();
            f.record(scope);
            let before = tree(f.home.path());
            let result = match route {
                0 => recover(f.root(), &mut f.current(), &f.store),
                1 => recover_with_password(f.root(), &f.store, Some(PASSWORD)),
                _ => recover_password_inspected(f.root(), &f.store, Some(PASSWORD), &bytes),
            };
            assert_rejected(result, scope);
            assert_eq!(before, tree(f.home.path()));
            assert_eq!(f.store.calls(), 0);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn rewrap_target_text_without_record() {
    for route in 0..3 {
        let f = Fixture::new();
        let next = VaultContext::create_generation_with_user_text(
            f.current().metadata().vault_id.clone(),
            2,
            "synthetic exact text",
        )
        .unwrap()
        .with_password(PASSWORD)
        .unwrap();
        let bytes = intent(&f, &next);
        std::fs::write(f.root().join(INTENT), &bytes).unwrap();
        let before = tree(f.home.path());
        let result = match route {
            0 => recover(f.root(), &mut f.current(), &f.store),
            1 => recover_with_password(f.root(), &f.store, Some(PASSWORD)),
            _ => recover_password_inspected(f.root(), &f.store, Some(PASSWORD), &bytes),
        };
        assert_rejected(result, Scope::Encrypted);
        assert_eq!(before, tree(f.home.path()));
        assert_eq!(f.store.calls(), 0);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn rewrap_legacy_no_record_changes_password() {
    let f = Fixture::new();
    let old = f.current().metadata().clone();
    change_password(&f.db, &f.store, PASSWORD, true).unwrap();
    let next = f.current();
    assert_eq!(next.metadata().key_id, old.key_id);
    assert!(next.metadata().revision > old.revision);
    assert_eq!(
        f.db.get_setting("global_proxy_url").unwrap().as_deref(),
        Some("synthetic-fence-value")
    );
    assert!(!f.root().join("protection.json").exists());
    assert!(!f.root().join(INTENT).exists());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn rewrap_current_text_without_record() {
    let f = Fixture::with_user_text();
    let before = tree(f.home.path());
    assert_rejected(
        change_password(&f.db, &f.store, PASSWORD, true),
        Scope::Encrypted,
    );
    assert_eq!(before, tree(f.home.path()));
    assert_eq!(f.store.calls(), 0);
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify(case: &str) {
    match case {
        "rewrap_current_text_without_record" => rewrap_current_text_without_record(),
        "rewrap_change_protected_scope" => rewrap_change_protected_scope(),
        "rewrap_recover_protected_scope" => rewrap_recover_protected_scope(),
        "rewrap_target_text_without_record" => rewrap_target_text_without_record(),
        "rewrap_legacy_no_record_changes_password" => rewrap_legacy_no_record_changes_password(),
        _ => panic!("unknown legacy rewrap fixture: {case}"),
    }
}
