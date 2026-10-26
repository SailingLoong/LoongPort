use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs};

fn prepare_scope_at(
    store: &Path,
    old: &Path,
    device: &Path,
    inspection: &str,
) -> Result<LegacyPendingScope, AppError> {
    let expected = capture_legacy_selection_at(store, old)?;
    super::prepare_scope_at(store, old, device, inspection, &expected)
}

const INSPECTION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VAULT: &str = "4f281075-5976-4bba-aa7e-a3b24abc5d49";
const KEY: &str = "445863c5-7aa4-4f88-817a-7532fc72f6f6";

struct Fixture {
    _temp: tempfile::TempDir,
    store: PathBuf,
    old: PathBuf,
    device: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = crate::secrets::testing::tempdir().unwrap();
        let old = temp.path().join("old-data");
        let device = temp.path().join("old-device");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&device).unwrap();
        fs::write(old.join("loongport.db"), b"original encrypted accounts").unwrap();
        fs::write(old.join("vault.json"), b"original unavailable vault").unwrap();
        fs::write(device.join("settings.json"), b"original protected settings").unwrap();
        fs::create_dir(device.join("device-backups")).unwrap();
        fs::write(
            device.join("device-backups/state"),
            b"original encrypted device state",
        )
        .unwrap();
        let store = temp.path().join("control/app_paths.json");
        fs::create_dir(store.parent().unwrap()).unwrap();
        fs::write(
            &store,
            serde_json::to_vec(&json!({"app_config_dir_override":old,"unrelated":{"keep":true}}))
                .unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            store,
            old,
            device,
        }
    }
    fn prepare(&self) -> LegacyPendingScope {
        prepare_scope_at(&self.store, &self.old, &self.device, INSPECTION).unwrap()
    }
    fn hashes(&self) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        (tree_hash(&self.old), tree_hash(&self.device))
    }
}
fn tree_hash(root: &Path) -> BTreeMap<String, String> {
    fn visit(base: &Path, path: &Path, out: &mut BTreeMap<String, String>) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
            if p.is_dir() {
                out.insert(format!("{rel}/"), "dir".into());
                visit(base, &p, out);
            } else {
                out.insert(
                    rel,
                    Sha256::digest(fs::read(p).unwrap())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn vault_at(root: &Path, vault: &str, key: &str) {
    fs::write(
        root.join("vault.json"),
        serde_json::to_vec(
            &json!({"metadata":{"vaultId":vault,"keyId":key},"automaticUnlock":true}),
        )
        .unwrap(),
    )
    .unwrap();
}
fn install_fixture(f: &Fixture) -> IsolatedScope {
    let id = uuid::Uuid::new_v4().to_string();
    let root = f.store.parent().unwrap().join("isolated-configs").join(&id);
    fs::create_dir_all(&root).unwrap();
    let scope = IsolatedScope {
        format_version: 1,
        scope_id: id,
        root,
        preserved_root: f.old.clone(),
        preserved_device_root: f.device.clone(),
        inspection_id: INSPECTION.into(),
        vault_id: VAULT.into(),
        key_id: KEY.into(),
    };
    fs::write(
        scope.root.join(IDENTITY_FILE),
        serde_json::to_vec(&scope).unwrap(),
    )
    .unwrap();
    vault_at(&scope.root, VAULT, KEY);
    let mut doc: Value = serde_json::from_slice(&fs::read(&f.store).unwrap()).unwrap();
    doc[SELECTION_KEY] = serde_json::to_value(&scope).unwrap();
    fs::write(&f.store, serde_json::to_vec(&doc).unwrap()).unwrap();
    scope
}
#[test]
fn legacy_without_explicit_selection_is_unchanged() {
    let f = Fixture::new();
    let before = f.hashes();
    assert!(read_scope_at(&f.store).unwrap().is_none());
    assert_eq!(before, f.hashes());
}
#[test]
fn explicit_selection_resolves_only_its_bound_root() {
    let f = Fixture::new();
    let s = install_fixture(&f);
    assert_eq!(read_scope_at(&f.store).unwrap(), Some(s));
}
#[test]
fn missing_selected_root_never_falls_back() {
    let f = Fixture::new();
    let s = install_fixture(&f);
    let before = f.hashes();
    fs::remove_dir_all(&s.root).unwrap();
    assert!(read_scope_at(&f.store).is_err());
    assert_eq!(before, f.hashes());
    assert!(!s.root.exists());
}
#[test]
fn wrong_or_missing_identity_and_vault_fail_closed() {
    for mutation in [
        "marker-missing",
        "marker-id",
        "vault-missing",
        "vault-id",
        "key-id",
        "automatic-off",
    ] {
        let f = Fixture::new();
        let s = install_fixture(&f);
        match mutation {
            "marker-missing" => fs::remove_file(s.root.join(IDENTITY_FILE)).unwrap(),
            "marker-id" => {
                let mut bad = s.clone();
                bad.scope_id = uuid::Uuid::new_v4().to_string();
                fs::write(
                    s.root.join(IDENTITY_FILE),
                    serde_json::to_vec(&bad).unwrap(),
                )
                .unwrap();
            }
            "vault-missing" => fs::remove_file(s.root.join("vault.json")).unwrap(),
            "vault-id" => vault_at(&s.root, KEY, KEY),
            "key-id" => vault_at(&s.root, VAULT, VAULT),
            _ => fs::write(
                s.root.join("vault.json"),
                b"{\"metadata\":{},\"automaticUnlock\":false}",
            )
            .unwrap(),
        };
        let before = f.hashes();
        assert!(read_scope_at(&f.store).is_err(), "{mutation}");
        assert_eq!(before, f.hashes());
    }
}
#[test]
fn malformed_selection_is_an_error_not_legacy() {
    for value in [
        Value::Null,
        json!({}),
        json!("ignored"),
        json!({"formatVersion":99}),
    ] {
        let f = Fixture::new();
        fs::write(
            &f.store,
            serde_json::to_vec(&json!({SELECTION_KEY:value})).unwrap(),
        )
        .unwrap();
        assert!(read_scope_at(&f.store).is_err());
    }
}
#[test]
fn prepare_does_not_publish_or_touch_old_roots() {
    let f = Fixture::new();
    let before = f.hashes();
    let selector = fs::read(&f.store).unwrap();
    let pending = f.prepare();
    assert!(pending.root().is_dir());
    assert!(!pending.root().starts_with(&f.old));
    assert!(!pending.root().starts_with(&f.device));
    assert_eq!(selector, fs::read(&f.store).unwrap());
    assert_eq!(before, f.hashes());
    drop(pending);
    assert_eq!(selector, fs::read(&f.store).unwrap());
}
#[test]
fn publish_requires_existing_matching_new_vault() {
    for mode in ["missing", "wrong"] {
        let f = Fixture::new();
        let before = f.hashes();
        let selector = fs::read(&f.store).unwrap();
        let p = f.prepare();
        if mode == "wrong" {
            vault_at(p.root(), KEY, VAULT);
        }
        assert!(p.publish(VAULT, KEY).is_err());
        assert_eq!(selector, fs::read(&f.store).unwrap());
        assert_eq!(before, f.hashes());
    }
}
#[test]
fn successful_publication_preserves_legacy_bytes_and_unknown_keys() {
    let f = Fixture::new();
    let before = f.hashes();
    let p = f.prepare();
    vault_at(p.root(), VAULT, KEY);
    let selected = p.publish(VAULT, KEY).unwrap();
    assert_eq!(read_scope_at(&f.store).unwrap(), Some(selected));
    let v: Value = serde_json::from_slice(&fs::read(&f.store).unwrap()).unwrap();
    assert_eq!(v["unrelated"]["keep"], true);
    assert_eq!(v["app_config_dir_override"], json!(f.old));
    assert_eq!(before, f.hashes());
}
#[test]
fn concurrent_store_change_rejects_publication() {
    let f = Fixture::new();
    let p = f.prepare();
    vault_at(p.root(), VAULT, KEY);
    fs::write(&f.store, b"{\"otherOwner\":true}").unwrap();
    let changed = fs::read(&f.store).unwrap();
    assert!(p.publish(VAULT, KEY).is_err());
    assert_eq!(fs::read(&f.store).unwrap(), changed);
}
#[test]
fn second_preparation_is_denied_once_selected_even_if_selected_root_is_missing() {
    let f = Fixture::new();
    let s = install_fixture(&f);
    fs::remove_dir_all(&s.root).unwrap();
    let before = tree_hash(f.store.parent().unwrap());
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
    assert_eq!(before, tree_hash(f.store.parent().unwrap()));
}
#[cfg(unix)]
#[test]
fn aliased_scope_and_store_are_rejected_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let s = install_fixture(&f);
    let other = s.root.with_extension("other");
    fs::rename(&s.root, &other).unwrap();
    symlink(&other, &s.root).unwrap();
    assert!(read_scope_at(&f.store).is_err());
    let alias = f.store.with_extension("alias");
    symlink(&f.store, &alias).unwrap();
    assert!(read_scope_at(&alias).is_err());
}
#[test]
fn invalid_binding_and_scope_inside_old_root_are_rejected() {
    let f = Fixture::new();
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, "").is_err());
    let inner = f.old.join("app_paths.json");
    fs::write(&inner, b"{}").unwrap();
    let before = f.hashes();
    assert!(prepare_scope_at(&inner, &f.old, &f.device, INSPECTION).is_err());
    assert_eq!(before, f.hashes());
}

