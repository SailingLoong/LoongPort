//! Native routing policy editor. Client connection remains the original mode owner.
use super::routing_groups::{self as groups, Binding, Group, Member, RouteTarget};
use super::service_source_identity::{self as source_identity, NativeReadGuard};
use crate::mode::state::{SaveRequest, SaveScope};
use crate::{app_config::AppType, database::Database, error::AppError, store::AppState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupDraft {
    pub id: Option<String>,
    pub expected_revision: Option<i64>,
    pub name: String,
    pub members: Vec<Member>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedGroup {
    pub group: Group,
    pub expected_revision: Option<i64>,
    pub draft_digest: String,
    pub affected_app_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedRouteTarget {
    pub target: RouteTarget,
    pub request: SaveRequest,
    pub expected_binding_revision: Option<i64>,
    pub group_revision: Option<i64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedRoutingState {
    pub app_id: String,
    pub binding: Option<Binding>,
    pub groups: Vec<Group>,
    pub can_apply: bool,
    pub blocked_reason: Option<String>,
    pub pending_request: Option<SaveRequest>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequestResult {
    pub request: SaveRequest,
    pub status: String,
}

fn invalid() -> AppError {
    AppError::Config("routing_groups.invalid".into())
}
fn unavailable() -> AppError {
    AppError::Config("routing_groups.unavailable".into())
}
fn conflict() -> AppError {
    AppError::Conflict("routing_groups.conflict".into())
}
fn storage(error: groups::Error) -> AppError {
    match error {
        groups::Error::Conflict => conflict(),
        groups::Error::Invalid => invalid(),
        _ => unavailable(),
    }
}
fn digest(value: &impl Serialize) -> Result<String, AppError> {
    Ok(crate::live::engine::sha256_hex(
        &crate::config::serialize_json_bytes(&serde_json::to_value(value).map_err(|_| invalid())?)?,
    ))
}

/// Read only: callers retain the original app switch lock across this guard
/// and the old writer. Historical authenticated snapshots never gain new tables.
fn has_explicit_binding(db: &Database, app: &AppType) -> Result<bool, AppError> {
    if !app.supports_local_proxy() {
        return Ok(false);
    }
    let vault = db.secret_session().read()?;
    let conn = db.conn.lock()?;
    let version = crate::database::loongport_schema::read_stored_version(&conn)?;
    if version < 30
        && crate::database::loongport_schema::supports_authenticated_snapshot_version(version)
    {
        crate::database::storage::check_identity(&conn, &vault)?;
        crate::secrets::inventory::validate_storage_database(&conn, &vault)?;
        return Ok(false);
    }
    Ok(groups::binding(&conn, app.as_str())
        .map_err(storage)?
        .is_some())
}
fn shared_owner_required() -> AppError {
    AppError::Config("routing_groups.explicit_target: Manage the routing target in Applications. To select a native direct provider, first exit gateway takeover.".into())
}
/// Old auto/order switches do not describe the effective shared routing policy.
pub(crate) fn ensure_legacy_policy_edit_allowed(
    db: &Database,
    app: &AppType,
) -> Result<(), AppError> {
    if has_explicit_binding(db, app)? {
        return Err(shared_owner_required());
    }
    Ok(())
}
/// A retained binding is inactive after an explicit exit. Preserve the original
/// Direct selection owner; detached logical Proxy still uses the shared target.
pub(crate) fn ensure_legacy_selection_allowed(
    db: &Database,
    app: &AppType,
) -> Result<(), AppError> {
    if has_explicit_binding(db, app)? {
        let vault = db.secret_session().read()?;
        if crate::mode::current::validate_known_mode(
            &crate::live::engine::DeviceStore::for_device(),
            &vault,
            app,
        )?
        .is_proxy()
        {
            return Err(shared_owner_required());
        }
    }
    Ok(())
}

fn target_evidence(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    target: &RouteTarget,
    group_revision: Option<i64>,
    native: Option<&NativeReadGuard>,
) -> Result<String, AppError> {
    target.validate().map_err(storage)?;
    let members = match target {
        RouteTarget::Manual { source_id, model } => {
            if group_revision.is_some() {
                return Err(invalid());
            }
            vec![Member {
                source_id: source_id.clone(),
                model: model.clone(),
            }]
        }
        RouteTarget::OrderedGroup { group_id } => {
            let group = groups::read(conn, group_id)
                .map_err(storage)?
                .ok_or_else(unavailable)?;
            if Some(group.revision) != group_revision {
                return Err(conflict());
            }
            group.members
        }
    };
    digest(&(
        target,
        group_revision,
        members_evidence(
            conn,
            vault,
            app,
            &members,
            matches!(target, RouteTarget::OrderedGroup { .. }),
            native,
        )?,
    ))
}
fn members_evidence(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    members: &[Member],
    automatic: bool,
    native: Option<&NativeReadGuard>,
) -> Result<String, AppError> {
    let providers = Database::get_all_providers_on_connection(conn, vault, app.as_str())?;
    let mut facts = Vec::new();
    let mut eligible = 0;
    for member in members {
        if let source_identity::MemberAppCapability::UnsupportedApp(evidence) =
            source_identity::member_app_capability(conn, vault, app, member)?
        {
            if !automatic {
                return Err(unavailable());
            }
            facts.push(serde_json::json!([member, "unsupportedApp", evidence]));
            continue;
        }
        eligible += 1;
        let (provider, source) =
            source_identity::resolve_on(conn, vault, app, &providers, &member.source_id, native)?
                .ok_or_else(unavailable)?;
        let provider = &provider;
        if !crate::proxy::provider_router::provider_supports_captured_model(
            app,
            provider,
            &member.model,
            source.native_stamp.as_ref(),
        ) || source.source_id != member.source_id
            || !source_identity::member_model_supported_on(
                conn,
                app,
                provider,
                &source,
                &member.model,
            )?
            || !crate::proxy::provider_router::provider_supports_proxy_routing(
                app.as_str(),
                provider,
            )
        {
            return Err(unavailable());
        }
        if automatic
            && !crate::proxy::provider_router::provider_supports_failover(app.as_str(), provider)
        {
            return Err(unavailable());
        }
        let provider_digest = Database::provider_update_digest(provider)?;
        // Preserve the relay/other-vendor evidence shape. OpenCode reviews also
        // bind its original key owner; transient session epochs stay outside it.
        let vendor_evidence =
            crate::services::vendor_response_fence::review_evidence_on(conn, provider)
                .map_err(|_| unavailable())?;
        if let Some(local) = source.local_provider {
            if source.native_stamp.is_some() || vendor_evidence.is_some() {
                return Err(invalid());
            }
            facts.push(
                serde_json::to_value((
                    member,
                    provider_digest,
                    ("openrouterLocal", local.stable_evidence),
                ))
                .map_err(|_| invalid())?,
            );
            continue;
        }
        facts.push(
            match (source.native_stamp, vendor_evidence) {
                (Some(stamp), None) => serde_json::to_value((member, provider_digest, stamp)),
                (None, Some(evidence)) => {
                    serde_json::to_value((member, provider_digest, ("opencodeVendor", evidence)))
                }
                (None, None) => serde_json::to_value((member, provider_digest)),
                (Some(_), Some(_)) => return Err(invalid()),
            }
            .map_err(|_| invalid())?,
        );
    }
    if eligible == 0 {
        return Err(unavailable());
    }
    digest(&facts)
}

fn native_for_target<'a>(
    db: &'a Database,
    vault: &crate::secrets::storage::StorageContext,
    conn: &rusqlite::Connection,
    target: &RouteTarget,
) -> Result<Option<NativeReadGuard<'a>>, AppError> {
    let ids = match target {
        RouteTarget::Manual { source_id, .. } => vec![source_id.clone()],
        RouteTarget::OrderedGroup { group_id } => groups::read(conn, group_id)
            .map_err(storage)?
            .ok_or_else(unavailable)?
            .members
            .into_iter()
            .map(|m| m.source_id)
            .collect(),
    };
    source_identity::read_native_for_sources(
        db.secret_session(),
        vault,
        ids.iter().map(String::as_str),
    )
}
fn check_native(native: Option<&NativeReadGuard>) -> Result<(), AppError> {
    if let Some(guard) = native {
        guard.check()?;
    }
    Ok(())
}

fn check_session(db: &Database, epoch: u64) -> Result<(), AppError> {
    db.secret_session().ensure_available()?;
    if db.secret_session().lifecycle_epoch() != epoch {
        return Err(unavailable());
    }
    Ok(())
}

fn group_revision(
    conn: &rusqlite::Connection,
    target: &RouteTarget,
) -> Result<Option<i64>, AppError> {
    match target {
        RouteTarget::Manual { .. } => Ok(None),
        RouteTarget::OrderedGroup { group_id } => Ok(Some(
            groups::read(conn, group_id)
                .map_err(storage)?
                .ok_or_else(unavailable)?
                .revision,
        )),
    }
}
fn known_gateway(state: &AppState, app: &AppType) -> Result<(), AppError> {
    if !crate::mode::controller::PROXY_APPS.contains(app) {
        return Err(AppError::Config("routing_groups.unsupported_app".into()));
    }
    crate::mode::controller::gateway_route_plan(
        &state.proxy_service,
        app,
        &uuid::Uuid::new_v4().to_string(),
    )?;
    Ok(())
}
pub fn read_state(state: &AppState, app: &AppType) -> Result<SharedRoutingState, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let (binding, groups, pending_request, pending) = {
        let vault = state.db.secret_session().read()?;
        let live = crate::mode::state::load_app(
            &crate::live::engine::DeviceStore::for_device(),
            &vault,
            app.as_str(),
        )?;
        let pending = live
            .apps
            .get(app.as_str())
            .and_then(|entry| entry.pending.as_ref());
        let request = pending
            .filter(|pending| pending.target.route_target.is_some())
            .and_then(|pending| pending.target.save_request.clone());
        let conn = state.db.conn.lock()?;
        (
            groups::binding(&conn, app.as_str()).map_err(storage)?,
            groups::list(&conn).map_err(storage)?,
            request,
            pending.is_some(),
        )
    };
    let blocked_reason = if pending {
        Some("routing_groups.recovery_required".into())
    } else {
        known_gateway(state, app).err().map(|error| match error {
            AppError::Config(code) if code.starts_with("routing_groups.") => code,
            _ => "routing_groups.recovery_required".into(),
        })
    };
    Ok(SharedRoutingState {
        app_id: app.as_str().into(),
        binding,
        groups,
        can_apply: blocked_reason.is_none(),
        blocked_reason,
        pending_request,
    })
}
fn lock_apps(state: &AppState) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    crate::mode::controller::PROXY_APPS
        .iter()
        .map(|app| {
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()))
        })
        .collect()
}
fn group_admission(
    vault: &std::sync::RwLockReadGuard<'_, crate::secrets::storage::StorageContext>,
) -> Result<(), AppError> {
    for app in crate::mode::controller::PROXY_APPS {
        let live = crate::mode::state::load_app(
            &crate::live::engine::DeviceStore::for_device(),
            vault,
            app.as_str(),
        )?;
        if live
            .apps
            .get(app.as_str())
            .is_some_and(|entry| entry.pending.is_some())
        {
            return Err(AppError::Config("routing_groups.recovery_required".into()));
        }
    }
    Ok(())
}
fn group_review_on(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    group: &Group,
    expected: Option<i64>,
    native: Option<&NativeReadGuard>,
) -> Result<(Vec<String>, String), AppError> {
    groups::validate(&group.id, &group.members).map_err(storage)?;
    crate::secrets::upgrade::checkpoint::ensure_no_pending_checkpoint(
        &crate::live::engine::DeviceStore::for_device(),
    )?;
    if (expected.is_none() && uuid::Uuid::parse_str(&group.id).is_err())
        || expected.is_some_and(|revision| revision <= 0)
        || group.name.trim().is_empty()
        || group.name.trim() != group.name
        || group.members.is_empty()
        || group.revision != expected.unwrap_or(0).checked_add(1).ok_or_else(invalid)?
    {
        return Err(invalid());
    }
    let current = groups::read(conn, &group.id).map_err(storage)?;
    if current.as_ref().map(|g| g.revision) != expected {
        return Err(conflict());
    }
    let bound = groups::bindings(conn)
        .map_err(storage)?
        .into_iter()
        .filter(
            |(_, b)| matches!(&b.target,RouteTarget::OrderedGroup{group_id} if group_id==&group.id),
        )
        .collect::<Vec<_>>();
    let affected = bound.iter().map(|(app, _)| app.clone()).collect::<Vec<_>>();
    // Validate actual source facts against every affected app; an unused group
    // still must reference known native sources (never a frontend-only inventory).
    let apps = crate::mode::controller::PROXY_APPS;
    let mut source_facts = Vec::new();
    for member in &group.members {
        let mut valid = false;
        for app in &apps {
            if let source_identity::MemberAppCapability::UnsupportedApp(evidence) =
                source_identity::member_app_capability(conn, vault, app, member)?
            {
                source_facts.push((app.as_str(), member.source_id.as_str(), evidence));
                // Capability evidence alone never makes a new/edited group
                // eligible: at least one original supported ready owner must validate.
                continue;
            }
            if let Ok(fact) =
                members_evidence(conn, vault, app, std::slice::from_ref(member), true, native)
            {
                source_facts.push((app.as_str(), member.source_id.as_str(), fact));
                valid = true;
            }
        }
        if !valid {
            return Err(unavailable());
        }
    }
    // A shared identity/order has one owner. Unsupported app capabilities are
    // projected at read time, never persisted as parallel per-app groups.
    for app_name in &affected {
        let app = app_name.parse::<AppType>()?;
        let fact = members_evidence(conn, vault, &app, &group.members, true, native)?;
        source_facts.push((app_name.as_str(), group.id.as_str(), fact));
    }
    let review_digest = digest(&(group, expected, bound, source_facts))?;
    Ok((affected, review_digest))
}
fn group_review(
    state: &AppState,
    group: &Group,
    expected: Option<i64>,
) -> Result<(Vec<String>, String), AppError> {
    let vault = state.db.secret_session().read()?;
    let operation_epoch = state.db.secret_session().lifecycle_epoch();
    group_admission(&vault)?;
    let conn = state.db.conn.lock()?;
    let native = source_identity::read_native_for_sources(
        state.db.secret_session(),
        &vault,
        group.members.iter().map(|m| m.source_id.as_str()),
    )?;
    let result = group_review_on(&conn, &vault, group, expected, native.as_ref())?;
    check_native(native.as_ref())?;
    check_session(&state.db, operation_epoch)?;
    Ok(result)
}
pub fn prepare_group(state: &AppState, draft: GroupDraft) -> Result<PreparedGroup, AppError> {
    let _locks = lock_apps(state);
    if draft.id.is_some() != draft.expected_revision.is_some() {
        return Err(invalid());
    }
    let group = Group {
        id: draft.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: draft.name,
        revision: draft
            .expected_revision
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(invalid)?,
        members: draft.members,
    };
    let (affected_app_ids, draft_digest) = group_review(state, &group, draft.expected_revision)?;
    Ok(PreparedGroup {
        group,
        expected_revision: draft.expected_revision,
        draft_digest,
        affected_app_ids,
    })
}
pub fn confirm_group(state: &AppState, prepared: PreparedGroup) -> Result<Group, AppError> {
    let _locks = lock_apps(state);
    let vault = state.db.secret_session().read()?;
    let operation_epoch = state.db.secret_session().lifecycle_epoch();
    group_admission(&vault)?;
    let mut conn = state.db.conn.lock()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| unavailable())?;
    // Exact state readback is not a second operation receipt.
    if groups::read(&tx, &prepared.group.id)
        .map_err(storage)?
        .as_ref()
        == Some(&prepared.group)
    {
        return Ok(prepared.group);
    }
    let native = source_identity::read_native_for_sources(
        state.db.secret_session(),
        &vault,
        prepared.group.members.iter().map(|m| m.source_id.as_str()),
    )?;
    let (affected, digest) = group_review_on(
        &tx,
        &vault,
        &prepared.group,
        prepared.expected_revision,
        native.as_ref(),
    )?;
    if affected != prepared.affected_app_ids || digest != prepared.draft_digest {
        return Err(conflict());
    }
    crate::mode::operation::failpoint::hit("shared_group:validated")?;
    check_native(native.as_ref())?;
    check_session(&state.db, operation_epoch)?;
    let result = groups::save_named_on(
        &tx,
        &prepared.group.id,
        prepared.expected_revision,
        &prepared.group.name,
        &prepared.group.members,
    )
    .map_err(storage)?;
    check_native(native.as_ref())?;
    check_session(&state.db, operation_epoch)?;
    tx.commit().map_err(|_| unavailable())?;
    Ok(result)
}

