//! Source facts for the original authenticated upgrade owner. Schema setup can
//! add defaults, so these facts are read before staging changes the private DB.
use super::*;
use crate::{
    app_config::AppType,
    mode::state::{LiveState, Mode, PreservedState},
    settings::AppSettings,
};
use rusqlite::{Connection, OptionalExtension};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagedAppFacts {
    pub(crate) app_type: String,
    pub(crate) provider_count: usize,
    pub(crate) local_current_present: bool,
    pub(crate) local_current_exists: Option<bool>,
    pub(crate) database_current_count: usize,
    pub(crate) local_matches_database_current: Option<bool>,
    pub(crate) legacy_proxy_enabled: Option<bool>,
    pub(crate) legacy_failover_enabled: Option<bool>,
    pub(crate) saved_mode: Option<Mode>,
    /// None means this app's original state could not be decoded; it must not
    /// be reported as having no operation or be admitted as a fresh default.
    pub(crate) has_pending_operation: Option<bool>,
    pub(crate) live_status: &'static str,
    pub(crate) legacy_takeover_marker: Option<bool>,
    /// Stored-row projection facts, not an effective model or final apply plan.
    /// None remains unresolved; Codex only reports proven declared-field conflicts.
    pub(crate) stored_fields_match: Option<bool>,
    pub(crate) catalog_ownership: Option<&'static str>,
    pub(crate) catalog_file_present: Option<bool>,
    /// Resolution of captured evidence only, never permission to publish. The
    /// original difference card asks only about missing or conflicting facts.
    pub(crate) mode_resolution: &'static str,
    /// The provider in the saved mode: proxy route or direct pointer. No IDs or
    /// decrypted row values leave this authenticated review owner.
    pub(crate) provider_resolution: &'static str,
    pub(crate) requires_mode_choice: bool,
    pub(crate) requires_provider_choice: bool,
    pub(crate) default_action: &'static str,
    pub(crate) default_takeover: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagedUpgradeReview {
    pub(crate) checkpoint_id: String,
    pub(crate) source_versions: SchemaVersions,
    pub(crate) staged_versions: SchemaVersions,
    pub(crate) apps: Vec<StagedAppFacts>,
    pub(crate) can_start_upgrade: bool,
}

pub(super) fn source_facts(
    conn: &Connection,
    vault: &VaultContext,
    settings: &mut AppSettings,
    live: &PreservedState,
    files: &super::live_review::BoundFiles,
    managed_catalog_present: Option<bool>,
) -> Result<Vec<StagedAppFacts>, AppError> {
    let mut facts = Vec::new();
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        let mut query =
            conn.prepare("SELECT id, is_current FROM providers WHERE app_type=?1 ORDER BY id")?;
        let providers = query
            .query_map([app.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let current: Vec<_> = providers
            .iter()
            .filter(|(_, current)| *current)
            .map(|(id, _)| id)
            .collect();
        let local = crate::settings::current_provider_from_settings(settings, &app);
        let flags: Option<(bool, bool)> = conn
            .query_row(
                "SELECT enabled, auto_failover_enabled FROM proxy_config WHERE app_type=?1",
                [app.as_str()],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        // Never infer modern mode from legacy flags, a missing row or a pointer.
        let app_live = live.app_view(app.as_str());
        let mode = app_live
            .as_ref()
            .ok()
            .and_then(|live| crate::mode::current::known_mode_from_state(live, &app).ok());
        let mode_resolution = match &app_live {
            Ok(live) => mode_resolution(live, &app, mode.as_ref()),
            Err(_) => "verification_required",
        };
        let provider_resolution = if mode_resolution == "verification_required" {
            "verification_required"
        } else if mode
            .as_ref()
            .is_some_and(|mode| mode.mode == Some(Mode::Proxy))
        {
            match mode.as_ref().and_then(|mode| mode.proxy_route.as_ref()) {
                Some(id) if providers.iter().any(|(candidate, _)| candidate == id) => "preserved",
                Some(_) => "conflict",
                None => "missing",
            }
        } else {
            match local.as_ref() {
                Some(id)
                    if !providers.iter().any(|(candidate, _)| candidate == id)
                        || current.len() > 1
                        || (current.len() == 1 && current[0] != id) =>
                {
                    "conflict"
                }
                Some(_) => "preserved",
                None => match current.len() {
                    0 => "missing",
                    1 => "preserved",
                    _ => "conflict",
                },
            }
        };
        let rows = Database::get_all_providers_on_connection(conn, vault, app.as_str())?;
        let selected = local
            .as_ref()
            .filter(|id| rows.contains_key(*id))
            .or_else(|| {
                if local.is_none() && current.len() == 1 {
                    Some(current[0])
                } else {
                    None
                }
            });
        let selected = if mode
            .as_ref()
            .is_some_and(|mode| mode.mode == Some(Mode::Proxy) && mode.attached)
        {
            mode.as_ref().and_then(|mode| mode.proxy_route.as_ref())
        } else {
            selected
        };
        let pending = app_live.as_ref().ok().map(|live| {
            live.apps
                .get(app.as_str())
                .is_some_and(|entry| entry.pending.is_some())
        });
        let candidate = selected
            .filter(|_| pending == Some(false))
            .and_then(|id| rows.get(id));
        let client = super::live_review::inspect(&app, files, candidate, managed_catalog_present)?;
        facts.push(StagedAppFacts {
            live_status: client.status,
            legacy_takeover_marker: client.marker,
            stored_fields_match: client.stored_fields_match,
            catalog_ownership: client.catalog_ownership,
            catalog_file_present: client.catalog_file_present,
            app_type: app.as_str().to_owned(),
            mode_resolution,
            provider_resolution,
            requires_mode_choice: matches!(mode_resolution, "missing" | "conflict"),
            requires_provider_choice: matches!(provider_resolution, "missing" | "conflict"),
            default_action: "keep_files",
            default_takeover: false,
            provider_count: providers.len(),
            local_current_present: local.is_some(),
            local_current_exists: local
                .as_ref()
                .map(|id| providers.iter().any(|(candidate, _)| candidate == id)),
            database_current_count: current.len(),
            local_matches_database_current: local
                .as_ref()
                .and_then(|id| (current.len() == 1).then(|| current[0] == id)),
            legacy_proxy_enabled: flags.map(|flags| flags.0),
            legacy_failover_enabled: flags.map(|flags| flags.1),
            saved_mode: mode.and_then(|mode| mode.mode),
            has_pending_operation: pending,
        });
    }
    Ok(facts)
}

fn mode_resolution(
    live: &LiveState,
    app: &AppType,
    known: Option<&crate::mode::state::ModeState>,
) -> &'static str {
    if !live.extra.is_empty() {
        return "verification_required";
    }
    let Some(entry) = live.apps.get(app.as_str()) else {
        return "missing";
    };
    if entry.pending.is_some()
        || !entry.extra.is_empty()
        || !entry.stack.extra.is_empty()
        || entry.stack.enabled
        || entry
            .written
            .as_ref()
            .is_some_and(|written| written.validate().is_err())
    {
        return "verification_required";
    }
    let Some(known) = known else {
        return if entry.mode.is_none()
            && entry.contract.is_none()
            && entry.proxy_route.is_none()
            && !entry.attached
        {
            "missing"
        } else {
            "verification_required"
        };
    };
    if (known.mode == Some(Mode::Direct) && known.attached)
        || (known.mode == Some(Mode::Proxy) && known.attached && known.contract.is_none())
    {
        return "conflict";
    }
    "preserved"
}