#[test]
fn interruptions_keep_old_selector_before_commit_and_new_selector_after_commit() {
    for stop in [
        PublishBoundary::IdentityDurable,
        PublishBoundary::SelectorStaged,
        PublishBoundary::SelectorPublished,
    ] {
        let f = Fixture::new();
        let before = f.hashes();
        let selector = fs::read(&f.store).unwrap();
        let p = f.prepare();
        let candidate = p.root().to_path_buf();
        vault_at(&candidate, VAULT, KEY);
        assert!(p
            .publish_inner(VAULT, KEY, |stage| if stage == stop {
                Err(invalid())
            } else {
                Ok(())
            })
            .is_err());
        assert_eq!(before, f.hashes());
        if stop == PublishBoundary::SelectorPublished {
            assert_eq!(read_scope_at(&f.store).unwrap().unwrap().root, candidate);
            assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
        } else {
            assert_eq!(selector, fs::read(&f.store).unwrap());
            assert!(read_scope_at(&f.store).unwrap().is_none());
        }
    }
}
#[test]
fn preparation_supports_an_absent_control_file_without_old_root_mutation() {
    let f = Fixture::new();
    fs::remove_dir_all(f.store.parent().unwrap()).unwrap();
    let before = f.hashes();
    let p = f.prepare();
    assert!(!f.store.exists());
    vault_at(p.root(), VAULT, KEY);
    let s = p.publish(VAULT, KEY).unwrap();
    assert_eq!(read_scope_at(&f.store).unwrap(), Some(s));
    assert_eq!(before, f.hashes());
}
#[test]
fn selection_lock_rejects_a_second_writer_and_unknown_stale_lock() {
    let f = Fixture::new();
    let p = f.prepare();
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
    drop(p);
    let lock = f.store.parent().unwrap().join("app_paths.selection.lock");
    fs::write(&lock, b"unknown owner").unwrap();
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
    assert_eq!(fs::read(lock).unwrap(), b"unknown owner");
}
#[test]
fn malformed_document_and_directory_store_fail_without_side_effects() {
    let f = Fixture::new();
    fs::write(&f.store, b"{broken").unwrap();
    assert!(read_scope_at(&f.store).is_err());
    fs::remove_file(&f.store).unwrap();
    fs::create_dir(&f.store).unwrap();
    assert!(read_scope_at(&f.store).is_err());
}

