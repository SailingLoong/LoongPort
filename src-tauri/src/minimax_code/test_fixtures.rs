use super::catalog::*;

pub(super) fn account() -> Account {
    Account {
        auto_claim_enabled: false,
        auto_claim_revision: 0,
        auto_claim_failure: None,
        id: "synthetic-account".into(),
        label: "Synthetic".into(),
        region: Region::Cn,
        build_env: "prod".into(),
        real_user_id: "synthetic-user".into(),
        auth_generation: 1,
        credential_version: 1,
        credentials: Some(Credentials {
            access_token: "synthetic-access".into(),
            refresh_token: Some("synthetic-refresh".into()),
            kind: CredentialKind::DeviceOAuth,
            native_source: None,
            expires_at: i64::MAX,
        }),
        status: Status::Claimable,
        operation: None,
        last_checked_at: None,
        last_observed_status: Some(Status::Claimable),
    }
}
