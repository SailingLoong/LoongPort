use super::*;

#[cfg_attr(test, test)]
fn explicit_plaintext_catalog_reuses_original_payload_and_rejects_ciphertext() {
    use crate::secrets::storage::{PlaintextIdentity, StorageContext};
    let plaintext = StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: "11111111-1111-4111-8111-111111111111".into(),
        generation_id: "22222222-2222-4222-8222-222222222222".into(),
        revision: 1,
    });
    let native = NativeCipher::new("synthetic-context", "synthetic-secret").unwrap();
    let catalog = ProfileCatalog::default();
    let bytes = catalog.seal(&plaintext, &native).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&bytes).is_ok());
    assert_eq!(
        ProfileCatalog::open(&bytes, &plaintext, &native)
            .unwrap()
            .len(),
        0
    );
    assert!(ProfileCatalog::open("lpenc:damaged", &plaintext, &native).is_err());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run() {
    explicit_plaintext_catalog_reuses_original_payload_and_rejects_ciphertext();
}