#[test]
fn process_scope_is_frozen_until_restart_not_changed_by_publication() {
    let f = Fixture::new();
    let cache = ScopeCache::default();
    cache.initialize_at(&f.store).unwrap();
    assert_eq!(cache.root().unwrap(), None);
    let p = f.prepare();
    vault_at(p.root(), VAULT, KEY);
    let s = p.publish(VAULT, KEY).unwrap();
    cache.initialize_at(&f.store).unwrap();
    assert_eq!(cache.root().unwrap(), None);
    let restarted = ScopeCache::default();
    restarted.initialize_at(&f.store).unwrap();
    assert_eq!(restarted.root().unwrap(), Some(s.root));
}
#[test]
fn failed_scope_cache_never_retries_or_falls_back_in_same_process() {
    let f = Fixture::new();
    let s = install_fixture(&f);
    let marker = s.root.join(IDENTITY_FILE);
    let saved = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    let cache = ScopeCache::default();
    assert!(cache.initialize_at(&f.store).is_err());
    assert!(cache.root().is_err());
    fs::write(marker, saved).unwrap();
    assert!(cache.initialize_at(&f.store).is_err());
    assert!(cache.root().is_err());
    let restarted = ScopeCache::default();
    restarted.initialize_at(&f.store).unwrap();
    assert_eq!(restarted.root().unwrap(), Some(s.root));
}
#[test]
fn uninitialized_scope_cache_is_not_a_legacy_selection() {
    assert!(ScopeCache::default().root().is_err());
}

