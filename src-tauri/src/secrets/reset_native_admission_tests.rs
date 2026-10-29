//! Key-loss reset never receives the original native storage context. All native
//! receipts below use the actual registered codec in private synthetic roots;
//! they are not native activation, account consumption, or a new reset protocol.
use super::*;
use crate::secrets::{storage::StorageContext, testing::TestHome, transition::fence_test_support};
use crate::workbuddy::native_owner::testing as native_fixture;
use std::{collections::BTreeMap, time::SystemTime};

const PASSWORD: &str = "synthetic native reset archive password";
const NATIVE: &str = super::super::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE;
type Image = BTreeMap<PathBuf, (u8, Vec<u8>, u64, Option<SystemTime>)>;

// Include sibling reset stage/archive paths and metadata; never follow a marker
// symlink or open a non-regular marker while observing a refusal.
fn image(root: &Path) -> Image {
    fn walk(base: &Path, path: &Path, out: &mut Image) {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        let (kind, bytes) = if metadata.file_type().is_symlink() {
            (
                2,
                std::fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else if metadata.is_file() {
            (1, std::fs::read(path).unwrap())
        } else if metadata.is_dir() {
            (0, Vec::new())
        } else {
            (3, Vec::new())
        };
        out.insert(
            path.strip_prefix(base).unwrap().to_owned(),
            (kind, bytes, metadata.len(), metadata.modified().ok()),
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                walk(base, &entry.unwrap().path(), out);
            }
        }
    }
    let mut out = Image::new();
    walk(root, root, &mut out);
    out
}

struct Fixture {
    root: PathBuf,
    original: VaultContext,
    home: TestHome,
    client: tempfile::TempDir,
    owner: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let source = fence_test_support::Fixture::new();
        source.db.set_setting("theme", "dark").unwrap();
        let root = source.root().to_owned();
        let original = source.current();
        let fence_test_support::Fixture { db, store: _, home } = source;
        // No live SQLite handle may cross the original directory replacement.
        drop(db);
        Self {
            root,
            original,
            home,
            client: crate::secrets::testing::tempdir().unwrap(),
            owner: crate::secrets::testing::tempdir().unwrap(),
        }
    }
    fn receipts(&self, wrong_key: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let context = StorageContext::Encrypted(if wrong_key {
            VaultContext::generate().unwrap()
        } else {
            self.original.clone()
        });
        native_fixture::phase_receipts(self.owner.path(), &context, self.client.path())
    }
    fn place_marker(&self, kind: &str) {
        let (prepared, restored, committed) = self.receipts(kind == "other-key");
        let bytes = match kind {
            "prepared" => prepared,
            "restored" => restored,
            "corrupt" => b"synthetic untrusted native marker".to_vec(),
            "empty" => Vec::new(),
            _ => committed,
        };
        session::write_durable(&self.root.join(NATIVE), &bytes).unwrap();
    }
    fn historical_approval(&self) -> String {
        // The exact original fingerprint models an approval issued by a legacy
        // version which did not gate native presence. No original key is given
        // to the keyless reset consumer and no fingerprint rule is weakened.
        fingerprint(
            &self.root,
            &snapshot(&self.root).unwrap(),
            &read_optional(&crate::settings::settings_path()).unwrap(),
        )
        .unwrap()
    }
    fn interrupt(&self, checkpoint: Checkpoint) {
        let approval = preview(&self.root).unwrap();
        let result = reset_with_hook(&self.root, &approval.fingerprint, PASSWORD, &mut |at| {
            if at == checkpoint {
                Err(AppError::Config("test.native_reset_checkpoint".into()))
            } else {
                Ok(())
            }
        });
        assert!(
            matches!(result, Err(AppError::Config(code)) if code == "test.native_reset_checkpoint")
        );
    }
    fn record(&self) -> (Intent, Manifest, VaultContext) {
        let intent: Intent =
            serde_json::from_slice(&pending_record(&self.root).unwrap().unwrap()).unwrap();
        let next = VaultContext::from_password(intent.metadata.clone(), PASSWORD).unwrap();
        let plain = next
            .open(&["local", "reset-intent", &intent.id], &intent.body)
            .unwrap();
        let manifest = serde_json::from_slice(&plain).unwrap();
        (intent, manifest, next)
    }
    fn assert_refused_unchanged(&self, operation: impl FnOnce() -> Result<(), AppError>) {
        let before = image(self.home.path());
        let result = operation();
        assert!(
            image(self.home.path()) == before,
            "native reset refusal changed source, stage, archive, intent, or settings"
        );
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "secret.recovery_required"),
            "native reset admission did not refuse: {result:?}"
        );
    }
}

