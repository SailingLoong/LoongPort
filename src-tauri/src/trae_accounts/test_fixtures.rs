use super::store::*;

pub(super) fn account(id: &str, realm: Realm) -> Account {
    Account {
        id: id.into(),
        uid: "synthetic-user".into(),
        realm,
        generation: 1,
        version: 1,
        device: Device {
            auth_host: realm.auth().into(),
            native_package: None,
            uid: "synthetic-user".into(),
            id: "synthetic-device".into(),
            machine: "synthetic-machine".into(),
            client: "ono9krqynydwx5".into(),
            version: "synthetic-version".into(),
            version_code: "20260917".into(),
            os: "linux".into(),
            os_version: "synthetic-os".into(),
            source_digest: "a".repeat(64),
            private_key: String::new(),
            public_key: String::new(),
        },
        grant: Some(Grant {
            access: "synthetic-access".into(),
            refresh: "synthetic-refresh".into(),
            expires: now() + 3600000,
        }),
        refresh_pending: false,
        checkin_day: None,
        checkin_attempt: None,
        auto_checkin_enabled: false,
        auto_checkin_revision: 0,
        imported_refresh_hash: {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"synthetic-refresh"))
        },
    }
}