#[test]
fn legacy_override_setter_cannot_clear_or_overwrite_an_isolated_selection() {
    for malformed in [false, true] {
        let f = Fixture::new();
        let scope = install_fixture(&f);
        if malformed {
            fs::remove_file(scope.root.join(IDENTITY_FILE)).unwrap();
        }
        let before = fs::read(&f.store).unwrap();
        assert!(write_legacy_override_at(&f.store, None).is_err());
        assert!(write_legacy_override_at(&f.store, Some("/another/root")).is_err());
        assert_eq!(fs::read(&f.store).unwrap(), before);
    }
}
#[test]
fn legacy_override_setter_preserves_other_store_keys_and_uses_same_writer_lock() {
    let f = Fixture::new();
    let p = f.prepare();
    assert!(write_legacy_override_at(&f.store, None).is_err());
    drop(p);
    write_legacy_override_at(&f.store, Some("  /new/legacy/root  ")).unwrap();
    let v: Value = serde_json::from_slice(&fs::read(&f.store).unwrap()).unwrap();
    assert_eq!(v["app_config_dir_override"], "/new/legacy/root");
    assert_eq!(v["unrelated"]["keep"], true);
    write_legacy_override_at(&f.store, None).unwrap();
    let v: Value = serde_json::from_slice(&fs::read(&f.store).unwrap()).unwrap();
    assert!(v.get("app_config_dir_override").is_none());
}

#[test]
fn unpublished_candidate_blocks_automatic_recreation_even_after_writer_drops() {
    let f = Fixture::new();
    let old = f.hashes();
    let selector = fs::read(&f.store).unwrap();
    let p = f.prepare();
    let root = p.root().to_path_buf();
    drop(p);
    let before = tree_hash(f.store.parent().unwrap());
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
    assert_eq!(before, tree_hash(f.store.parent().unwrap()));
    assert!(root.exists());
    assert_eq!(old, f.hashes());
    assert_eq!(selector, fs::read(&f.store).unwrap());
}
#[test]
fn unknown_pending_contents_are_never_reused_or_cleaned() {
    let f = Fixture::new();
    let container = f.store.parent().unwrap().join("isolated-configs");
    fs::create_dir(&container).unwrap();
    fs::write(container.join("unknown"), b"preserve").unwrap();
    assert!(prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION).is_err());
    assert_eq!(fs::read(container.join("unknown")).unwrap(), b"preserve");
}