#[test]
#[serial_test::serial]
fn native_persistence_blocks_keyless_preview_without_writes() {
    for kind in [
        "prepared",
        "restored",
        "committed",
        "corrupt",
        "empty",
        "other-key",
    ] {
        let f = Fixture::new();
        f.place_marker(kind);
        for _ in 0..2 {
            f.assert_refused_unchanged(|| preview(&f.root).map(|_| ()));
        }
    }
}

#[test]
#[serial_test::serial]
fn native_persistence_blocks_original_historical_approval_before_publication() {
    for kind in [
        "prepared",
        "restored",
        "committed",
        "corrupt",
        "empty",
        "other-key",
    ] {
        let f = Fixture::new();
        f.place_marker(kind);
        let expected = f.historical_approval();
        for _ in 0..2 {
            f.assert_refused_unchanged(|| {
                reset_with_hook(&f.root, &expected, PASSWORD, &mut |_| {
                    Err(AppError::Config(
                        "test.unsafe_native_reset_publication".into(),
                    ))
                })
                .map(|_| ())
            });
        }
    }
}

#[test]
#[serial_test::serial]
fn native_nonregular_marker_refuses_without_following_or_replacing_it() {
    let f = Fixture::new();
    let expected = preview(&f.root).unwrap().fingerprint;
    std::fs::create_dir(f.root.join(NATIVE)).unwrap();
    f.assert_refused_unchanged(|| preview(&f.root).map(|_| ()));
    f.assert_refused_unchanged(|| reset(&f.root, &expected, PASSWORD).map(|_| ()));
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn native_marker_symlink_refuses_and_preserves_the_private_target() {
    let f = Fixture::new();
    let expected = preview(&f.root).unwrap().fingerprint;
    let target = f.home.path().join("synthetic-native-marker-target");
    std::fs::write(&target, b"retained synthetic native recovery evidence").unwrap();
    std::os::unix::fs::symlink(&target, f.root.join(NATIVE)).unwrap();
    f.assert_refused_unchanged(|| preview(&f.root).map(|_| ()));
    f.assert_refused_unchanged(|| reset(&f.root, &expected, PASSWORD).map(|_| ()));
}

#[test]
#[serial_test::serial]
fn prepared_reset_reentry_refuses_native_marker_before_any_directory_or_settings_write() {
    for at in [
        Checkpoint::Published,
        Checkpoint::Archived,
        Checkpoint::Installed,
    ] {
        for location in ["root", "stage", "previous"] {
            let f = Fixture::new();
            let prepared = f.receipts(false).0;
            f.interrupt(at);
            let (intent, manifest, _) = f.record();
            assert!(manifest.phase == ResetPhase::Prepared);
            let (stage, previous, _) = locations(&f.root, &intent.id).unwrap();
            let source = match location {
                "stage" => &stage,
                "previous" => &previous,
                _ => &f.root,
            };
            if !source.is_dir() {
                continue;
            }
            session::write_durable(&source.join(NATIVE), &prepared).unwrap();
            for _ in 0..2 {
                f.assert_refused_unchanged(|| recover(&f.root, Some(PASSWORD)));
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn committed_reset_cannot_erase_a_late_native_marker_in_root_or_previous() {
    for in_previous in [false, true] {
        let f = Fixture::new();
        let prepared = f.receipts(false).0;
        f.interrupt(Checkpoint::CleanupCommitted);
        let (intent, manifest, _) = f.record();
        assert!(manifest.phase == ResetPhase::CleanupCommitted);
        let (_, previous, _) = locations(&f.root, &intent.id).unwrap();
        let source = if in_previous { &previous } else { &f.root };
        session::write_durable(&source.join(NATIVE), &prepared).unwrap();
        for _ in 0..2 {
            f.assert_refused_unchanged(|| recover(&f.root, Some(PASSWORD)));
        }
    }
}

// Reconstruct a native-bearing historical archive through the actual owner
// structs/codecs, as the original ZCode archive-only regression does. The reset
// replacement and checkpoint are real; no second reset engine is implemented.
fn archive_with_native(f: &Fixture, checkpoint: Checkpoint) {
    let prepared = f.receipts(false).0;
    f.interrupt(checkpoint);
    let (intent, mut manifest, next) = f.record();
    let (_, _, archive_path) = locations(&f.root, &intent.id).unwrap();
    let mut archive: RecoveryArchive =
        serde_json::from_slice(&std::fs::read(&archive_path).unwrap()).unwrap();
    let plaintext = next
        .open(&["local", "reset-archive", &intent.id], &archive.content)
        .unwrap();
    let mut original = zip::ZipArchive::new(Cursor::new(plaintext.as_slice())).unwrap();
    let member = format!("data/{NATIVE}");
    assert!(!original.file_names().any(|name| name == member));
    let mut updated = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(0o600);
    for index in 0..original.len() {
        let mut file = original.by_index(index).unwrap();
        assert!(file.is_file());
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        file.read_to_end(&mut bytes).unwrap();
        updated.start_file(file.name(), options).unwrap();
        updated.write_all(&bytes).unwrap();
    }
    updated.start_file(member, options).unwrap();
    updated.write_all(&prepared).unwrap();
    archive.content = next
        .seal(
            &["local", "reset-archive", &intent.id],
            &updated.finish().unwrap().into_inner(),
        )
        .unwrap();
    let bytes = serde_json::to_vec(&archive).unwrap();
    session::write_durable(&archive_path, &bytes).unwrap();
    manifest.archive_hash = hash(&bytes);
    // Re-authenticate only this synthetic historical owner record with its
    // original ID/key/phase. Never discard or recapture a production intent.
    publish_intent(&f.root, &intent.id, &manifest, &next).unwrap();
}

#[test]
#[serial_test::serial]
fn authenticated_native_archive_only_blocks_prepared_and_committed_reentry() {
    for at in [Checkpoint::Installed, Checkpoint::CleanupCommitted] {
        let f = Fixture::new();
        archive_with_native(&f, at);
        assert!(!f.root.join(NATIVE).exists());
        let (intent, _, _) = f.record();
        let (_, previous, _) = locations(&f.root, &intent.id).unwrap();
        assert!(!previous.join(NATIVE).exists());
        for _ in 0..2 {
            f.assert_refused_unchanged(|| recover(&f.root, Some(PASSWORD)));
        }
    }
}

#[test]
#[serial_test::serial]
fn untrusted_archive_and_stale_owner_never_write_or_replay() {
    for corruption in ["hash", "id", "key"] {
        let f = Fixture::new();
        f.interrupt(Checkpoint::Published);
        let original_intent = pending_record(&f.root).unwrap().unwrap();
        let (intent, mut manifest, next) = f.record();
        let (_, _, archive_path) = locations(&f.root, &intent.id).unwrap();
        let mut archive: RecoveryArchive =
            serde_json::from_slice(&std::fs::read(&archive_path).unwrap()).unwrap();
        match corruption {
            "hash" => archive.content.push('x'),
            "id" => archive.id = uuid::Uuid::new_v4().to_string(),
            _ => {
                archive.content = VaultContext::generate()
                    .unwrap()
                    .seal(
                        &["local", "reset-archive", &intent.id],
                        b"untrusted synthetic archive",
                    )
                    .unwrap()
            }
        }
        let bytes = serde_json::to_vec(&archive).unwrap();
        session::write_durable(&archive_path, &bytes).unwrap();
        if corruption != "hash" {
            manifest.archive_hash = hash(&bytes);
            publish_intent(&f.root, &intent.id, &manifest, &next).unwrap();
            let before = image(f.home.path());
            assert!(
                matches!(recover_inspected(&f.root, Some(PASSWORD), &original_intent), Err(AppError::Config(code)) if code == "upgrade.source_changed")
            );
            assert!(
                before == image(f.home.path()),
                "untrusted owner recovery changed the synthetic tree"
            );
        }
        let before = image(f.home.path());
        assert!(recover(&f.root, Some(PASSWORD)).is_err());
        assert!(
            before == image(f.home.path()),
            "untrusted owner recovery changed the synthetic tree"
        );
    }
}

#[test]
#[serial_test::serial]
fn marker_absent_original_reset_keeps_plain_facts_and_authenticated_archive() {
    let f = Fixture::new();
    let original_database = std::fs::read(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let original_metadata = std::fs::read(f.root.join("vault.json")).unwrap();
    let approval = preview(&f.root).unwrap();
    let archive_path = reset(&f.root, &approval.fingerprint, PASSWORD).unwrap();
    assert!(archive_path.is_file());
    assert!(pending_record(&f.root).unwrap().is_none());
    let local = session::read_metadata(&f.root).unwrap();
    let next = VaultContext::from_password(local.metadata, PASSWORD).unwrap();
    let archive: RecoveryArchive =
        serde_json::from_slice(&std::fs::read(&archive_path).unwrap()).unwrap();
    assert!(archive.metadata == *next.metadata());
    let plaintext = next
        .open(&["local", "reset-archive", &archive.id], &archive.content)
        .unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(plaintext.as_slice())).unwrap();
    for (name, expected) in [
        (
            format!("data/{}", crate::config::DB_FILE_NAME),
            original_database,
        ),
        ("data/vault.json".into(), original_metadata),
    ] {
        let mut actual = zeroize::Zeroizing::new(Vec::new());
        zip.by_name(&name)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert!(
            actual.as_slice() == expected,
            "original archive member changed: {name}"
        );
    }
    let conn = snapshot(&f.root).unwrap();
    vault::check_identity(&conn, &next).unwrap();
    inventory::validate_database(&conn, &next).unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM settings WHERE key='theme'", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "dark"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM settings WHERE key='global_proxy_url'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_ne!(next.metadata().vault_id, f.original.metadata().vault_id);
}
