use super::*;
const PASSWORD: &str = "synthetic-zsb-password";
fn native() -> NativeCipher {
    NativeCipher::new(
        "synthetic-target-context",
        "zcode-credential-fallback:darwin:/synthetic/local:fixture-user",
    )
    .unwrap()
}
#[test]
fn bundle_import_preview_masks_identifiers_and_never_grants_source_or_includes_config() {
    let file = include_bytes!("../../../tests/zcode-bundle-limits/fixtures/valid-multiple.zsb");
    let native = native();
    let preview = inspect(file, PASSWORD, &native, &ProfileCatalog::default()).unwrap();
    assert_eq!(preview.rows.len(), 2);
    assert!(preview.accounts.iter().all(Option::is_some));
    let safe = serde_json::to_string(&preview.rows).unwrap();
    for forbidden in [
        "synthetic-one",
        "synthetic-two",
        "token",
        "credentials",
        "config",
        "enc:v1:",
    ] {
        assert!(!safe.contains(forbidden));
    }
    assert!(preview
        .rows
        .iter()
        .all(|row| row.error.is_none() && !row.duplicate));
    let mut catalog = ProfileCatalog::default();
    catalog.upsert(preview.accounts[0].as_ref().unwrap().clone());
    assert!(inspect(file, PASSWORD, &native, &catalog).unwrap().rows[0].duplicate);
}
#[test]
fn bundle_import_preview_rejects_foreign_inner_key_and_duplicate_identity_per_item() {
    let native = native();
    let foreign =
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/foreign-inner-key.zsb");
    let preview = inspect(foreign, PASSWORD, &native, &ProfileCatalog::default()).unwrap();
    assert!(preview.accounts.iter().all(Option::is_none));
    assert!(preview
        .rows
        .iter()
        .all(|row| row.error == Some("incompatibleCredentials")));
    let duplicate =
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/duplicate-identity.zsb");
    let preview = inspect(duplicate, PASSWORD, &native, &ProfileCatalog::default()).unwrap();
    assert!(preview.accounts[0].is_some());
    assert!(preview.accounts[1].is_some());
    assert!(preview
        .rows
        .iter()
        .all(|row| row.error.is_none() && row.ambiguous));
}