#[test]
fn eligibility_selector_preimage_must_still_match_before_preparation() {
    let f = Fixture::new();
    let expected = capture_legacy_selection_at(&f.store, &f.old).unwrap();
    fs::write(&f.store, b"{\"changedBeforeAction\":true}").unwrap();
    let before = tree_hash(f.store.parent().unwrap());
    assert!(super::prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION, &expected).is_err());
    assert_eq!(before, tree_hash(f.store.parent().unwrap()));
}
#[test]
fn eligibility_snapshot_is_bound_to_its_control_store_not_just_identical_bytes() {
    let f = Fixture::new();
    let expected = capture_legacy_selection_at(&f.store, &f.old).unwrap();
    let other = f.store.parent().unwrap().join("other.json");
    fs::copy(&f.store, &other).unwrap();
    let before = tree_hash(f.store.parent().unwrap());
    assert!(super::prepare_scope_at(&other, &f.old, &f.device, INSPECTION, &expected).is_err());
    assert_eq!(before, tree_hash(f.store.parent().unwrap()));
}

#[test]
fn snapshot_resolves_persisted_override_not_a_stale_runtime_cache() {
    let f = Fixture::new();
    let newer = f._temp.path().join("newer-old-data");
    fs::create_dir(&newer).unwrap();
    fs::write(
        &f.store,
        serde_json::to_vec(&json!({"app_config_dir_override":newer})).unwrap(),
    )
    .unwrap();
    let expected = capture_legacy_selection_at(&f.store, &f.old).unwrap();
    let before = tree_hash(f.store.parent().unwrap());
    assert!(super::prepare_scope_at(&f.store, &f.old, &f.device, INSPECTION, &expected).is_err());
    assert_eq!(before, tree_hash(f.store.parent().unwrap()));
}
#[test]
fn absent_legacy_device_directory_is_preserved_as_absent() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.device).unwrap();
    let p = f.prepare();
    assert!(!f.device.exists());
    drop(p);
    assert!(!f.device.exists());
}
#[test]
fn scope_identity_does_not_force_an_automatic_key_custody_policy() {
    let f = Fixture::new();
    let s = install_fixture(&f);
    let path = s.root.join("vault.json");
    let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    doc["automaticUnlock"] = json!(false);
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(read_scope_at(&f.store).unwrap(), Some(s));
}
#[test]
fn publication_and_legacy_override_preserve_unknown_large_number_lexemes() {
    for isolated in [true, false] {
        let f = Fixture::new();
        let original=format!("{{\"app_config_dir_override\":{},\"futureInteger\":1844674407370955161600000000001,\"futurePreciseDecimal\":1.234567890123456789012345678901}}",serde_json::to_string(&f.old).unwrap());
        fs::write(&f.store, original).unwrap();
        if isolated {
            let p = f.prepare();
            vault_at(p.root(), VAULT, KEY);
            p.publish(VAULT, KEY).unwrap();
        } else {
            write_legacy_override_at(&f.store, None).unwrap();
        }
        let after = fs::read_to_string(&f.store).unwrap();
        assert!(after.contains("1844674407370955161600000000001"), "{after}");
        assert!(
            after.contains("1.234567890123456789012345678901"),
            "{after}"
        );
    }
}

