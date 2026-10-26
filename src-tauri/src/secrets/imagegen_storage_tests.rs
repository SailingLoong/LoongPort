use crate::{
    config_file_io as io,
    secrets::{
        self,
        key_store::KeyStore,
        protection::{self, ProtectionRecord},
        session,
        storage::StorageContext,
        testing::{MemoryKeyStore, TestHome},
        VaultContext,
    },
};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use zeroize::Zeroizing;
const TEXT: &str = " Headless 中文\r\n🔑 ";
struct Denied(AtomicUsize);
impl KeyStore for Denied {
    fn load(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, secrets::key_store::KeyStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(secrets::key_store::KeyStoreError::Unavailable)
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), secrets::key_store::KeyStoreError> {
        panic!("MCP never saves a key")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), secrets::key_store::KeyStoreError> {
        panic!("MCP never removes a key")
    }
}
fn fixture(root: &Path, kind: &str, cache: &MemoryKeyStore) -> StorageContext {
    io::ensure_private_directory(root).unwrap();
    let context = if kind == "plaintext" {
        let record = protection::FreshScope::capture(root, root, &[])
            .unwrap()
            .publish_initial_plaintext()
            .unwrap();
        protection::open_context(&record, None, None, |_, _| {
            panic!("plaintext never loads a key")
        })
        .unwrap()
    } else {
        let vault = if kind == "text" {
            VaultContext::create_generation_with_user_text(
                uuid::Uuid::new_v4().to_string(),
                1,
                TEXT,
            )
            .unwrap()
        } else {
            VaultContext::generate().unwrap()
        };
        session::write_metadata(root, &session::completed_metadata(&vault, true).unwrap()).unwrap();
        if kind == "text" {
            io::write_durable(
                &root.join(protection::FILE),
                &serde_json::to_vec(&ProtectionRecord::encrypted(&vault, None).unwrap()).unwrap(),
            )
            .unwrap();
            secrets::key_store::save_user_text_verified(cache, &vault, TEXT).unwrap();
        } else {
            secrets::key_store::save_verified(
                cache,
                &vault.metadata().vault_id,
                &vault.metadata().key_id,
                &vault.export_key(),
            )
            .unwrap();
        }
        StorageContext::from(vault)
    };
    drop(
        crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
            .unwrap(),
    );
    context
}
fn success(kind: &str) {
    let _home = TestHome::new().unwrap();
    let root = crate::config::get_app_config_dir();
    let cache = MemoryKeyStore::default();
    let context = fixture(&root, kind, &cache);
    let db = std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap();
    let denied = Denied(AtomicUsize::new(0));
    let store: &dyn KeyStore = if kind == "plaintext" { &denied } else { &cache };
    let actual = crate::imagegen_mcp_session(store).unwrap();
    assert_eq!(actual.read().unwrap().identity(), context.identity());
    assert_eq!(
        std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap(),
        db
    );
    assert_eq!(denied.0.load(Ordering::SeqCst), 0);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn headless_plaintext_uses_the_selected_storage_owner() {
    success("plaintext");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn headless_user_text_uses_the_same_original_cache_codec() {
    success("text");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn headless_legacy_raw_key_remains_compatible() {
    success("legacy");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn headless_never_initializes_or_repairs_missing_or_pending_storage() {
    for case in ["fresh", "pending", "denied", "record-missing"] {
        let _home = TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        let cache = MemoryKeyStore::default();
        let denied = Denied(AtomicUsize::new(0));
        if case == "fresh" {
            assert!(crate::imagegen_mcp_session(&denied).is_err());
            assert!(!root.exists());
            assert_eq!(denied.0.load(Ordering::SeqCst), 0);
            continue;
        }
        let context = fixture(&root, "text", &cache);
        if case == "pending" {
            io::write_durable(
                &root.join(protection::FILE),
                &serde_json::to_vec(
                    &ProtectionRecord::encrypted(
                        context.encrypted().unwrap(),
                        Some("synthetic unfinished cleanup"),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        }
        if case == "record-missing" {
            std::fs::remove_file(root.join(protection::FILE)).unwrap();
        }
        let db = std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(root.join("vault.json")).unwrap();
        let record = std::fs::read(root.join(protection::FILE)).ok();
        assert!(
            crate::imagegen_mcp_session(if case == "denied" { &denied } else { &cache }).is_err(),
            "{case}"
        );
        assert_eq!(
            std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap(),
            db
        );
        assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), metadata);
        assert_eq!(std::fs::read(root.join(protection::FILE)).ok(), record);
    }
}
#[cfg(feature = "test-hooks")]
pub(crate) fn run(case: &str) {
    match case {
        "plaintext" => headless_plaintext_uses_the_selected_storage_owner(),
        "text" => headless_user_text_uses_the_same_original_cache_codec(),
        "legacy" => headless_legacy_raw_key_remains_compatible(),
        "blocked" => headless_never_initializes_or_repairs_missing_or_pending_storage(),
        _ => panic!("unknown headless storage case"),
    }
}
