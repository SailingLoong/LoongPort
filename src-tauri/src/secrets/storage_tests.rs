use super::*;
fn plain() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: "11111111-1111-4111-8111-111111111111".into(),
        generation_id: "22222222-2222-4222-8222-222222222222".into(),
        revision: 1,
    })
}
#[test]
fn plaintext_is_real_exact_storage_not_an_encrypted_fallback() {
    let c = plain();
    let raw = " synthetic-local-token ";
    assert_eq!(c.seal(&["owner", "field"], raw.as_bytes()).unwrap(), raw);
    assert_eq!(&*c.open(&["owner", "field"], raw).unwrap(), raw.as_bytes());
    assert!(c.is_plaintext());
    assert!(
        matches!(c.encrypted(), Err(AppError::Config(code)) if code == "protection.encryption_required")
    );
}
#[test]
fn plaintext_raw_bytes_are_not_forced_through_utf8() {
    let c = plain();
    let raw = [0, 255, 20, 128];
    assert_eq!(c.encode_bytes(&["file"], &raw).unwrap(), raw);
    assert_eq!(&*c.decode_bytes(&["file"], &raw).unwrap(), &raw);
    assert!(c.seal(&["text"], &raw).is_err());
}
#[test]
fn plaintext_rejects_all_reserved_envelopes_and_empty_identity() {
    let c = plain();
    for text in ["lpenc1.abc", "lpenc2.any", "lpencbroken", "lpenc"] {
        assert!(c.seal(&["owner"], text.as_bytes()).is_err());
        assert!(c.open(&["owner"], text).is_err());
        assert!(c.encode_bytes(&["owner"], text.as_bytes()).is_err());
        assert!(c.decode_bytes(&["owner"], text.as_bytes()).is_err());
    }
    for id in [&[][..], &[""][..], &["", ""][..]] {
        assert!(c.seal(id, b"text").is_err());
        assert!(c.open(id, "text").is_err());
    }
}
#[test]
fn plaintext_identity_is_versioned_canonical_and_nonzero() {
    let StorageContext::Plaintext(v) = plain() else {
        unreachable!()
    };
    v.validate().unwrap();
    for bad in [
        PlaintextIdentity {
            format_version: 2,
            ..v.clone()
        },
        PlaintextIdentity {
            revision: 0,
            ..v.clone()
        },
        PlaintextIdentity {
            store_id: uuid::Uuid::nil().to_string(),
            ..v.clone()
        },
        PlaintextIdentity {
            generation_id: "invalid".into(),
            ..v.clone()
        },
    ] {
        assert!(bad.validate().is_err());
        assert!(StorageContext::Plaintext(bad)
            .open(&["field"], "text")
            .is_err());
    }
    let identity = StorageIdentity::Plaintext(v.clone());
    assert_eq!(identity.store_id(), v.store_id);
    assert_eq!(identity.generation_id(), v.generation_id);
    assert_eq!(identity.revision(), 1);
    let mut json = serde_json::to_value(&v).unwrap();
    json["key"] = "not-accepted".into();
    assert!(serde_json::from_value::<PlaintextIdentity>(json).is_err());
}
#[test]
fn encrypted_codec_and_full_metadata_identity_remain_strict() {
    let vault = VaultContext::generate().unwrap();
    let c = StorageContext::from(vault.clone());
    let text = c.seal(&["a", "b"], b"synthetic-token").unwrap();
    assert!(text.starts_with("lpenc1."));
    assert_eq!(&*c.open(&["a", "b"], &text).unwrap(), b"synthetic-token");
    assert!(c.open(&["other"], &text).is_err());
    assert!(c.open(&["a", "b"], "plain").is_err());
    assert_eq!(
        c.identity(),
        StorageIdentity::Encrypted(vault.metadata().clone())
    );
    assert_eq!(c.identity().store_id(), vault.metadata().vault_id);
}