fn route_digest(app: &AppType, prepared: &PreparedRouteTarget) -> Result<String, AppError> {
    digest(&(
        "application-route-target-v1",
        app.as_str(),
        &prepared.target,
        prepared.expected_binding_revision,
        prepared.group_revision,
    ))
}
fn route_revision(
    state: &AppState,
    app: &AppType,
    target: &RouteTarget,
    expected_group: Option<i64>,
) -> Result<String, AppError> {
    known_gateway(state, app)?;
    let owner = crate::mode::controller::routing_request_revision_for_service(
        &state.proxy_service,
        app,
        None,
    )?;
    let vault = state.db.secret_session().read()?;
    let operation_epoch = state.db.secret_session().lifecycle_epoch();
    let conn = state.db.conn.lock()?;
    let binding = groups::binding(&conn, app.as_str()).map_err(storage)?;
    let native = native_for_target(&state.db, &vault, &conn, target)?;
    let facts = target_evidence(&conn, &vault, app, target, expected_group, native.as_ref())?;
    check_native(native.as_ref())?;
    check_session(&state.db, operation_epoch)?;
    digest(&(owner, binding, facts))
}
pub fn prepare_target(
    state: &AppState,
    app: &AppType,
    target: RouteTarget,
) -> Result<PreparedRouteTarget, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    known_gateway(state, app)?;
    let (expected_binding_revision, group_revision) = {
        let conn = state.db.conn.lock()?;
        (
            groups::binding(&conn, app.as_str())
                .map_err(storage)?
                .map(|b| b.revision),
            group_revision(&conn, &target)?,
        )
    };
    let request = SaveRequest {
        id: uuid::Uuid::new_v4().to_string(),
        scope: SaveScope::ApplicationRouting,
        provider_id: None,
        draft_digest: String::new(),
        revision: route_revision(state, app, &target, group_revision)?,
    };
    let mut result = PreparedRouteTarget {
        target,
        request,
        expected_binding_revision,
        group_revision,
    };
    result.request.draft_digest = route_digest(app, &result)?;
    result.request.validate()?;
    Ok(result)
}
pub fn query_target(
    state: &AppState,
    app: &AppType,
    request: SaveRequest,
) -> Result<RouteRequestResult, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    query_locked(state, app, request)
}
fn query_locked(
    state: &AppState,
    app: &AppType,
    request: SaveRequest,
) -> Result<RouteRequestResult, AppError> {
    request.validate()?;
    if request.scope != SaveScope::ApplicationRouting {
        return Err(invalid());
    }
    let status = crate::mode::controller::routing_request_status_for_db(&state.db, app, &request)?;
    Ok(RouteRequestResult { request, status })
}

