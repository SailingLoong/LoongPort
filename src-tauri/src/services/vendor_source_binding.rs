//! Attach an existing vendor source without selecting it or provisioning keys.
use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta},
    relay::provider_config,
    secrets::{
        inventory::{open_db, seal_db},
        storage::StorageContext,
    },
    vendor::{self, Vendor},
};
use rusqlite::{params, Connection, OptionalExtension};

fn invalid() -> AppError {
    AppError::Config("source.invalid_vendor_binding".into())
}

fn supported(app: &AppType) -> Result<(), AppError> {
    if matches!(app, AppType::Claude | AppType::Codex) {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Validate identity without reading credential columns. Names never establish ownership.
pub(crate) fn identity(
    conn: &Connection,
    source: &str,
) -> Result<(i64, Vendor, String, &'static vendor::PlanInfo), AppError> {
    let mut statement = conn.prepare(
        "SELECT id,vendor_id,account_id FROM loongport_vendor WHERE account_id IS NOT NULL",
    )?;
    let rows = statement.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut found = None;
    for row in rows {
        let (id, vendor_id, account) = row?;
        let Some(vendor) = Vendor::from_id(&vendor_id) else {
            continue;
        };
        for plan in vendor::plans_for_account(vendor, Some(&account)) {
            if vendor::provision::provider_id_for(plan.id_segment, &account) == source {
                if found.is_some() {
                    return Err(invalid());
                }
                found = Some((id, vendor, account.clone(), plan));
            }
        }
    }
    found.ok_or_else(invalid)
}

fn template(
    vendor: Vendor,
    account: &str,
    plan: &vendor::PlanInfo,
    app: &AppType,
) -> Result<Provider, AppError> {
    supported(app)?;
    let (base, model) = vendor::config_for(vendor, plan.id_segment, app).ok_or_else(invalid)?;
    let style = vendor::plan_style(vendor, plan.id_segment);
    let catalog = vendor::catalog_models(vendor, plan.id_segment);
    let mut settings = provider_config::settings_config_with_roles_and_models(
        app,
        "",
        plan.display_name,
        &base,
        &model,
        vendor::provision::claude_roles_for(vendor, plan.id_segment, app),
        Some(&catalog),
        style,
    )
    .ok_or_else(invalid)?;
    vendor::apply_plan_policy(vendor, plan.id_segment, &mut settings);
    let mut provider = Provider::with_id(
        vendor::provision::provider_id_for(plan.id_segment, account),
        plan.display_name.into(),
        settings,
        Some(vendor::site_origin(vendor).into()),
    );
    provider.category = Some("cn_official".into());
    provider.meta = Some(ProviderMeta {
        loongport_vendor_account: Some(account.into()),
        api_format: matches!(app, AppType::Codex).then(|| {
            if style.codex_wire_chat {
                "openai_chat"
            } else {
                "openai_responses"
            }
            .into()
        }),
        ..Default::default()
    });
    Ok(provider)
}

pub(crate) fn is_reference(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|m| m.auth_binding.as_ref())
        .is_some_and(|b| b.source == AuthBindingSource::VendorApiKey)
}

/// Resolve only a validated reference, using the caller's pinned vault and SQLite snapshot.
/// Returned credentials are transient; inventory writes strip them again.
pub(crate) fn resolve_on(
    conn: &Connection,
    vault: &StorageContext,
    app: &AppType,
    provider: &mut Provider,
) -> Result<(), AppError> {
    if !is_reference(provider) {
        return Ok(());
    }
    let (id, _, _) = validate_reference_on(conn, app, provider)?;
    let encrypted: String = conn.query_row(
        "SELECT api_key FROM loongport_vendor WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let key = zeroize::Zeroizing::new(open_db(
        vault,
        "loongport_vendor",
        "api_key",
        &[&id.to_string()],
        &encrypted,
    )?);
    if key.trim().is_empty() {
        return Err(invalid());
    }
    if !provider_config::patch_api_key(&mut provider.settings_config, app, &key) {
        return Err(invalid());
    }
    Ok(())
}

