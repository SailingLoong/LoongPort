use super::{plain, tree, Fixture};
use crate::services::sync_protocol;

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plain_snapshot_has_explicit_plaintext_descriptor_without_a_vault() {
    let fixture = Fixture::new(plain());
    let before = tree(fixture._home.path());
    let snapshot = sync_protocol::build_local_snapshot(&fixture.db)
        .expect("selected plaintext storage must export plaintext without an invented key");
    let manifest: serde_json::Value = serde_json::from_slice(&snapshot.manifest_bytes).unwrap();
    assert_eq!(manifest["version"], 4);
    assert_eq!(manifest["storage"]["mode"], "plaintext");
    assert!(manifest.get("vault").is_none());
    assert!(std::str::from_utf8(&snapshot.db_sql)
        .unwrap()
        .contains("local-before-canary"));
    assert_eq!(tree(fixture._home.path()), before);
    assert!(!fixture.db.secrets.root().join("vault.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encrypted_snapshot_retains_original_v3_descriptor() {
    let fixture = Fixture::new(plain());
    let manifest = serde_json::to_value(&fixture.snapshot.manifest).unwrap();
    assert_eq!(manifest["version"], 3);
    assert!(manifest.get("vault").is_some());
    assert!(manifest.get("storage").is_none());
    assert!(!std::str::from_utf8(&fixture.snapshot.db_sql)
        .unwrap()
        .contains("remote-joint-canary"));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encrypted_v3_descriptor_cannot_also_claim_plaintext() {
    let fixture = Fixture::new(plain());
    let mut value = serde_json::to_value(&fixture.snapshot.manifest).unwrap();
    let crate::secrets::storage::StorageContext::Plaintext(identity) = &fixture.current else {
        unreachable!();
    };
    value["storage"] = serde_json::json!({"mode":"plaintext", "identity":identity});
    let admitted = serde_json::from_value::<sync_protocol::SyncManifest>(value)
        .ok()
        .is_some_and(|manifest| {
            sync_protocol::validate_manifest_compat(&manifest, sync_protocol::RemoteLayout::Current)
                .is_ok()
        });
    assert!(
        !admitted,
        "a manifest cannot claim encrypted and plaintext sources simultaneously"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_source_restores_into_selected_plaintext_without_input() {
    let fixture = Fixture::new_with_source(plain(), plain());
    fixture.restore("").unwrap();
    fixture.assert_installed();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_source_preserves_selected_encrypted_target() {
    let fixture = Fixture::new_with_source(super::text(super::TARGET_TEXT), plain());
    fixture.restore("").unwrap();
    fixture.assert_installed();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_source_rejects_irrelevant_input_before_any_local_write() {
    let fixture = Fixture::new_with_source(plain(), plain());
    let before = tree(fixture._home.path());
    let error = fixture
        .restore("must-not-be-used-as-a-new-key")
        .unwrap_err();
    assert!(error.to_string().contains("sync.source_input_not_required"));
    assert_eq!(tree(fixture._home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_source_keeps_legacy_encrypted_owner_without_inventing_a_choice() {
    let legacy = crate::secrets::VaultContext::generate()
        .unwrap()
        .with_password("legacy recovery material")
        .unwrap();
    let mut fixture = Fixture::new_with_source(legacy.into(), plain());
    let record = fixture
        .db
        .secrets
        .root()
        .join(crate::secrets::protection::FILE);
    std::fs::remove_file(&record).unwrap();
    fixture.metadata.remove(&record);
    fixture.restore("").unwrap();
    fixture.assert_installed();
    assert!(
        !record.exists(),
        "remote content must not create a protection choice"
    );
}

#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_registered_recovery_describes_its_source_input() {
    let fixture = Fixture::new(plain());
    super::interrupt(&fixture, super::Checkpoint::Intent);
    let app = super::recovery_app(fixture.db.secrets.root());
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let status = super::invoke(&window, "get_startup_recovery", serde_json::json!({})).unwrap();
    assert_eq!(status["canRecover"], true);
    assert_eq!(status["inputRequired"], true);
    assert_eq!(status["inputKind"], "source");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_skill_content_is_not_a_credential_envelope() {
    let body = b"lpenc:arbitrary skill content, not a credential envelope";
    let fixture = Fixture::new_with_source_and_skill(plain(), plain(), body);
    let epoch = fixture.db.secrets.lifecycle_epoch();
    fixture.restore("").unwrap();
    assert_eq!(
        std::fs::read(
            fixture
                .db
                .secrets
                .root()
                .join("skills/remote-skill/SKILL.md")
        )
        .unwrap(),
        body
    );
    assert_eq!(
        fixture.db.secrets.read().unwrap().identity(),
        fixture.current.identity()
    );
    assert!(
        fixture.db.secrets.lifecycle_epoch() > epoch,
        "old leases must not revive after same-mode DB replacement"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn missing_target_record_is_not_a_plaintext_choice() {
    let fixture = Fixture::new_with_source(plain(), plain());
    std::fs::remove_file(
        fixture
            .db
            .secrets
            .root()
            .join(crate::secrets::protection::FILE),
    )
    .unwrap();
    let before = tree(fixture._home.path());
    assert!(fixture.restore("").is_err());
    assert_eq!(tree(fixture._home.path()), before);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run(case: &str) {
    match case {
        "plain_restore" => plaintext_source_restores_into_selected_plaintext_without_input(),
        "plain_to_encrypted" => plaintext_source_preserves_selected_encrypted_target(),
        "plain_reject_input" => plaintext_source_rejects_irrelevant_input_before_any_local_write(),
        "plain_to_legacy" => {
            plaintext_source_keeps_legacy_encrypted_owner_without_inventing_a_choice()
        }
        #[cfg(feature = "gui")]
        "recovery_input" => original_registered_recovery_describes_its_source_input(),
        "skill_bytes" => plaintext_skill_content_is_not_a_credential_envelope(),
        "missing_choice" => missing_target_record_is_not_a_plaintext_choice(),
        "plain_export" => plain_snapshot_has_explicit_plaintext_descriptor_without_a_vault(),
        "encrypted_export" => encrypted_snapshot_retains_original_v3_descriptor(),
        "mixed_descriptor" => encrypted_v3_descriptor_cannot_also_claim_plaintext(),
        _ => panic!("unknown remote plaintext case"),
    }
}