#[test]
fn identity_ancestry_rejects_alias_spelling_even_when_textual_overlap_misses_it() {
    let old = Path::new("/volume/OldRoot");
    let control = Path::new("/volume/oldroot/control");
    assert!(!overlap(old, control));
    let same = DirectoryIdentity {
        volume: 11,
        index: 22,
    };
    let lookup = |path: &Path| {
        Ok(if path == old || path == Path::new("/volume/oldroot") {
            Some(same)
        } else {
            None
        })
    };
    assert!(overlap_by_identity_with(control, old, lookup).unwrap());
    assert!(overlap_by_identity_with(old, control, lookup).unwrap());
}
#[test]
fn duplicate_control_keys_fail_closed_instead_of_rewriting_ambiguous_document() {
    let f = Fixture::new();
    fs::write(&f.store, b"{\"future\":1,\"future\":2}").unwrap();
    let before = fs::read(&f.store).unwrap();
    assert!(read_scope_at(&f.store).is_err());
    assert!(write_legacy_override_at(&f.store, None).is_err());
    assert_eq!(fs::read(&f.store).unwrap(), before);
}

#[test]
fn native_directory_identity_distinguishes_siblings_and_detects_parenthood() {
    let f = Fixture::new();
    assert!(overlap_by_identity_with(&f.old, &f.device, directory_identity).is_ok_and(|b| !b));
    assert!(overlap_by_identity_with(&f.old.join("missing"), &f.old, directory_identity).unwrap());
    assert_eq!(
        directory_identity(&f.old).unwrap(),
        directory_identity(&f.old).unwrap()
    );
}
#[test]
fn case_insensitive_filesystem_alias_is_rejected_if_supported_by_this_volume() {
    let f = Fixture::new();
    let actual = f._temp.path().join("CaseSensitiveProbe");
    fs::create_dir(&actual).unwrap();
    let alias = f._temp.path().join("casesensitiveprobe");
    if !alias.exists() {
        eprintln!("case-insensitive volume not available; native Windows/macOS alias execution still required");
        return;
    }
    let control = actual.join("control");
    fs::create_dir(&control).unwrap();
    let store = control.join("app_paths.json");
    fs::write(&store, b"{}").unwrap();
    let expected = capture_legacy_selection_at(&store, &alias).unwrap();
    let before = tree_hash(&actual);
    assert!(super::prepare_scope_at(&store, &alias, &f.device, INSPECTION, &expected).is_err());
    assert_eq!(before, tree_hash(&actual));
}

#[test]
fn inability_to_locate_control_store_is_a_frozen_error_not_retry_to_legacy() {
    let f = Fixture::new();
    let cache = ScopeCache::default();
    assert!(cache.initialize_optional_path(None).is_err());
    assert!(cache.initialize_optional_path(Some(&f.store)).is_err());
    assert!(cache.root().is_err());
}

#[test]
fn v2_root_selection_is_mode_neutral_and_does_not_require_a_vault() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let root = f.store.parent().unwrap().join("isolated-configs").join(&id);
    fs::create_dir_all(&root).unwrap();
    let selected = json!({"formatVersion":2,"rootId":id,"root":root,
        "source":{"dataRoot":f.old,"deviceRoot":f.device,"rootId":null,
        "identityDigest":null,"selectionDigest":INSPECTION,"inspectionToken":INSPECTION}});
    fs::write(
        root.join(IDENTITY_FILE),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    fs::write(
        &f.store,
        serde_json::to_vec(&json!({SELECTION_KEY:selected})).unwrap(),
    )
    .unwrap();
    let cache = ScopeCache::new();
    cache
        .initialize_at(&f.store)
        .expect("v2 root identity does not choose credential protection");
    assert_eq!(cache.root().unwrap(), Some(root));
}

#[cfg(windows)]
#[test]
fn windows_verbatim_drive_root_is_never_probed_as_a_bare_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    assert!(root.is_absolute());
    validate_existing_ancestors(&root).unwrap();
    let nested = root.join("private/new-control");
    create_private_directories(&nested).unwrap();
    validate_directory(&nested).unwrap();
}