/// The same credential-free reference validation used before original key resolution.
pub(crate) fn validate_reference_on(
    conn: &Connection,
    app: &AppType,
    provider: &Provider,
) -> Result<(i64, Vendor, &'static vendor::PlanInfo), AppError> {
    let (id, vendor, account, plan) = identity(conn, &provider.id)?;
    let expected = template(vendor, &account, plan, app)?;
    let meta = provider.meta.as_ref().ok_or_else(invalid)?;
    let binding = meta.auth_binding.as_ref().ok_or_else(invalid)?;
    let adapter = crate::proxy::providers::get_adapter(app).ok_or_else(invalid)?;
    if binding.account_id.as_deref() != Some(id.to_string().as_str())
        || binding.auth_provider.as_deref() != Some(vendor.vendor_id())
        || meta.loongport_vendor_account.as_deref() != Some(&account)
        || provider.website_url != expected.website_url
        || !meta.custom_endpoints.is_empty()
        || meta.provider_type.is_some()
        || adapter.extract_base_url(provider).map_err(|_| invalid())?
            != adapter.extract_base_url(&expected).map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    // Native Codex reads its selected TOML table, while the proxy also accepts
    // a top-level URL. Both destinations must agree before a credential is read.
    if matches!(app, AppType::Codex) {
        let native_base = |row: &Provider| -> Result<String, AppError> {
            match crate::services::provider::codex_direct::project(row)?.route {
                crate::live::project::codex::Route::Custom { table, .. } => table
                    .get("base_url")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_owned)
                    .ok_or_else(invalid),
                _ => Err(invalid()),
            }
        };
        if native_base(provider)? != native_base(&expected)? {
            return Err(invalid());
        }
    }
    Ok((id, vendor, plan))
}

/// Explicit shared intent may differ from a saved app default, but only for an
/// original OpenCode plan/model/protocol combination evidenced by that owner.
pub(crate) fn shared_model_supported_on(
    conn: &Connection,
    app: &AppType,
    provider: &Provider,
    model: &str,
) -> Result<bool, AppError> {
    if !is_reference(provider) {
        return Ok(false);
    }
    let (_, vendor, _, plan) = identity(conn, &provider.id)?;
    if vendor != Vendor::OpenCode || plan.id_segment != vendor::opencode_personal::GO {
        return Ok(false);
    }
    validate_reference_on(conn, app, provider)?;
    let format = match app {
        AppType::Claude => crate::proxy::providers::get_claude_api_format(provider),
        AppType::Codex
            if crate::proxy::providers::codex_provider_uses_chat_completions(provider) =>
        {
            "openai_chat"
        }
        _ => return Ok(false),
    };
    Ok(
        vendor::opencode_personal::shared_model_protocol_supported(plan.id_segment, model, format)
            && crate::relay::model_catalog::select_model(app, provider, model).is_ok(),
    )
}

pub(crate) fn strip_key(app: &AppType, provider: &Provider) -> Result<serde_json::Value, AppError> {
    let mut settings = provider.settings_config.clone();
    if is_reference(provider) {
        supported(app)?;
        if !provider_config::patch_api_key(&mut settings, app, "") {
            return Err(invalid());
        }
        if matches!(app, AppType::Codex) {
            if let Some(text) = settings.get("config").and_then(serde_json::Value::as_str) {
                settings["config"] = serde_json::Value::String(
                    crate::codex_config::remove_codex_experimental_bearer_token_if(text, |_| true)?,
                );
            }
        }
    }
    Ok(settings)
}

/// SavedRow has no app field; infer only the two supported reference shapes.
pub(crate) fn inventory_settings(provider: &Provider) -> Result<serde_json::Value, AppError> {
    let app = if provider.settings_config.get("config").is_some() {
        AppType::Codex
    } else {
        AppType::Claude
    };
    strip_key(&app, provider)
}

impl Database {
    pub(crate) fn resolve_vendor_auth(
        &self,
        app: &AppType,
        provider: &Provider,
    ) -> Result<Provider, AppError> {
        let mut resolved = provider.clone();
        if is_reference(provider) {
            let vault = self.secret_session().read()?;
            let conn = self.conn.lock()?;
            resolve_on(&conn, &vault, app, &mut resolved)?;
        }
        Ok(resolved)
    }
}

