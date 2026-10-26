//! Synthetic legacy-reset admission tests. No system credential provider is used.
use super::*;
use crate::secrets::{
    key_store::{self, KeyStore},
    testing::TestHome,
    transition::fence_test_support::{
        self as support, CountingStore, Fixture, Scope, PASSWORD, SCOPES,
    },
};

struct ResetFixture {
    root: PathBuf,
    current: VaultContext,
    store: CountingStore,
    cached: zeroize::Zeroizing<Vec<u8>>,
    home: TestHome,
}
impl ResetFixture {
    fn new(text: bool) -> Self {
        let original = if text {
            Fixture::with_user_text()
        } else {
            Fixture::new()
        };
        let root = original.root().to_owned();
        let current = original.current();
        let Fixture { db, store, home } = original;
        // Reset works on directory generations, never on a live Database handle.
        drop(db);
        let metadata = current.metadata();
        if text {
            key_store::save_user_text_verified(&store, &current, "synthetic exact text").unwrap();
        } else {
            key_store::save_verified(
                &store,
                &metadata.vault_id,
                &metadata.key_id,
                &current.export_key(),
            )
            .unwrap();
        }
        let cached = store
            .load(&metadata.vault_id, &metadata.key_id)
            .unwrap()
            .unwrap();
        store.reset();
        Self {
            root,
            current,
            store,
            cached,
            home,
        }
    }
    fn record(&self, scope: Scope) {
        support::write_record(&self.root, scope, &self.current);
    }
    fn assert_no_write<T>(&self, operation: impl FnOnce() -> Result<T, AppError>, scope: Scope) {
        let before = support::tree(self.home.path());
        self.store.reset();
        let result = operation();
        support::assert_rejected(result, scope);
        assert_eq!(
            support::tree(self.home.path()),
            before,
            "reset admission changed tree bytes or mtimes"
        );
        // Reset has no KeyStore parameter or system-cache calls in its call graph.
        // This counter asserts the synthetic cached account was not touched; it
        // is not an injected trap for an arbitrary OS credential implementation.
        assert_eq!(self.store.calls(), 0);
        let metadata = self.current.metadata();
        let after = self
            .store
            .load(&metadata.vault_id, &metadata.key_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.as_slice(), self.cached.as_slice());
        self.store.reset();
    }
    fn approval(&self) -> String {
        // Build the exact old reset fingerprint passively, without calling the
        // new guarded preview. SQLite opens only the private captured image.
        let captured =
            crate::database::inspection::capture(&self.root.join(crate::config::DB_FILE_NAME))
                .unwrap()
                .unwrap();
        fingerprint(
            &self.root,
            &captured.image,
            &read_optional(&crate::settings::settings_path()).unwrap(),
        )
        .unwrap()
    }
}
fn target(text: bool) -> VaultContext {
    let context = if text {
        VaultContext::create_generation_with_user_text(
            uuid::Uuid::new_v4().to_string(),
            1,
            "synthetic target exact text",
        )
        .unwrap()
    } else {
        VaultContext::generate().unwrap()
    };
    // Separate legacy recovery-password fixture, not the user-text KDF contract.
    context.with_password(PASSWORD).unwrap()
}
fn manifest() -> Manifest {
    Manifest {
        device: None,
        phase: ResetPhase::Prepared,
        fingerprint: "synthetic-original-fingerprint".into(),
        archive_hash: hash(b"synthetic archive not published"),
        database_hash: hash(b"synthetic database not published"),
        settings_hash: hash(b"synthetic settings not published"),
    }
}
fn intent(next: &VaultContext, manifest: &Manifest) -> Intent {
    let id = uuid::Uuid::new_v4().to_string();
    let body = next
        .seal(
            &["local", "reset-intent", &id],
            &serde_json::to_vec(manifest).unwrap(),
        )
        .unwrap();
    Intent {
        id,
        metadata: next.metadata().clone(),
        body,
    }
}
fn write_intent(root: &Path, intent: &Intent) -> Vec<u8> {
    let bytes = serde_json::to_vec(intent).unwrap();
    session::write_durable(&intent_path(root).unwrap(), &bytes).unwrap();
    bytes
}
fn stop_before_install(point: Checkpoint) -> Result<(), AppError> {
    if matches!(point, Checkpoint::Published) {
        Err(AppError::Config("test.reset_stopped_before_install".into()))
    } else {
        Ok(())
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_preview_protected_scope() {
    for scope in SCOPES {
        let f = ResetFixture::new(false);
        f.record(scope);
        f.assert_no_write(|| preview(&f.root), scope);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_write_protected_scope() {
    for scope in SCOPES {
        let f = ResetFixture::new(false);
        f.record(scope);
        let expected = f.approval();
        // Missing fence RED is bounded before installing or deleting directories.
        f.assert_no_write(
            || reset_with_hook(&f.root, &expected, PASSWORD, &mut stop_before_install),
            scope,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_recover_protected_scope() {
    for scope in SCOPES {
        let f = ResetFixture::new(false);
        f.record(scope);
        let next = target(false);
        let pending = write_intent(&f.root, &intent(&next, &manifest()));
        for password in [None, Some(PASSWORD)] {
            f.assert_no_write(|| recover(&f.root, password), scope);
            f.assert_no_write(|| recover_inspected(&f.root, password, &pending), scope);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_publish_protected_scope() {
    for scope in SCOPES {
        let f = ResetFixture::new(false);
        f.record(scope);
        let next = target(false);
        f.assert_no_write(
            || {
                publish_intent(
                    &f.root,
                    &uuid::Uuid::new_v4().to_string(),
                    &manifest(),
                    &next,
                )
            },
            scope,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_finish_protected_scope() {
    for scope in SCOPES {
        let f = ResetFixture::new(false);
        f.record(scope);
        let next = target(false);
        let manifest = manifest();
        let intent = intent(&next, &manifest);
        f.assert_no_write(
            || finish(&f.root, &intent, &manifest, &next, &mut |_| Ok(())),
            scope,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_current_text_without_record() {
    let f = ResetFixture::new(true);
    let expected = f.approval();
    f.assert_no_write(|| preview(&f.root), Scope::Encrypted);
    f.assert_no_write(
        || reset_with_hook(&f.root, &expected, PASSWORD, &mut stop_before_install),
        Scope::Encrypted,
    );
    // Even no pending journal must not admit a new-text scope to legacy recovery.
    f.assert_no_write(|| recover(&f.root, None), Scope::Encrypted);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_current_text_db_survives_corrupt_vault_record() {
    let f = ResetFixture::new(true);
    std::fs::write(f.root.join("vault.json"), b"{legacy opaque broken metadata").unwrap();
    let expected = f.approval();
    f.assert_no_write(|| preview(&f.root), Scope::Encrypted);
    f.assert_no_write(
        || reset_with_hook(&f.root, &expected, PASSWORD, &mut stop_before_install),
        Scope::Encrypted,
    );
    f.assert_no_write(|| recover(&f.root, None), Scope::Encrypted);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_readable_text_vault_fenced_without_matching_db() {
    let f = ResetFixture::new(false);
    let next = target(true);
    session::write_metadata(&f.root, &session::completed_metadata(&next, false).unwrap()).unwrap();
    f.assert_no_write(|| preview(&f.root), Scope::Encrypted);
    f.assert_no_write(|| recover(&f.root, None), Scope::Encrypted);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_intent_text_without_record() {
    let f = ResetFixture::new(false);
    let next = target(true);
    let pending = write_intent(&f.root, &intent(&next, &manifest()));
    for password in [None, Some(PASSWORD)] {
        f.assert_no_write(|| recover(&f.root, password), Scope::Encrypted);
        f.assert_no_write(
            || recover_inspected(&f.root, password, &pending),
            Scope::Encrypted,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_target_text_without_record() {
    let f = ResetFixture::new(false);
    let next = target(true);
    let manifest = manifest();
    let intent = intent(&next, &manifest);
    f.assert_no_write(
        || publish_intent(&f.root, &intent.id, &manifest, &next),
        Scope::Encrypted,
    );
    f.assert_no_write(
        || finish(&f.root, &intent, &manifest, &next, &mut |_| Ok(())),
        Scope::Encrypted,
    );
    let raw = target(false);
    f.assert_no_write(
        || finish(&f.root, &intent, &manifest, &raw, &mut |_| Ok(())),
        Scope::Encrypted,
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_finish_staged_or_previous_protected_scope() {
    for previous in [false, true] {
        for scope in SCOPES {
            let f = ResetFixture::new(false);
            let next = target(false);
            let manifest = manifest();
            let intent = intent(&next, &manifest);
            let (stage, old, _) = locations(&f.root, &intent.id).unwrap();
            support::write_record(if previous { &old } else { &stage }, scope, &f.current);
            f.assert_no_write(
                || finish(&f.root, &intent, &manifest, &next, &mut |_| Ok(())),
                scope,
            );
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_finish_staged_or_previous_text_metadata() {
    let text = target(true);
    for previous in [false, true] {
        let f = ResetFixture::new(false);
        let next = target(false);
        let manifest = manifest();
        let intent = intent(&next, &manifest);
        let (stage, old, _) = locations(&f.root, &intent.id).unwrap();
        let protected = if previous { &old } else { &stage };
        crate::config::ensure_private_directory(protected).unwrap();
        session::write_metadata(
            protected,
            &session::completed_metadata(&text, false).unwrap(),
        )
        .unwrap();
        f.assert_no_write(
            || finish(&f.root, &intent, &manifest, &next, &mut |_| Ok(())),
            Scope::Encrypted,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_legacy_preview_without_record_stays_read_only() {
    for corrupt_vault in [false, true] {
        let f = ResetFixture::new(false);
        if corrupt_vault {
            // Old reset intentionally archives unreadable metadata verbatim.
            std::fs::write(f.root.join("vault.json"), b"{legacy opaque broken metadata").unwrap();
        }
        let before = support::tree(f.home.path());
        let approval = preview(&f.root).unwrap();
        assert!(!approval.fingerprint.is_empty());
        assert!(approval.protected_values > 0);
        assert_eq!(support::tree(f.home.path()), before);
        assert_eq!(f.store.calls(), 0);
        assert!(!pending(&f.root).unwrap());
        recover(&f.root, None).unwrap();
        assert_eq!(support::tree(f.home.path()), before);
    }
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify(case: &str) {
    match case {
        "reset_preview_protected_scope" => reset_preview_protected_scope(),
        "reset_write_protected_scope" => reset_write_protected_scope(),
        "reset_recover_protected_scope" => reset_recover_protected_scope(),
        "reset_publish_protected_scope" => reset_publish_protected_scope(),
        "reset_finish_protected_scope" => reset_finish_protected_scope(),
        "reset_current_text_without_record" => reset_current_text_without_record(),
        "reset_current_text_db_survives_corrupt_vault_record" => {
            reset_current_text_db_survives_corrupt_vault_record()
        }
        "reset_readable_text_vault_fenced_without_matching_db" => {
            reset_readable_text_vault_fenced_without_matching_db()
        }
        "reset_intent_text_without_record" => reset_intent_text_without_record(),
        "reset_target_text_without_record" => reset_target_text_without_record(),
        "reset_finish_staged_or_previous_protected_scope" => {
            reset_finish_staged_or_previous_protected_scope()
        }
        "reset_finish_staged_or_previous_text_metadata" => {
            reset_finish_staged_or_previous_text_metadata()
        }
        "reset_legacy_preview_without_record_stays_read_only" => {
            reset_legacy_preview_without_record_stays_read_only()
        }
        _ => panic!("unknown reset legacy fence test case"),
    }
}