/// Stored inside the original authenticated app-local journal, never a new registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetPublication {
    pub before: Option<Binding>,
    pub planned: Binding,
    pub group_revision: Option<i64>,
    pub source_revision: String,
}
impl TargetPublication {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        self.planned.target.validate().map_err(storage)?;
        if self.planned.revision <= 0
            || self.before.as_ref().is_some_and(|b| b.revision <= 0)
            || self.group_revision.is_some_and(|r| r <= 0)
            || !self
                .source_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        if let Some(before) = &self.before {
            before.target.validate().map_err(storage)?;
        }
        if self.planned.revision
            != self
                .before
                .as_ref()
                .map_or(0, |b| b.revision)
                .checked_add(1)
                .ok_or_else(invalid)?
            || self.source_revision.len() != 64
        {
            return Err(invalid());
        }
        if matches!(self.planned.target, RouteTarget::Manual { .. })
            != self.group_revision.is_none()
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub(crate) fn prepare_publication(
    state: &AppState,
    app: &AppType,
    prepared: &PreparedRouteTarget,
) -> Result<TargetPublication, AppError> {
    prepared.request.validate()?;
    if prepared.request.scope != SaveScope::ApplicationRouting
        || prepared.request.draft_digest != route_digest(app, prepared)?
    {
        return Err(invalid());
    }
    if prepared.request.revision
        != route_revision(state, app, &prepared.target, prepared.group_revision)?
    {
        return Err(conflict());
    }
    let vault = state.db.secret_session().read()?;
    let operation_epoch = state.db.secret_session().lifecycle_epoch();
    let conn = state.db.conn.lock()?;
    let before = groups::binding(&conn, app.as_str()).map_err(storage)?;
    if before.as_ref().map(|b| b.revision) != prepared.expected_binding_revision {
        return Err(conflict());
    }
    let native = native_for_target(&state.db, &vault, &conn, &prepared.target)?;
    let result = TargetPublication {
        planned: Binding {
            target: prepared.target.clone(),
            revision: prepared
                .expected_binding_revision
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(invalid)?,
        },
        before,
        group_revision: prepared.group_revision,
        source_revision: target_evidence(
            &conn,
            &vault,
            app,
            &prepared.target,
            prepared.group_revision,
            native.as_ref(),
        )?,
    };
    check_native(native.as_ref())?;
    check_session(&state.db, operation_epoch)?;
    result.validate()?;
    Ok(result)
}
pub fn apply_target(
    state: &AppState,
    app: &AppType,
    prepared: PreparedRouteTarget,
) -> Result<RouteRequestResult, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    if prepared.request.draft_digest != route_digest(app, &prepared)? {
        return Err(invalid());
    }
    let original = query_locked(state, app, prepared.request.clone())?;
    if !matches!(original.status.as_str(), "unknown" | "notRecorded") {
        return Ok(original);
    }
    let publication = prepare_publication(state, app, &prepared)?;
    let target = crate::mode::state::PendingTarget {
        save_request: Some(prepared.request.clone()),
        route_target: Some(publication),
        ..Default::default()
    };
    let mut plan = crate::mode::controller::gateway_route_plan(
        &state.proxy_service,
        app,
        &prepared.request.id,
    )?;
    plan.target = target;
    crate::mode::controller::consume_row_save(
        &state.proxy_service,
        app,
        plan,
        prepared.request.clone(),
    )?;
    query_locked(state, app, prepared.request)
}
pub fn recover_target(
    state: &AppState,
    app: &AppType,
    request: SaveRequest,
) -> Result<RouteRequestResult, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let current = query_locked(state, app, request.clone())?;
    if current.status == "completed" {
        return Ok(current);
    }
    {
        let vault = state.db.secret_session().read()?;
        let pending = crate::mode::state::pending(
            &crate::live::engine::DeviceStore::for_device(),
            &vault,
            app.as_str(),
        )?
        .ok_or_else(conflict)?;
        if pending.target.save_request.as_ref() != Some(&request)
            || pending.target.route_target.is_none()
        {
            return Err(conflict());
        }
    }
    crate::mode::controller::recover_locked(&state.proxy_service, app)?;
    query_locked(state, app, request)
}
/// Called under the existing vault/app write guards before publish and recovery.
pub(crate) fn verify_publication(
    db: &Database,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    target: &TargetPublication,
    committed: bool,
) -> Result<(), AppError> {
    db.secret_session().ensure_available()?;
    let operation_epoch = db.secret_session().lifecycle_epoch();
    let conn = db.conn.lock()?;
    let native = native_for_target(db, vault, &conn, &target.planned.target)?;
    verify_publication_on(&conn, vault, app, target, committed, native.as_ref())?;
    check_native(native.as_ref())?;
    check_session(db, operation_epoch)
}
fn verify_publication_on(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    target: &TargetPublication,
    committed: bool,
    native: Option<&NativeReadGuard>,
) -> Result<(), AppError> {
    target.validate()?;
    let current = groups::binding(conn, app.as_str()).map_err(storage)?;
    if current.as_ref() != Some(&target.planned) && (committed || current != target.before) {
        return Err(conflict());
    }
    if target_evidence(
        conn,
        vault,
        app,
        &target.planned.target,
        target.group_revision,
        native,
    )? != target.source_revision
    {
        return Err(conflict());
    }
    Ok(())
}
pub(crate) fn publish_target(
    db: &Database,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    target: &TargetPublication,
) -> Result<(), AppError> {
    db.secret_session().ensure_available()?;
    let operation_epoch = db.secret_session().lifecycle_epoch();
    let mut conn = db.conn.lock()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| unavailable())?;
    let native = native_for_target(db, vault, &tx, &target.planned.target)?;
    verify_publication_on(&tx, vault, app, target, false, native.as_ref())?;
    crate::mode::operation::failpoint::hit("route_target:validated")?;
    check_native(native.as_ref())?;
    check_session(db, operation_epoch)?;
    if groups::binding(&tx, app.as_str())
        .map_err(storage)?
        .as_ref()
        == Some(&target.planned)
    {
        return Ok(());
    }
    let actual = groups::set_target_on(
        &tx,
        app.as_str(),
        target.planned.target.clone(),
        target.before.as_ref().map(|b| b.revision),
        target.group_revision,
    )
    .map_err(storage)?;
    if actual != target.planned {
        return Err(conflict());
    }
    check_native(native.as_ref())?;
    check_session(db, operation_epoch)?;
    tx.commit().map_err(|_| unavailable())?;
    Ok(())
}
