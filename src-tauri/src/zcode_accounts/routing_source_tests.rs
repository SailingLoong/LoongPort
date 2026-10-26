//! Explicit synthetic ordinary-session checks. No login, network or consent changes.
use super::*;
use crate::services::service_source_identity::NativeStamp;

pub(crate) async fn verify(db: Arc<Database>, case: &str) {
    let session = db.secrets.clone();
    assert!(matches!(
        case,
        "facts_read_only"
            | "stamp_mismatch"
            | "capability_missing_drop"
            | "credential_format"
            | "facts_expired_read_only"
    ));
    let owner = FixtureTransport::seed_accounts(&session).await.unwrap();
    if case == "capability_missing_drop" {
        let vault = session.read().unwrap();
        assert!(fixture::routing_context(&session, &vault).is_err());
        drop(vault);
        let capability = owner.install_routing_capability(&session).unwrap();
        assert!(owner.install_routing_capability(&session).is_err());
        {
            let vault = session.read().unwrap();
            assert!(fixture::routing_context(&session, &vault).is_ok());
            let other = SecretSession::ephemeral().unwrap();
            let other_vault = other.read().unwrap();
            assert!(fixture::routing_context(&other, &other_vault).is_err());
        }
        drop(capability);
        let vault = session.read().unwrap();
        assert!(fixture::routing_context(&session, &vault).is_err());
        return;
    }
    let _capability = owner.install_routing_capability(&session).unwrap();
    let profile_path = session.root().join(super::super::checkpoint::PROFILE_FILE);
    if matches!(case, "credential_format" | "facts_expired_read_only") {
        // Modify only this generated fixture's already encrypted local image.
        // Deliberately bypass StartJwt's format constructor to represent a
        // decryptable but invalid stored value, not a missing authorization.
        let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = session.read().unwrap();
        let context = fixture::routing_context(&session, &vault).unwrap();
        let native = context.cipher().unwrap();
        let store = VaultAccountStore::new(session.root(), &vault).unwrap();
        let status = store.catalog_status(&native).unwrap();
        let mut catalog = store.import_catalog(&native, &status.revision).unwrap();
        let snapshot = catalog
            .profiles()
            .find(|s| s.identity().opaque_id() == owner.account_id(0))
            .unwrap()
            .clone();
        let mut document: serde_json::Value =
            serde_json::from_slice(&snapshot.scoped_document().to_bytes().unwrap()).unwrap();
        let token = if case == "credential_format" {
            "synthetic\ninvalid-control".to_owned()
        } else {
            format!(
                "e30.{}.synthetic",
                URL_SAFE_NO_PAD.encode(r#"{"exp":1,"fixture":"expired-local-only"}"#)
            )
        };
        document[snapshot.identity().credential_keys()[4].as_str()] =
            native.encrypt(&token).unwrap().into();
        let document =
            super::super::core::CredentialDocument::parse(&serde_json::to_vec(&document).unwrap())
                .unwrap();
        catalog.upsert(AccountSnapshot::capture(snapshot.identity().clone(), &document).unwrap());
        crate::config_file_io::write_durable(
            &profile_path,
            catalog.seal(&vault, &native).unwrap().as_bytes(),
        )
        .unwrap();
    }
    let before = std::fs::read(&profile_path).unwrap();
    let (context, facts) = {
        let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = session.read().unwrap();
        let context = fixture::routing_context(&session, &vault).unwrap();
        let result = routing_source::facts_in_context(&session, &vault, &context);
        if case == "credential_format" {
            assert!(
                result.is_err(),
                "malformed stored credential must fail closed, not become skippable absence"
            );
            assert_eq!(std::fs::read(&profile_path).unwrap(), before);
            return;
        }
        let (facts, unavailable) = result.unwrap();
        if case == "facts_expired_read_only" {
            assert_eq!(facts.len(), 1);
            assert_eq!(unavailable.len(), 1);
            assert_eq!(unavailable[0].account_id, owner.account_id(0));
            assert_eq!(unavailable[0].reason, crate::services::service_source_identity::LocalNativeUnavailable::CredentialsExpired);
            assert_eq!(std::fs::read(&profile_path).unwrap(), before);
            return;
        }
        assert!(unavailable.is_empty());
        assert_eq!(facts.len(), 2);
        (context, facts)
    };
    assert_eq!(std::fs::read(&profile_path).unwrap(), before);
    let stamp = facts
        .into_iter()
        .find_map(|fact| match fact.stamp {
            NativeStamp::ZCode(stamp) if stamp.account_id == owner.account_id(0) => Some(stamp),
            _ => None,
        })
        .expect("original selected account has a routing fact");
    let lease =
        Lease::acquire_in_context(db, context, owner.account_id(0), "routing-owner-fixture")
            .await
            .unwrap();
    lease.verify_routing_stamp(&stamp).unwrap();
    if case == "facts_read_only" {
        let serialized = serde_json::to_string(&stamp).unwrap();
        assert!(!serialized.contains(owner.synthetic_token(0)));
        assert!(!serialized.contains("session_epoch"));
        assert!(!serialized.contains("catalog_revision"));
        assert_eq!(std::fs::read(&profile_path).unwrap(), before);
        lease.check().await.unwrap();
    } else {
        for field in 0..8 {
            let mut changed = stamp.clone();
            match field {
                0 => changed.account_id.push('x'),
                1 => changed.context_id.push('x'),
                2 => changed.vault_id.push('x'),
                3 => changed.session_binding.push('x'),
                4 => match &mut changed.storage {
                    crate::secrets::storage::StorageIdentity::Encrypted(meta) => {
                        meta.key_id.push('x')
                    }
                    crate::secrets::storage::StorageIdentity::Plaintext(meta) => {
                        meta.generation_id.push('x')
                    }
                },
                5 => match &mut changed.storage {
                    crate::secrets::storage::StorageIdentity::Encrypted(meta) => {
                        meta.revision = meta.revision.wrapping_add(1)
                    }
                    crate::secrets::storage::StorageIdentity::Plaintext(meta) => {
                        meta.revision = meta.revision.wrapping_add(1)
                    }
                },
                6 => changed.catalog_revision.push('x'),
                _ => changed.session_epoch = changed.session_epoch.wrapping_add(1),
            }
            assert!(
                matches!(lease.verify_routing_stamp(&changed), Err(ProxyError::Internal(_))),
                "mismatched field {field} must be terminal, not an auth error eligible for another account"
            );
        }
        lease.verify_routing_stamp(&stamp).unwrap();
        assert_eq!(std::fs::read(&profile_path).unwrap(), before);
    }
}