/// One atomic inventory INSERT; deliberately bypasses save_provider's chain append.
/// No activation, native file write, key provisioning or other-app mutation occurs.
pub fn bind_vendor_source(db: &Database, source: &str, app: &AppType) -> Result<bool, AppError> {
    supported(app)?;
    let vault = db.secret_session().read()?;
    let mut conn = db.conn.lock()?;
    let tx = conn.transaction()?;
    let (id, vendor, account, plan) = identity(&tx, source)?;
    let mut statement = tx.prepare("SELECT app_type,meta FROM providers WHERE id=?1")?;
    let rows = statement.query_map([source], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut owned = false;
    for row in rows {
        let (source_app, encrypted) = row?;
        let meta: ProviderMeta = serde_json::from_str(&open_db(
            &vault,
            "providers",
            "meta",
            &[source, &source_app],
            &encrypted,
        )?)
        .map_err(|_| invalid())?;
        if meta.loongport_vendor_account.as_deref() != Some(&account) {
            return Err(invalid());
        }
        owned = true;
    }
    drop(statement);
    if !owned {
        return Err(invalid());
    }
    let mut provider = template(vendor, &account, plan, app)?;
    provider.meta.as_mut().ok_or_else(invalid)?.auth_binding = Some(AuthBinding {
        source: AuthBindingSource::VendorApiKey,
        auth_provider: Some(vendor.vendor_id().into()),
        account_id: Some(id.to_string()),
    });
    resolve_on(&tx, &vault, app, &mut provider)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT meta FROM providers WHERE id=?1 AND app_type=?2",
            params![source, app.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(encrypted) = existing {
        let meta: ProviderMeta = serde_json::from_str(&open_db(
            &vault,
            "providers",
            "meta",
            &[source, app.as_str()],
            &encrypted,
        )?)
        .map_err(|_| invalid())?;
        if meta.loongport_vendor_account.as_deref() != Some(&account) {
            return Err(invalid());
        }
        return Ok(false);
    }
    let settings = seal_db(
        &vault,
        "providers",
        "settings_config",
        &[source, app.as_str()],
        &serde_json::to_string(&strip_key(app, &provider)?).map_err(|_| invalid())?,
    )?;
    let meta = seal_db(
        &vault,
        "providers",
        "meta",
        &[source, app.as_str()],
        &serde_json::to_string(&provider.meta).map_err(|_| invalid())?,
    )?;
    // Missing priority is a legacy read-time projection of all existing rows.
    // Materialize that exact pre-insert projection in this same transaction so
    // an explicitly inactive attachment cannot join the fallback candidates.
    // Existing order (including []) and duplicate bindings remain unchanged.
    let priority_key = crate::proxy::application_routing::priority_key(app.as_str());
    let priority_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key=?1)",
        [&priority_key],
        |row| row.get(0),
    )?;
    if !priority_exists {
        let before = crate::proxy::application_routing::chain_ids_on(&tx, app.as_str())?;
        crate::proxy::application_routing::write_order_on(&tx, app.as_str(), &before)?;
    }
    // This is an original row creation (not a shared consumer read). Initialize
    // its private incarnation in the same insert; subsequent no-op binds do not rotate it.
    tx.execute("INSERT INTO providers(id,app_type,name,settings_config,website_url,category,meta,credential_revision,is_current,in_failover_queue) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,0)",params![source,app.as_str(),provider.name,settings,provider.website_url,provider.category,meta,crate::database::credential_revision::new_revision()])?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider::Provider,
        vendor::{self, Vendor},
    };

    fn fixture() -> (Database, String) {
        let db = Database::memory().unwrap();
        {
            let vault = db.secret_session().read().unwrap();
            let conn = db.conn.lock().unwrap();
            conn.execute("INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(21,'deepseek','synthetic-account')", []).unwrap();
            vendor::creds::set_api_key(&conn, &vault, 21, "SYNTHETIC_OWNER_KEY").unwrap();
        }
        let bundle = vendor::provision::plan_rows_for(
            Vendor::DeepSeek,
            "synthetic-account",
            "SYNTHETIC_OWNER_KEY",
        )
        .remove(0);
        let settings = bundle
            .rows
            .into_iter()
            .find(|(a, _)| *a == AppType::Codex)
            .unwrap()
            .1;
        let mut source = Provider::with_id(
            bundle.provider_id.clone(),
            "Synthetic source".into(),
            settings,
            None,
        );
        source.meta = Some(crate::provider::ProviderMeta {
            loongport_vendor_account: Some("synthetic-account".into()),
            ..Default::default()
        });
        db.save_provider("codex", &source).unwrap();
        (db, bundle.provider_id)
    }

    fn inventory_except_target(
        db: &Database,
        id: &str,
    ) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
        let conn = db.conn.lock().unwrap();
        let tables=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        tables
            .into_iter()
            .map(|table| {
                let sql = if table == "providers" {
                    format!("SELECT * FROM \"{table}\" WHERE NOT(id=?1 AND app_type='claude')")
                } else {
                    format!("SELECT * FROM \"{table}\"")
                };
                let mut statement = conn.prepare(&sql).unwrap();
                let count = statement.column_count();
                let mut rows = if table == "providers" {
                    statement.query([id]).unwrap()
                } else {
                    statement.query([]).unwrap()
                };
                let mut values = vec![];
                while let Some(row) = rows.next().unwrap() {
                    values.push((0..count).map(|i| row.get(i).unwrap()).collect());
                }
                (table, values)
            })
            .collect()
    }

    #[test]
    fn vendor_binding_creates_only_the_requested_inactive_reference() {
        let (db, source) = fixture();
        let before = db.get_all_providers("codex").unwrap();
        let mut inventory = inventory_except_target(&db, &source);
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        // The only non-provider change is materializing the exact pre-insert
        // empty chain. Keep the full snapshot comparison for every other row.
        inventory
            .iter_mut()
            .find(|(table, _)| table == "settings")
            .unwrap()
            .1
            .push(vec![
                rusqlite::types::Value::Text("application_priority_claude".into()),
                rusqlite::types::Value::Text("[]".into()),
            ]);
        assert!(bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
        assert!(!bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
        assert_eq!(
            serde_json::to_value(db.get_all_providers("codex").unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert_eq!(inventory_except_target(&db, &source), inventory);
        assert!(crate::proxy::application_routing::chain_ids(&db, "claude")
            .unwrap()
            .is_empty());
        assert_eq!(db.get_all_providers("claude").unwrap().len(), 1);
        assert!(db.get_all_providers("gemini").unwrap().is_empty());
        assert!(db.get_current_provider("claude").unwrap().is_none());
        let conn = db.conn.lock().unwrap();
        let flags:(bool,bool) = conn.query_row("SELECT is_current,in_failover_queue FROM providers WHERE id=?1 AND app_type='claude'",[&source],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(flags, (false, false));
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key LIKE 'easy_mode_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    fn bound() -> (Database, String, Provider) {
        let (db, id) = fixture();
        bind_vendor_source(&db, &id, &AppType::Claude).unwrap();
        let row = db.get_provider_by_id(&id, "claude").unwrap().unwrap();
        (db, id, row)
    }

    #[test]
    fn vendor_binding_inventory_and_saved_row_never_copy_the_owner_key() {
        let (db, id, row) = bound();
        assert!(!serde_json::to_string(&row)
            .unwrap()
            .contains("SYNTHETIC_OWNER_KEY"));
        let effective = db.resolve_vendor_auth(&AppType::Claude, &row).unwrap();
        assert_eq!(
            provider_config::extract_api_key(&effective.settings_config, &AppType::Claude)
                .as_deref(),
            Some("SYNTHETIC_OWNER_KEY")
        );
        assert!(!Database::provider_update_value(&effective)
            .unwrap()
            .to_string()
            .contains("SYNTHETIC_OWNER_KEY"));
        db.save_provider("claude", &effective).unwrap();
        db.update_provider_settings_config("claude", &id, &effective.settings_config)
            .unwrap();
        let loaded = db.get_provider_by_id(&id, "claude").unwrap().unwrap();
        assert!(!serde_json::to_string(&loaded)
            .unwrap()
            .contains("SYNTHETIC_OWNER_KEY"));
        assert!(is_reference(&loaded));
    }

    #[test]
    fn vendor_binding_output_refreshes_cached_auth_and_fails_closed_without_owner_key() {
        let (db, _, row) = bound();
        let cached = db.resolve_vendor_auth(&AppType::Claude, &row).unwrap();
        {
            let vault = db.secret_session().read().unwrap();
            let conn = db.conn.lock().unwrap();
            vendor::creds::set_api_key(&conn, &vault, 21, "SYNTHETIC_ROTATED_KEY").unwrap();
        }
        let router = crate::proxy::provider_router::ProviderRouter::new(std::sync::Arc::new(db));
        let refreshed = router
            .resolve_vendor_auth(&AppType::Claude, &cached)
            .unwrap();
        assert_eq!(
            provider_config::extract_api_key(&refreshed.settings_config, &AppType::Claude)
                .as_deref(),
            Some("SYNTHETIC_ROTATED_KEY")
        );
    }

    #[test]
    fn vendor_binding_rejects_tampered_ownership_and_destinations() {
        let (db, _, row) = bound();
        for case in 0..8 {
            let mut altered = row.clone();
            match case {
                0 => altered.id = "another-source".into(),
                1 => {
                    altered
                        .meta
                        .as_mut()
                        .unwrap()
                        .auth_binding
                        .as_mut()
                        .unwrap()
                        .account_id = Some("22".into())
                }
                2 => {
                    altered
                        .meta
                        .as_mut()
                        .unwrap()
                        .auth_binding
                        .as_mut()
                        .unwrap()
                        .auth_provider = Some("another-vendor".into())
                }
                3 => {
                    altered.meta.as_mut().unwrap().loongport_vendor_account =
                        Some("another-account".into())
                }
                4 => altered.website_url = Some("https://other.example".into()),
                5 => {
                    altered.settings_config["env"]["ANTHROPIC_BASE_URL"] =
                        serde_json::json!("https://other.example")
                }
                6 => {
                    altered.meta.as_mut().unwrap().custom_endpoints.insert(
                        "https://other.example".into(),
                        crate::settings::CustomEndpoint {
                            url: "https://other.example".into(),
                            added_at: 0,
                            last_used: None,
                        },
                    );
                }
                _ => altered.meta.as_mut().unwrap().provider_type = Some("github_copilot".into()),
            }
            assert!(
                db.resolve_vendor_auth(&AppType::Claude, &altered).is_err(),
                "case {case}"
            );
        }
    }

    #[test]
    fn vendor_binding_unknown_source_unsupported_app_and_missing_key_make_no_row() {
        let (db, id) = fixture();
        assert!(bind_vendor_source(&db, "unknown", &AppType::Claude).is_err());
        assert!(bind_vendor_source(&db, &id, &AppType::Gemini).is_err());
        {
            let vault = db.secret_session().read().unwrap();
            let conn = db.conn.lock().unwrap();
            vendor::creds::set_api_key(&conn, &vault, 21, "").unwrap();
        }
        assert!(bind_vendor_source(&db, &id, &AppType::Claude).is_err());
        assert!(db.get_all_providers("claude").unwrap().is_empty());
    }
    #[test]
    fn vendor_binding_deleted_owner_empty_key_and_locked_vault_do_not_fall_back() {
        for scenario in 0..3 {
            let (db, _, row) = bound();
            let cached = db.resolve_vendor_auth(&AppType::Claude, &row).unwrap();
            if scenario == 2 {
                db.secret_session().set_blocked(true);
            } else {
                let vault = db.secret_session().read().unwrap();
                let conn = db.conn.lock().unwrap();
                if scenario == 0 {
                    conn.execute("DELETE FROM loongport_vendor WHERE id=21", [])
                        .unwrap();
                } else {
                    vendor::creds::set_api_key(&conn, &vault, 21, "").unwrap();
                }
            }
            assert!(
                db.resolve_vendor_auth(&AppType::Claude, &cached).is_err(),
                "scenario {scenario}"
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn vendor_binding_native_output_resolves_without_changing_inventory() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (db, _, row) = bound();
        let state = crate::store::AppState::new(std::sync::Arc::new(db)).unwrap();
        let effective =
            crate::services::provider::build_effective_provider_for_live_with_codex_oauth_manager(
                &state.db,
                &AppType::Claude,
                &row,
                &state.codex_oauth_manager,
            )
            .unwrap();
        assert_eq!(
            provider_config::extract_api_key(&effective.settings_config, &AppType::Claude)
                .as_deref(),
            Some("SYNTHETIC_OWNER_KEY")
        );
        assert!(
            !serde_json::to_string(&state.db.get_all_providers("claude").unwrap())
                .unwrap()
                .contains("SYNTHETIC_OWNER_KEY")
        );
    }
    #[test]
    #[serial_test::serial]
    fn vendor_binding_output_checks_common_config_before_injecting_owner_key() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (db, _, mut row) = bound();
        row.meta.as_mut().unwrap().common_config_enabled = Some(true);
        let state = crate::store::AppState::new(std::sync::Arc::new(db)).unwrap();
        state
            .db
            .set_config_snippet(
                "claude",
                Some(
                    serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://other.example"}})
                        .to_string(),
                ),
            )
            .unwrap();
        assert!(
            crate::services::provider::build_effective_provider_for_live_with_codex_oauth_manager(
                &state.db,
                &AppType::Claude,
                &row,
                &state.codex_oauth_manager
            )
            .is_err()
        );
        state
            .db
            .set_config_snippet(
                "claude",
                Some(
                    serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"SYNTHETIC_SNIPPET_KEY"}})
                        .to_string(),
                ),
            )
            .unwrap();
        let effective =
            crate::services::provider::build_effective_provider_for_live_with_codex_oauth_manager(
                &state.db,
                &AppType::Claude,
                &row,
                &state.codex_oauth_manager,
            )
            .unwrap();
        assert_eq!(
            provider_config::extract_api_key(&effective.settings_config, &AppType::Claude)
                .as_deref(),
            Some("SYNTHETIC_OWNER_KEY")
        );
    }

    #[test]
    fn vendor_binding_output_rejects_codex_top_level_url_masking_native_destination() {
        let (db, _, row) = bound();
        let plan = &vendor::plans(Vendor::DeepSeek)[0];
        let mut codex =
            template(Vendor::DeepSeek, "synthetic-account", plan, &AppType::Codex).unwrap();
        codex.meta.as_mut().unwrap().auth_binding = row.meta.unwrap().auth_binding;
        let (base, _) =
            vendor::config_for(Vendor::DeepSeek, plan.id_segment, &AppType::Codex).unwrap();
        codex.settings_config["base_url"] = serde_json::json!(base);
        let config = codex.settings_config["config"]
            .as_str()
            .unwrap()
            .replace(&base, "https://other.example/v1");
        codex.settings_config["config"] = serde_json::json!(config);
        assert!(db.resolve_vendor_auth(&AppType::Codex, &codex).is_err());
    }
    #[test]
    #[serial_test::serial]
    fn vendor_binding_inactive_broken_reference_does_not_block_another_codex_source() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (db, id) = fixture();
        let working = db.get_provider_by_id(&id, "codex").unwrap().unwrap();
        let mut broken = working.clone();
        broken.id = "loongport-vendor-absent".into();
        broken.meta.as_mut().unwrap().auth_binding = Some(AuthBinding {
            source: AuthBindingSource::VendorApiKey,
            auth_provider: Some("deepseek".into()),
            account_id: Some("999".into()),
        });
        db.save_provider("codex", &broken).unwrap();
        assert!(crate::services::provider::codex_direct::plan(
            &db,
            &crate::services::provider::codex_direct::Owner::None,
            &working
        )
        .is_ok());
    }
    #[test]
    fn vendor_binding_codex_output_projects_owner_key_but_inventory_keeps_reference() {
        let (db, id, _) = bound();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM providers WHERE id=?1 AND app_type='codex'",
                [&id],
            )
            .unwrap();
        assert!(bind_vendor_source(&db, &id, &AppType::Codex).unwrap());
        let row = db.get_provider_by_id(&id, "codex").unwrap().unwrap();
        assert!(!row
            .settings_config
            .to_string()
            .contains("SYNTHETIC_OWNER_KEY"));
        let output = db.resolve_vendor_auth(&AppType::Codex, &row).unwrap();
        match crate::services::provider::codex_direct::project(&output)
            .unwrap()
            .route
        {
            crate::live::project::codex::Route::Custom { table, .. } => assert_eq!(
                table
                    .get("experimental_bearer_token")
                    .and_then(toml_edit::Item::as_str),
                Some("SYNTHETIC_OWNER_KEY")
            ),
            _ => panic!("expected supported vendor route"),
        }
        assert!(!Database::provider_update_value(&output)
            .unwrap()
            .to_string()
            .contains("SYNTHETIC_OWNER_KEY"));
    }

    #[test]
    fn vendor_binding_account_and_plan_identities_keep_their_own_keys() {
        let (db, id, _) = bound();
        let mut ids = vec![id];
        for (row_id, vendor, account, key) in [
            (
                22,
                Vendor::DeepSeek,
                "synthetic-other",
                "SYNTHETIC_OTHER_KEY",
            ),
            (
                23,
                Vendor::OpenCode,
                "synthetic-multi-plan",
                "SYNTHETIC_PLAN_KEY",
            ),
            (
                24,
                Vendor::OpenCode,
                "personal-key:synthetic-owned-credential",
                "SYNTHETIC_PERSONAL_KEY",
            ),
        ] {
            {
                let vault = db.secret_session().read().unwrap();
                let conn = db.conn.lock().unwrap();
                conn.execute(
                    "INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(?1,?2,?3)",
                    params![row_id, vendor.vendor_id(), account],
                )
                .unwrap();
                vendor::creds::set_api_key(&conn, &vault, row_id, key).unwrap();
            }
            for plan in vendor::plans_for_account(vendor, Some(account)) {
                if vendor::is_free_plan(vendor, plan.id_segment) {
                    // Free retains its native source and account identity, but cannot
                    // acquire either external binding even with a valid owner/key.
                    let mut bundle = vendor::provision::plan_rows_for(vendor, account, key)
                        .into_iter()
                        .find(|bundle| bundle.plan.id_segment == plan.id_segment)
                        .unwrap();
                    assert_eq!(bundle.rows.len(), 1);
                    let (native_app, settings) = bundle.rows.remove(0);
                    assert_eq!(native_app, AppType::OpenCode);
                    assert_eq!(
                        provider_config::extract_api_key(&settings, &native_app).as_deref(),
                        Some(key)
                    );
                    let mut source = Provider::with_id(
                        bundle.provider_id,
                        plan.display_name.into(),
                        settings.clone(),
                        Some(vendor::site_origin(vendor).into()),
                    );
                    source.meta = Some(ProviderMeta {
                        loongport_vendor_account: Some(account.into()),
                        ..Default::default()
                    });
                    db.save_provider(native_app.as_str(), &source).unwrap();
                    let (owner, owner_vendor, owner_account, owner_plan) =
                        identity(&db.conn.lock().unwrap(), &source.id).unwrap();
                    assert_eq!(
                        (owner, owner_vendor, owner_account.as_str()),
                        (row_id, vendor, account)
                    );
                    assert_eq!(owner_plan.id_segment, plan.id_segment);
                    for app in [AppType::Claude, AppType::Codex] {
                        assert!(vendor::config_for(vendor, plan.id_segment, &app).is_none());
                        assert!(template(vendor, account, plan, &app).is_err());
                        assert!(bind_vendor_source(&db, &source.id, &app).is_err());
                        assert!(db
                            .get_provider_by_id(&source.id, app.as_str())
                            .unwrap()
                            .is_none());
                    }
                    assert_eq!(
                        db.get_provider_by_id(&source.id, native_app.as_str())
                            .unwrap()
                            .unwrap()
                            .settings_config,
                        settings
                    );
                    continue;
                }
                let source = template(vendor, account, plan, &AppType::Codex).unwrap();
                db.save_provider("codex", &source).unwrap();
                assert!(bind_vendor_source(&db, &source.id, &AppType::Claude).unwrap());
                let row = db
                    .get_provider_by_id(&source.id, "claude")
                    .unwrap()
                    .unwrap();
                let output = db.resolve_vendor_auth(&AppType::Claude, &row).unwrap();
                assert_eq!(
                    provider_config::extract_api_key(&output.settings_config, &AppType::Claude)
                        .as_deref(),
                    Some(key)
                );
                assert!(!ids.contains(&source.id));
                ids.push(source.id);
            }
        }
        assert!(
            ids.len() >= 4,
            "same-account plans must remain separate sources"
        );
        assert_eq!(db.get_all_providers("claude").unwrap().len(), ids.len());
    }
    #[test]
    fn vendor_binding_native_bearer_backfill_does_not_create_an_inventory_copy() {
        let (db, id, row) = bound();
        let plan = &vendor::plans(Vendor::DeepSeek)[0];
        let mut codex =
            template(Vendor::DeepSeek, "synthetic-account", plan, &AppType::Codex).unwrap();
        codex.meta.as_mut().unwrap().auth_binding = row.meta.unwrap().auth_binding;
        let text = codex.settings_config["config"].as_str().unwrap();
        let mut doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        let provider_id = doc["model_provider"].as_str().unwrap().to_owned();
        doc["model_providers"][&provider_id]["experimental_bearer_token"] =
            toml_edit::value("SYNTHETIC_BACKFILL_KEY");
        codex.settings_config["config"] = serde_json::json!(doc.to_string());
        assert!(!Database::provider_update_value(&codex)
            .unwrap()
            .to_string()
            .contains("SYNTHETIC_BACKFILL_KEY"));
        db.save_provider("codex", &codex).unwrap();
        assert!(!db
            .get_provider_by_id(&id, "codex")
            .unwrap()
            .unwrap()
            .settings_config
            .to_string()
            .contains("SYNTHETIC_BACKFILL_KEY"));
        db.update_provider_settings_config("codex", &id, &codex.settings_config)
            .unwrap();
        assert!(!db
            .get_provider_by_id(&id, "codex")
            .unwrap()
            .unwrap()
            .settings_config
            .to_string()
            .contains("SYNTHETIC_BACKFILL_KEY"));
    }

    #[test]
    fn vendor_binding_missing_priority_materializes_only_preinsert_candidates() {
        let (db, source) = fixture();
        let prior = Provider::with_id(
            "prior".into(),
            "Prior route".into(),
            serde_json::json!({"env":{}}),
            None,
        );
        db.save_provider("claude", &prior).unwrap();
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        assert!(bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["prior"]
        );
        assert_eq!(
            db.get_setting("application_priority_claude")
                .unwrap()
                .as_deref(),
            Some("[\"prior\"]")
        );
        assert!(db.get_current_provider("claude").unwrap().is_none());
    }

    #[test]
    fn vendor_binding_existing_and_explicit_empty_priorities_remain_exact() {
        for order in ["[]", "[\"prior-b\",\"prior-a\"]"] {
            let (db, source) = fixture();
            for id in ["prior-a", "prior-b"] {
                db.save_provider(
                    "claude",
                    &Provider::with_id(id.into(), id.into(), serde_json::json!({"env":{}}), None),
                )
                .unwrap();
            }
            db.set_setting("application_priority_claude", order)
                .unwrap();
            assert!(bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
            assert_eq!(
                db.get_setting("application_priority_claude")
                    .unwrap()
                    .as_deref(),
                Some(order)
            );
        }
    }

    #[test]
    fn vendor_binding_duplicate_does_not_repair_or_mutate_missing_priority() {
        let (db, source) = fixture();
        assert!(bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM settings WHERE key='application_priority_claude'",
                [],
            )
            .unwrap();
        let before = serde_json::to_value(db.get_all_providers("claude").unwrap()).unwrap();
        assert!(!bind_vendor_source(&db, &source, &AppType::Claude).unwrap());
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        assert_eq!(
            serde_json::to_value(db.get_all_providers("claude").unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn vendor_binding_failed_insert_rolls_back_missing_priority_materialization() {
        let (db, source) = fixture();
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        db.conn.lock().unwrap().execute_batch("CREATE TEMP TRIGGER synthetic_binding_insert_failure BEFORE INSERT ON providers WHEN NEW.app_type='claude' BEGIN SELECT RAISE(ABORT,'synthetic insert failure'); END;").unwrap();
        assert!(bind_vendor_source(&db, &source, &AppType::Claude).is_err());
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        assert!(db.get_all_providers("claude").unwrap().is_empty());
        assert!(db.get_provider_by_id(&source, "codex").unwrap().is_some());
    }

    #[test]
    fn vendor_binding_invalid_source_never_materializes_missing_priority() {
        let (db, _) = fixture();
        assert!(bind_vendor_source(&db, "not-a-source", &AppType::Claude).is_err());
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
        assert!(db.get_all_providers("claude").unwrap().is_empty());
    }
}
