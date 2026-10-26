//! Synthetic original MiniMax owner + existing projection; no remote access.
use super::catalog::*;
use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
    store::AppState,
};
use serde_json::json;

pub(crate) async fn verify(state: &AppState, case: &str) {
    access(state.db.secret_session(), None, |document, _| {
        assert!(document.accounts.is_empty());
        document.accounts.push(Account {
            auto_claim_enabled: false,
            auto_claim_revision: 0,
            auto_claim_failure: None,
            id: "synthetic-native-a".into(),
            label: "Synthetic MiniMax".into(),
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
            last_observed_status: None,
        });
        Ok(((), true))
    })
    .await
    .unwrap();
    for (app, id, config) in [
        (
            AppType::Claude,
            "synthetic-claude-projection",
            json!({"env":{"ANTHROPIC_MODEL":"MiniMax-M2.7"}}),
        ),
        (
            AppType::Codex,
            "synthetic-codex-projection",
            json!({"auth":{},"config":"model = \"MiniMax-M2.7\"\n"}),
        ),
    ] {
        let mut provider =
            Provider::with_id(id.into(), "Synthetic native model".into(), config, None);
        provider.category = Some("custom".into());
        provider.meta=Some(serde_json::from_value::<ProviderMeta>(json!({"authBinding":{"source":"managed_account","authProvider":"minimax_code","accountId":"synthetic-native-a"},"apiFormat":"anthropic","commonConfigEnabled":false})).unwrap());
        crate::services::provider::managed_inference::validate(&app, &provider).unwrap();
        state.db.save_provider(app.as_str(), &provider).unwrap();
    }
    let rows = crate::commands::get_service_sources_impl(state).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one original account/model must be one shared source, not omitted or split by app"
    );
    let row = &rows[0];
    assert_eq!(row.bindings.len(), 2);
    assert_ne!(row.bindings[0].provider_id, row.bindings[1].provider_id);
    assert!(row
        .bindings
        .iter()
        .all(|binding| binding.model.as_deref() == Some("MiniMax-M2.7")));
    let dto = serde_json::to_value(row).unwrap();
    assert_eq!(dto["account"]["kind"], "nativeAccount");
    assert_eq!(dto["account"]["platform"], "minimax_code");
    assert_eq!(dto["account"]["region"], "cn");
    assert!(!dto.to_string().contains("synthetic-access"));
    assert!(!dto.to_string().contains("authGeneration"));
    if matches!(case, "duplicate" | "model_changed") {
        let mut providers = state.db.get_all_providers("claude").unwrap();
        let original = providers
            .get("synthetic-claude-projection")
            .unwrap()
            .clone();
        let mut changed = original.clone();
        if case == "duplicate" {
            changed.id = "synthetic-duplicate-projection".into();
        } else {
            changed.settings_config["env"]["ANTHROPIC_MODEL"] = json!("Configured-Other-Model");
        }
        state.db.save_provider("claude", &changed).unwrap();
        providers = state.db.get_all_providers("claude").unwrap();
        let vault = state.db.secret_session().read().unwrap();
        let conn = state.db.conn.lock().unwrap();
        let guard = crate::services::service_source_identity::NativeReadGuard::acquire(
            state.db.secret_session(),
            &vault,
            true,
            false,
            false,
        )
        .unwrap();
        let result = crate::services::service_source_identity::resolve_on(
            &conn,
            &vault,
            &AppType::Claude,
            &providers,
            &row.source_id,
            Some(&guard),
        );
        let rejected = if case == "duplicate" {
            result.is_err()
        } else {
            matches!(result, Ok(None))
        };
        drop(guard);
        drop(conn);
        drop(vault);
        assert!(
            rejected,
            "duplicate projections/changed configured models cannot revive old source intent"
        );
    }
    if case == "epoch" {
        let vault = state.db.secret_session().read().unwrap();
        let guard = crate::services::service_source_identity::NativeReadGuard::acquire(
            state.db.secret_session(),
            &vault,
            true,
            false,
            false,
        )
        .unwrap();
        assert!(guard
            .account("minimax_code", "synthetic-native-a")
            .is_some());
        state.db.secret_session().set_blocked(true);
        state.db.secret_session().set_blocked(false);
        let expired = guard
            .account("minimax_code", "synthetic-native-a")
            .is_none();
        drop(guard); // A failed assertion must not poison the original runtime owner.
        assert!(
            expired,
            "unblocking cannot revive facts captured before an owner lifecycle fence"
        );
    }
}
