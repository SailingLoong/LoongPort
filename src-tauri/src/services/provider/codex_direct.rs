//! Fixed upstream v4.0.2 Codex direct/proxy writer: prepare → plan → run.
//! LoongPort adapts native auth placement, encrypted device stash and conservative
//! recovery. Direct and route-mode callers borrow the same five-file writer.
use super::{
    codex_login::{self, AuthInput, AuthTarget, LoginStash, STASH_FILENAME},
    ProviderService,
};
use crate::config::serialize_json_bytes as sorted_json_bytes;
use crate::live::{
    engine::{digest, read_current, DeviceStore, LiveFile},
    patch::toml::{value_text, TomlSteps},
    patch::{Guarded, LivePatch, WholeFile},
    project::codex::{
        official_mirror_table, proxy_route_table, row_catalog_pointer, CodexConfigPatch,
        CodexProjection, KnownTable, Route, RouteAuth, RouteWrite, RowInput, MODEL_CATALOG_JSON,
        ROUTE_ID, WEB_SEARCH_DISABLED,
    },
};
use crate::mode::{
    contract::CONTRACT_VERSION,
    operation::{self, AppWrite, FileChange, OperationReport, RecoveryOutcome},
    state::{self, Contract, PendingTarget},
};
use crate::proxy::providers::codex_oauth_auth::{CodexLiveAuthGuard, CodexOAuthManager};
use crate::secrets::owned_file::DeviceFile;
use crate::{
    app_config::AppType, codex_config::*, database::Database, error::AppError, provider::Provider,
    services::ProxyService, store::AppState,
};
use serde_json::{Map, Value};
use std::sync::Arc;
use toml_edit::{Item, Table, Value as TomlValue};

pub(crate) const CATALOG_PRESERVED_WARNING: &str = "保留外部模型目录；LoongPort 模型映射未生效 (External model catalog preserved; LoongPort model mapping was not applied)";

fn app() -> &'static str {
    "codex"
}
/// 官方卡：`category == "official"`，或按 `is_codex_official_provider` 认出来的（早期
/// 绑定托管账号时没存 category 的卡）。
pub(crate) fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official")
        || crate::proxy::providers::is_codex_official_provider(provider)
}

pub(crate) fn managed_account(provider: &Provider) -> Option<String> {
    ProviderService::managed_codex_oauth_account_id(provider)
}

#[derive(Clone, Copy)]
pub(crate) enum Target<'a> {
    Direct(Option<&'a Provider>),
    Proxy {
        route: &'a Provider,
        base_url: &'a str,
    },
}
impl<'a> Target<'a> {
    fn provider(&self) -> Option<&'a Provider> {
        match self {
            Self::Direct(provider) => *provider,
            Self::Proxy { route, .. } => Some(route),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Owner<'a> {
    Provider(&'a Provider),
    Contract {
        contract: &'a Contract,
        route: Option<&'a Provider>,
    },
    None,
}
impl<'a> Owner<'a> {
    fn provider(&self) -> Option<&'a Provider> {
        match self {
            Self::Provider(p) => Some(p),
            Self::Contract { route, .. } => *route,
            Self::None => None,
        }
    }
}
#[derive(Default)]
pub(crate) struct Prepared {
    target_login: Option<(String, Value)>,
    outgoing: Option<(String, Option<String>)>,
    auth_pre: Option<String>,
}
pub(crate) fn prepare(
    manager: &Arc<CodexOAuthManager>,
    owner: &Owner<'_>,
    target: &Provider,
) -> Result<Prepared, AppError> {
    prepare_target(manager, owner, &Target::Direct(Some(target)))
}
fn prepare_target(
    manager: &Arc<CodexOAuthManager>,
    owner: &Owner<'_>,
    target: &Target<'_>,
) -> Result<Prepared, AppError> {
    let auth_before = read_current(&get_codex_auth_path())?;
    if let Some(bytes) = &auth_before {
        let auth: Value = parse_json(bytes)?;
        if !auth.is_object() {
            return Err(invalid());
        }
    }
    let target_account = target
        .provider()
        .filter(|p| is_official(p))
        .and_then(managed_account);
    let target_login = target_account
        .as_ref()
        .map(|account| {
            super::live::prepare_codex_managed_oauth_live_auth_value(
                manager.clone(),
                account.clone(),
            )
            .map(|login| (account.clone(), login))
        })
        .transpose()?;
    let outgoing = owner
        .provider()
        .and_then(managed_account)
        .filter(|account| Some(account) != target_account.as_ref())
        .map(|account| {
            super::live::prepare_codex_managed_oauth_live_auth_switch_away(
                manager.clone(),
                account.clone(),
            )
            .map(|guard| (account, guard))
        })
        .transpose()?;
    let auth_pre = digest(auth_before.as_deref());
    if digest(read_current(&get_codex_auth_path())?.as_deref()) != auth_pre {
        return Err(invalid());
    }
    Ok(Prepared {
        target_login,
        outgoing,
        auth_pre,
    })
}
pub(crate) fn project(provider: &Provider) -> Result<CodexProjection, AppError> {
    CodexProjection::of(&RowInput {
        settings: &provider.settings_config,
        official: is_official(provider),
        proxy_injected_oauth: (provider.is_xai_oauth() || provider.is_github_copilot()),
    })
}

/// The pure row projector recognizes catalog names; actual writes also require
/// the original catalog owner's directory containment proof.
fn external_catalog<'a>(
    doc: &'a toml_edit::DocumentMut,
    base: &std::path::Path,
) -> Option<&'a TomlValue> {
    let value = doc.get(MODEL_CATALOG_JSON)?.as_value()?;
    value.as_str()?;
    resolve_cc_switch_catalog_path(&doc.to_string(), base)
        .is_none()
        .then_some(value)
}

fn project_for_write(provider: &Provider) -> Result<CodexProjection, AppError> {
    let mut projection = project(provider)?;
    if row_catalog_pointer(&projection.top).is_none() {
        let text = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        // Parsing was validated by the original projector above.
        if let Ok(doc) = text.parse::<toml_edit::DocumentMut>() {
            if let Some(pointer) = external_catalog(&doc, &get_codex_config_dir()) {
                let mut pointer = pointer.clone();
                pointer.decor_mut().clear();
                projection.top.push((MODEL_CATALOG_JSON.into(), pointer));
            }
        }
    }
    Ok(projection)
}

/// 这个供应商的独有字段，含 `web_search`（需要时为 `"disabled"`）。
pub(crate) fn exclusive_of(
    provider: &Provider,
    projection: &CodexProjection,
) -> Vec<(String, TomlValue)> {
    let mut exclusive = projection.exclusive.clone();
    let profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(provider);
    if codex_disables_web_search(
        &provider.settings_config,
        &projection.catalog_input_text(),
        profile,
    ) {
        exclusive.retain(|(key, _)| key != "web_search");
        exclusive.push((
            "web_search".to_string(),
            TomlValue::from(WEB_SEARCH_DISABLED),
        ));
    }
    exclusive
}

pub(crate) fn outgoing_exclusive(owner: &Owner<'_>) -> Vec<(String, TomlValue)> {
    match owner {
        Owner::Provider(provider) => match project(provider) {
            Ok(projection) => exclusive_of(provider, &projection),
            Err(err) => {
                log::warn!(
                    "无法投影 Codex 供应商 {} 的独有字段，切走时不清理它们: {err}",
                    provider.id
                );
                Vec::new()
            }
        },
        Owner::Contract { contract, .. } => contract
            .exclusive
            .iter()
            .filter_map(|(key, value)| {
                Some((key.clone(), value.as_str()?.parse::<TomlValue>().ok()?))
            })
            .collect(),
        Owner::None => Vec::new(),
    }
}

struct RowFacts {
    retired: Vec<KnownTable>,
    third_party_keys: Vec<String>,
    official_logins: Vec<Value>,
}

fn row_facts(db: &Database) -> Result<RowFacts, AppError> {
    Ok(row_facts_from_providers(
        db.get_all_providers(app())?.values(),
    ))
}

fn row_facts_from_providers<'a>(providers: impl Iterator<Item = &'a Provider>) -> RowFacts {
    let mut facts = RowFacts {
        retired: Vec::new(),
        third_party_keys: Vec::new(),
        official_logins: Vec::new(),
    };
    for provider in providers {
        let auth = provider.settings_config.get("auth");
        if is_official(provider) {
            if managed_account(provider).is_none() {
                if let Some(auth) =
                    auth.filter(|auth| codex_auth_has_credential_login_material(auth))
                {
                    facts.official_logins.push(auth.clone());
                }
            }
            continue;
        }
        if let Some(key) = auth.and_then(extract_codex_auth_api_key) {
            facts.third_party_keys.push(key);
        }
        let Some(doc) = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        else {
            continue;
        };
        let providers = doc.get("model_providers").and_then(Item::as_table_like);
        let base_url_of = |id: &str| {
            providers
                .and_then(|table| table.get(id))
                .and_then(Item::as_table_like)
                .and_then(|table| table.get("base_url"))
                .and_then(Item::as_str)
                .map(|url| url.trim().to_string())
        };
        // 旧版整份写入时，路由表用的是行自己的 id（custom 是 CC Switch 现在写的，不算）。
        let selector = doc
            .get("model_provider")
            .and_then(Item::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty() && *id != ROUTE_ID);
        if let Some((id, base_url)) = selector.and_then(|id| Some((id, base_url_of(id)?))) {
            facts.retired.push(KnownTable {
                id: id.to_string(),
                base_url,
            });
        }
        if let Some(base_url) = doc
            .get("openai_base_url")
            .and_then(Item::as_str)
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
        {
            facts.retired.push(KnownTable {
                id: "cc-switch".to_string(),
                base_url,
            });
        }
    }
    facts
}

fn row_auth(provider: &Provider) -> Value {
    provider
        .settings_config
        .get("auth")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

pub(crate) struct Planned {
    config: CodexConfigPatch,
    catalog: Option<Vec<u8>>,
    auth: Value,
    official: bool,
    keep_native: bool,
    stamp: Option<RouteAuth>,
    leaving_official: Option<Value>,
    outgoing_catalog: Option<String>,
    facts: RowFacts,
}

/// Planning: original upstream projection and outgoing ownership, with the
/// existing LoongPort catalog capability owner supplying generated bytes.
pub(crate) fn plan(
    db: &Database,
    owner: &Owner<'_>,
    provider: &Provider,
) -> Result<Planned, AppError> {
    let projection = project_for_write(provider)?;
    let unify = crate::settings::unify_codex_session_history();
    let endpoint = if is_official(provider) && !unify {
        let config = crate::rt::block_on(db.get_global_proxy_config())?;
        Some((config.listen_address, config.listen_port))
    } else {
        None
    };
    let route = direct_route(&projection, unify, endpoint.as_ref())?;
    plan_projected(owner, provider, projection, route, row_facts(db)?)
}

fn direct_route(
    projection: &CodexProjection,
    unify: bool,
    endpoint: Option<&(String, u16)>,
) -> Result<(RouteWrite, Option<RouteAuth>), AppError> {
    let (route, stamp) = match &projection.route {
        Route::Official if unify => (RouteWrite::OfficialMirror, None),
        Route::Official => {
            let (address, port) = endpoint.ok_or_else(invalid)?;
            let address = if address.contains(':') {
                format!("[{address}]")
            } else {
                address.clone()
            };
            (
                RouteWrite::Official {
                    dormant_base_url: format!("http://{address}:{port}/v1"),
                },
                None,
            )
        }
        Route::Custom { table, auth } => (RouteWrite::Custom(table.clone()), Some(*auth)),
        Route::BuiltIn { id, table } => (
            RouteWrite::BuiltIn {
                id: id.clone(),
                table: table.clone(),
            },
            None,
        ),
        Route::Default => (RouteWrite::Default, None),
    };
    Ok((route, stamp))
}

fn plan_projected(
    owner: &Owner<'_>,
    provider: &Provider,
    projection: CodexProjection,
    (route, stamp): (RouteWrite, Option<RouteAuth>),
    facts: RowFacts,
) -> Result<Planned, AppError> {
    let official = is_official(provider);
    let catalog = plan_codex_model_catalog(
        &provider.settings_config,
        &projection.catalog_input_text(),
        crate::proxy::providers::resolve_codex_catalog_tool_profile(provider),
        provider,
    )?
    .map(|v| sorted_json_bytes(&v))
    .transpose()?;
    let config = CodexConfigPatch {
        top: projection.top.clone(),
        nested: projection.nested.clone(),
        exclusive: exclusive_of(provider, &projection),
        outgoing: outgoing_exclusive(owner),
        route,
        catalog: catalog.is_some(),
        retired: facts.retired.clone(),
    };
    let outgoing_catalog = owner
        .provider()
        .and_then(|provider| project_for_write(provider).ok())
        .and_then(|projection| {
            row_catalog_pointer(&projection.top)
                .and_then(|(_, value)| value.as_str())
                .map(str::to_owned)
        });
    Ok(Planned {
        config,
        catalog,
        auth: row_auth(provider),
        official,
        keep_native: false,
        stamp,
        facts,
        outgoing_catalog,
        leaving_official: owner
            .provider()
            .filter(|p| is_official(p) && managed_account(p).is_none())
            .map(row_auth),
    })
}

fn plan_target(db: &Database, owner: &Owner<'_>, target: &Target<'_>) -> Result<Planned, AppError> {
    let mut planned = match target.provider() {
        Some(provider) => plan(db, owner, provider)?,
        None => {
            let facts = row_facts(db)?;
            Planned {
                config: CodexConfigPatch {
                    top: Vec::new(),
                    nested: Vec::new(),
                    exclusive: Vec::new(),
                    outgoing: outgoing_exclusive(owner),
                    route: RouteWrite::Default,
                    catalog: false,
                    retired: facts.retired.clone(),
                },
                catalog: None,
                auth: Value::Object(Map::new()),
                official: false,
                keep_native: true,
                stamp: None,
                leaving_official: owner
                    .provider()
                    .filter(|p| is_official(p) && managed_account(p).is_none())
                    .map(row_auth),
                outgoing_catalog: owner
                    .provider()
                    .and_then(|p| project_for_write(p).ok())
                    .and_then(|p| {
                        row_catalog_pointer(&p.top)
                            .and_then(|(_, v)| v.as_str())
                            .map(str::to_owned)
                    }),
                facts,
            }
        }
    };
    if let Target::Proxy { base_url, .. } = target {
        apply_proxy_projection(
            &mut planned,
            base_url,
            crate::settings::unify_codex_session_history(),
        );
    }
    Ok(planned)
}

fn apply_proxy_projection(planned: &mut Planned, base_url: &str, unify: bool) {
    if planned.official {
        planned.config.route = RouteWrite::OfficialProxy {
            base_url: base_url.into(),
            unified: unify,
        };
        planned.stamp = None;
    } else {
        planned.config.route = RouteWrite::Custom(proxy_route_table(ROUTE_ID, base_url, false));
        planned.stamp = Some(RouteAuth::Bearer);
        planned.keep_native = true;
    }
}

/// The controller borrows the existing manager and five-file writer. Preparation
/// precedes the pinned vault, and the manager guard encloses intent publication.
pub(crate) fn apply_mode(
    service: &ProxyService,
    owner: &Owner<'_>,
    desired: Target<'_>,
    operation: &str,
    mut target: PendingTarget,
) -> Result<bool, AppError> {
    crate::rt::block_on(
        service
            .codex_manager()
            .with_live_auth_guard(&[], |generation| {
                let write = AppWrite::begin_codex_mode(service, generation)?;
                read_inputs(&write, &[]).map(|_| ())
            }),
    )?;
    let prepared = prepare_target(service.codex_manager(), owner, &desired)?;
    let planned = plan_target(service.database(), owner, &desired)?;
    if matches!(desired, Target::Proxy { .. }) {
        target.state.as_mut().ok_or_else(invalid)?.contract = Some(contract_for_plan(
            &desired,
            &planned,
            prepared
                .target_login
                .as_ref()
                .map(|(account, _)| account.as_str()),
        ));
    }
    let ids = prepared
        .target_login
        .iter()
        .map(|(id, _)| id.clone())
        .chain(prepared.outgoing.iter().map(|(id, _)| id.clone()))
        .collect::<Vec<_>>();
    crate::rt::block_on(
        service
            .codex_manager()
            .with_live_auth_guard(&ids, |generation| {
                if let Some((account, auth)) = &prepared.target_login {
                    if !generation.matches_prepared(account, auth) {
                        return Err(invalid());
                    }
                }
                let write = AppWrite::begin_codex_mode(service, generation)?;
                run_pinned(
                    &write,
                    planned,
                    &prepared,
                    desired.provider(),
                    None,
                    operation,
                    target,
                )
                .map(|(_, preserved)| preserved)
            }),
    )
}

fn contract_for_plan(target: &Target<'_>, planned: &Planned, managed: Option<&str>) -> Contract {
    let login = planned
        .official
        .then(|| codex_login::official_login_requirement(&planned.auth))
        .flatten();
    contract_of(
        target,
        &planned.config,
        planned.catalog.as_deref(),
        managed,
        login.as_deref(),
    )
}

/// Pure expected contract for listener repair admission. No live auth read,
/// token preparation/refresh, vault publication or client file write occurs.
pub(crate) fn planned_proxy_contract(
    db: &Database,
    owner: &Owner<'_>,
    route: &Provider,
    base_url: &str,
) -> Result<Contract, AppError> {
    let desired = Target::Proxy { route, base_url };
    let planned = plan_target(db, owner, &desired)?;
    let managed = desired
        .provider()
        .filter(|provider| is_official(provider))
        .and_then(managed_account);
    Ok(contract_for_plan(&desired, &planned, managed.as_deref()))
}

/// Target-only operations share the same publication barrier as file writes.
pub(crate) fn apply_target_only(
    service: &ProxyService,
    operation: &str,
    target: PendingTarget,
) -> Result<(), AppError> {
    crate::rt::block_on(
        service
            .codex_manager()
            .with_live_auth_guard(&[], |generation| {
                AppWrite::begin_codex_mode(service, generation)?
                    .run(operation, &[], target)
                    .map(|_| ())
            }),
    )
}

fn table_text(table: &Table) -> String {
    let mut table = table.clone();
    table.remove("requires_openai_auth");
    let mut doc = toml_edit::DocumentMut::new();
    doc.insert("t", Item::Table(table));
    doc.to_string()
}

/// 代理契约：路由供应商在客户端那一侧的全部要求。摘要用于记录模式契约；实际操作
/// 仍读取客户端文件并保留 no-op witnesses，不能用摘要跳过回读。`requires_openai_auth` 跟着盘上的登录走，不算进契约；官方路由要的是谁的登录
/// （托管账号，或 `official_login`：没绑托管账号的官方卡行里的账号）算进去。
fn contract_of(
    target: &Target<'_>,
    config: &CodexConfigPatch,
    catalog: Option<&[u8]>,
    managed: Option<&str>,
    official_login: Option<&str>,
) -> Contract {
    let base_url = match target {
        Target::Proxy { base_url, .. } => *base_url,
        Target::Direct(_) => "",
    };
    let (selector, table) = match &config.route {
        RouteWrite::Custom(table) => (ROUTE_ID, table_text(table)),
        RouteWrite::OfficialProxy {
            base_url,
            unified: true,
        } => (
            ROUTE_ID,
            table_text(&official_mirror_table(Some(base_url), false)),
        ),
        // 不写选路，改道写在顶层（地址已经在 `url` 里）。
        RouteWrite::OfficialProxy { unified: false, .. } => ("", "openai_base_url".to_string()),
        _ => ("", String::new()),
    };
    let pairs = |entries: &[(String, TomlValue)]| -> Vec<Value> {
        let mut pairs: Vec<Value> = entries
            .iter()
            .map(|(key, value)| serde_json::json!([key, value_text(value)]))
            .collect();
        pairs.sort_by_key(|pair| pair[0].as_str().unwrap_or_default().to_string());
        pairs
    };
    let nested: Vec<Value> = config
        .nested
        .iter()
        .map(|(path, value)| serde_json::json!([path.join("."), value_text(value)]))
        .collect();
    let parts = serde_json::json!({
        "app": "codex",
        "version": CONTRACT_VERSION,
        "url": base_url,
        "top": pairs(&config.top),
        "nested": nested,
        "exclusive": pairs(&config.exclusive),
        "selector": selector,
        "table": table,
        "catalog": digest(catalog),
        "managed": managed,
        "login": official_login,
    });
    let key = digest(Some(
        &serde_json::to_vec(&parts).expect("contract parts serialize"),
    ))
    .expect("digest");
    Contract {
        version: CONTRACT_VERSION,
        extra: Default::default(),
        key,
        exclusive: config
            .exclusive
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value_text(value))))
            .collect(),
    }
}

fn invalid() -> AppError {
    AppError::Config("codex.unverified_live_auth".into())
}
fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, AppError> {
    serde_json::from_slice::<crate::mode::unique_keys::UniqueKeys>(bytes).map_err(|_| invalid())?;
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn guarded(pre: Option<&[u8]>, after: Option<Vec<u8>>) -> Guarded {
    Guarded {
        expected_pre: digest(pre),
        then: after.map(WholeFile::Write).unwrap_or(WholeFile::Delete),
    }
}

/// Every expected path is selected by the caller; journal paths are not authority.
pub(crate) fn files() -> Vec<LiveFile> {
    let store = DeviceStore::for_device();
    vec![
        LiveFile::private(get_codex_auth_path()),
        LiveFile::private(get_codex_config_path()),
        LiveFile::shared(get_codex_model_catalog_path()),
        LiveFile::private(get_codex_managed_oauth_live_auth_marker_path()),
        LiveFile::private(
            store.path_for(&DeviceFile::registered(STASH_FILENAME).expect("registered stash")),
        ),
    ]
}

/// Returns the actual file outcome plus a non-secret R5 mapping warning flag.
pub(crate) fn run(
    state: &AppState,
    planned: Planned,
    prepared: &Prepared,
    provider: &Provider,
) -> Result<(OperationReport, bool), AppError> {
    run_with_catalog(state, planned, prepared, provider, None)
}

fn run_with_catalog(
    state: &AppState,
    planned: Planned,
    prepared: &Prepared,
    provider: &Provider,
    revision: Option<&CatalogRevision>,
) -> Result<(OperationReport, bool), AppError> {
    let ids = prepared
        .target_login
        .iter()
        .map(|(id, _)| id.clone())
        .chain(prepared.outgoing.iter().map(|(id, _)| id.clone()))
        .collect::<Vec<_>>();
    crate::rt::block_on(
        state
            .codex_oauth_manager
            .with_live_auth_guard(&ids, |generation| {
                if let Some((account, auth)) = &prepared.target_login {
                    if !generation.matches_prepared(account, auth) {
                        return Err(invalid());
                    }
                }
                let write = AppWrite::begin_codex(state, generation)?;
                run_pinned(
                    &write,
                    planned,
                    prepared,
                    Some(provider),
                    revision,
                    state::op::SWITCH,
                    PendingTarget::pointer(Some(provider.id.clone())),
                )
            }),
    )
}

struct Inputs {
    files: Vec<LiveFile>,
    pre: Vec<Option<Vec<u8>>>,
    live_auth: Option<Value>,
    config_doc: toml_edit::DocumentMut,
    live_managed: bool,
    stash: LoginStash,
}

fn read_inputs(write: &AppWrite<'_>, official_logins: &[Value]) -> Result<Inputs, AppError> {
    let files = files();
    let pre = files
        .iter()
        .map(|file| read_current(&file.path))
        .collect::<Result<Vec<_>, _>>()?;
    parse_inputs(files, pre, &write.vault, official_logins)
}

fn parse_inputs(
    files: Vec<LiveFile>,
    pre: Vec<Option<Vec<u8>>>,
    vault: &crate::secrets::VaultContext,
    official_logins: &[Value],
) -> Result<Inputs, AppError> {
    if pre.len() != files.len() {
        return Err(invalid());
    }
    let live_auth: Option<Value> = pre[0].as_deref().map(parse_json).transpose()?;
    if live_auth.as_ref().is_some_and(|v| !v.is_object()) {
        return Err(invalid());
    }
    let config_text =
        std::str::from_utf8(pre[1].as_deref().unwrap_or_default()).map_err(|_| invalid())?;
    let config_doc = config_text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| AppError::Config("codex.invalid_live_config".into()))?;
    if let Some(bytes) = &pre[2] {
        let _: Value = parse_json(bytes)?;
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Marker {
        version: u32,
        account_id: String,
    }
    let marker: Option<Marker> = pre[3].as_deref().map(parse_json).transpose()?;
    if marker
        .as_ref()
        .is_some_and(|m| m.version != 2 || m.account_id.trim().is_empty())
    {
        return Err(invalid());
    }
    let live_managed = marker.as_ref().is_some_and(|m| {
        live_auth
            .as_ref()
            .is_some_and(|auth| codex_live_auth_is_managed_chatgpt_login(auth, &m.account_id))
    });
    if marker.is_some() && !live_managed && live_auth.is_some() {
        return Err(invalid());
    }
    let stash_file = DeviceFile::registered(STASH_FILENAME)?;
    let stash = match &pre[4] {
        Some(bytes) => {
            let mut stash: LoginStash = parse_json(&stash_file.decode(vault, bytes)?)?;
            stash.initialized = true;
            stash
        }
        None => LoginStash::seeded_from_rows(official_logins),
    };
    Ok(Inputs {
        files,
        pre,
        live_auth,
        config_doc,
        live_managed,
        stash,
    })
}

fn plan_auth(
    planned: &mut Planned,
    prepared: &Prepared,
    provider: Option<&Provider>,
    inputs: &Inputs,
    preserve: bool,
) -> Result<codex_login::AuthPlan, AppError> {
    let live_auth = &inputs.live_auth;
    let live_managed = inputs.live_managed;
    let config_text =
        std::str::from_utf8(inputs.pre[1].as_deref().unwrap_or_default()).map_err(|_| invalid())?;
    let replaces = !planned.keep_native
        && provider.is_some_and(|p| {
            codex_live_write_replaces_auth_with_policy(
                p.category.as_deref(),
                &planned.auth,
                preserve,
                inputs.pre[3].is_some(),
            )
        });
    let auth_target = match &prepared.target_login {
        Some((_, auth)) => AuthTarget::Managed { auth },
        None if planned.keep_native => AuthTarget::ProxyThirdParty,
        None if planned.official => AuthTarget::Official {
            row_auth: &planned.auth,
        },
        None => AuthTarget::ThirdParty {
            preserve: !replaces,
        },
    };
    let mut auth_plan = codex_login::plan(AuthInput {
        live: live_auth.as_ref(),
        live_is_managed: live_managed,
        third_party_keys: &planned.facts.third_party_keys,
        leaving_official: planned.leaving_official.as_ref(),
        target: auth_target,
        stash: inputs.stash.clone(),
    });
    // Keep LoongPort's existing auth.json versus bearer-TOML placement rule.
    if !planned.official && replaces {
        auth_plan.auth = Some(Some(planned.auth.clone()));
    }
    if codex_config_auth_store_mode(config_text) != CodexAuthStoreMode::File
        && auth_plan.auth.is_some()
    {
        return Err(AppError::Config("codex.auth_store_unavailable".into()));
    }
    if planned.keep_native {
        if let RouteWrite::Custom(table) = &mut planned.config.route {
            if let Some(kind) = planned.stamp {
                let login = match codex_config_auth_store_mode(config_text) {
                    CodexAuthStoreMode::File => auth_plan.login_on_disk,
                    CodexAuthStoreMode::Ephemeral => false,
                    CodexAuthStoreMode::Keyring
                    | CodexAuthStoreMode::Auto
                    | CodexAuthStoreMode::Unknown => true,
                };
                table.insert(
                    "requires_openai_auth",
                    toml_edit::value(crate::live::project::codex::requires_openai_auth(
                        kind, login,
                    )),
                );
            }
        }
    } else {
        let auth_json = !planned.official && replaces;
        normalize_direct_auth_placement(planned, auth_json);
    }
    Ok(auth_plan)
}

/// The original direct-route output normalization, shared by the writer and
/// read-only final-image proof. This does not select a write policy or persist
/// a second credential state; the writer still uses its original marker rule.
fn normalize_direct_auth_placement(planned: &mut Planned, auth_json: bool) {
    if let RouteWrite::Custom(table) = &mut planned.config.route {
        if auth_json && extract_codex_auth_api_key(&planned.auth).is_some() {
            table.remove("experimental_bearer_token");
            table.insert("requires_openai_auth", toml_edit::value(true));
        } else if matches!(planned.stamp, Some(RouteAuth::Bearer | RouteAuth::EnvKey)) {
            table.remove("requires_openai_auth");
        }
    }
}

fn preserve_external_catalog(
    planned: &mut Planned,
    doc: &toml_edit::DocumentMut,
    base: &std::path::Path,
) -> bool {
    let external = external_catalog(doc, base);
    let preserved = row_catalog_pointer(&planned.config.top).is_none()
        && external.is_some()
        && external.and_then(TomlValue::as_str) != planned.outgoing_catalog.as_deref();
    if preserved {
        planned
            .config
            .top
            .push((MODEL_CATALOG_JSON.into(), external.unwrap().clone()));
        planned.config.catalog = false;
        planned.catalog = None;
    }
    preserved
}

/// Transient evidence from the original directory/file inspection owner. No
/// catalog policy, content import, discovery or persistent state is created.
pub(crate) struct NativeCatalogInputs {
    base: std::path::PathBuf,
    row_digest: Option<String>,
    live_digest: Option<String>,
    references: Vec<(
        std::path::PathBuf,
        crate::database::inspection::SourceRevision,
    )>,
}

impl NativeCatalogInputs {
    pub(crate) fn capture(provider: &Provider, live: &str) -> Option<Self> {
        // The original planner discovers CLI/cache model sources when specs are
        // present. Completion is read-only and cannot invent that generation.
        if codex_has_catalog_model_specs(&provider.settings_config) {
            return None;
        }
        let row = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        let base = get_codex_config_dir();
        let mut references = Vec::new();
        for text in [row, live] {
            let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
            if let Some(pointer) = doc.get(MODEL_CATALOG_JSON) {
                if pointer.as_str()?.trim().is_empty() {
                    return None;
                }
            }
            if let Some(path) = cc_switch_catalog_reference_path(text, &base) {
                // Capture the original reference, not just the resolved target.
                // This owner rejects symlink/reparse components and binds every
                // ancestor, file identity/content, and genuine absence.
                let revision = crate::database::inspection::file_revision(&path).ok()?;
                resolve_cc_switch_catalog_path(text, &base)?;
                crate::database::inspection::verify_unchanged(&path, &revision).ok()?;
                if !references.iter().any(|(seen, _)| seen == &path) {
                    references.push((path, revision));
                }
            }
        }
        Some(Self {
            base,
            row_digest: digest(Some(row.as_bytes())),
            live_digest: digest(Some(live.as_bytes())),
            references,
        })
    }

    pub(crate) fn references(
        &self,
    ) -> &[(
        std::path::PathBuf,
        crate::database::inspection::SourceRevision,
    )] {
        &self.references
    }

    fn matches(&self, provider: &Provider, live: &str) -> bool {
        let row = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.base == get_codex_config_dir()
            && self.row_digest == digest(Some(row.as_bytes()))
            && self.live_digest == digest(Some(live.as_bytes()))
            && self.references.iter().all(|(path, revision)| {
                crate::database::inspection::verify_unchanged(path, revision).is_ok()
            })
    }
}

/// Read-only admission from the upgrade owner's already-bound five files.
/// Unknown external discovery or managed-token generation is never a match.
/// This proves the original writer's owned-file result, not remote login validity.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn native_completion_match(
    provider: &Provider,
    rows: &indexmap::IndexMap<String, Provider>,
    settings: &crate::settings::AppSettings,
    vault: &crate::secrets::VaultContext,
    pre: Vec<Option<Vec<u8>>>,
    written: Option<&state::Written>,
    endpoint: Option<&(String, u16)>,
) -> Option<bool> {
    native_completion_match_with_generation(
        provider, rows, settings, vault, pre, written, endpoint, None, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_completion_match_with_generation(
    provider: &Provider,
    rows: &indexmap::IndexMap<String, Provider>,
    settings: &crate::settings::AppSettings,
    vault: &crate::secrets::VaultContext,
    pre: Vec<Option<Vec<u8>>>,
    written: Option<&state::Written>,
    endpoint: Option<&(String, u16)>,
    generation: Option<&CodexLiveAuthGuard<'_>>,
    catalog_inputs: Option<&NativeCatalogInputs>,
) -> Option<bool> {
    if codex_has_catalog_model_specs(&provider.settings_config)
        || !row_auth(provider).is_object()
        || written.is_some_and(|written| written.validate_for_app(app()).is_err())
    {
        return None;
    }
    let facts = row_facts_from_providers(rows.values());
    let inputs = parse_inputs(files(), pre, vault, &facts.official_logins).ok()?;
    let text = std::str::from_utf8(inputs.pre[1].as_deref().unwrap_or_default()).ok()?;
    if codex_config_auth_store_mode(text) != CodexAuthStoreMode::File {
        return None;
    }
    let mut prepared = Prepared::default();
    if let Some(account) = managed_account(provider) {
        let generation = generation?;
        let auth = inputs.live_auth.as_ref()?;
        let intent = written?.codex.as_ref()?.auth.as_ref()?;
        if !is_official(provider)
            || !inputs.live_managed
            || intent.account_id != account
            || Some(intent.last_refresh_ms) != auth_time(auth)
            || Some(intent.digest.clone()) != digest(inputs.pre[0].as_deref())
            || !generation.matches_live_generation(&account, auth)
        {
            return Some(false);
        }
        // Compare the original marker's full semantic image, including unknown
        // fields; its account name alone is not proof of a completed write.
        if parse_json::<Value>(inputs.pre[3].as_deref()?).ok()?
            != parse_json::<Value>(&codex_managed_oauth_marker_bytes(auth, &account).ok()?).ok()?
        {
            return Some(false);
        }
        prepared.target_login = Some((account, auth.clone()));
    } else if inputs.pre[3].is_some()
        || written
            .and_then(|w| w.codex.as_ref())
            .is_some_and(|w| w.auth.is_some())
    {
        return None;
    }
    let row_doc = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or("")
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    // A bound-byte-only caller still cannot prove managed-name path ownership.
    // Runtime review lends original directory evidence for both row and live.
    if catalog_inputs.is_some_and(|inputs| !inputs.matches(provider, text)) {
        return None;
    }
    for doc in [&row_doc, &inputs.config_doc] {
        if let Some(pointer) = doc.get(MODEL_CATALOG_JSON) {
            let pointer = pointer.as_str()?.trim();
            if pointer.is_empty()
                || (catalog_inputs.is_none()
                    && std::path::Path::new(pointer)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(is_our_model_catalog_filename))
            {
                return None;
            }
        }
    }
    let projection = project_for_write(provider).ok()?;
    let route = direct_route(&projection, settings.unify_codex_session_history, endpoint).ok()?;
    let mut planned = plan_projected(
        &Owner::Provider(provider),
        provider,
        projection,
        route,
        facts,
    )
    .ok()?;
    // A completed original managed-to-ordinary write consumes its marker and
    // can leave the row's API key in auth.json. Its valid final image need not
    // be a fixed point of a later write's marker-sensitive placement policy.
    // Prove the whole current auth and original route projection instead of
    // fabricating a historical marker or changing the preservation setting.
    let direct_auth_json = !planned.official
        && !planned.keep_native
        && matches!(planned.stamp, Some(RouteAuth::Bearer))
        && matches!(planned.config.route, RouteWrite::Custom(_))
        && crate::codex_config::codex_active_custom_route_uses_auth_json(&inputs.config_doc)
        && inputs.live_auth.as_ref() == Some(&planned.auth);
    if direct_auth_json && !crate::codex_config::codex_auth_is_loadable_api_key_only(&planned.auth)
    {
        // Do not fall back to the next-write plan: preserve=false would simply
        // copy this same invalid payload and make equality look like proof.
        return Some(false);
    }
    let auth = plan_auth(
        &mut planned,
        &prepared,
        Some(provider),
        &inputs,
        settings.preserve_codex_official_auth_on_switch,
    )
    .ok()?;
    let after_auth = match &auth.auth {
        None => inputs.live_auth.as_ref(),
        Some(value) => value.as_ref(),
    };
    if direct_auth_json {
        normalize_direct_auth_placement(&mut planned, true);
    } else if after_auth != inputs.live_auth.as_ref() {
        return Some(false);
    }
    // Missing stash initialization is not a native auth/config mismatch. Its
    // authenticated contents still participate in the original login plan.
    preserve_external_catalog(
        &mut planned,
        &inputs.config_doc,
        inputs.files[1].path.parent()?,
    );
    let actual = text.parse::<toml::Table>().ok()?;
    let mut after = inputs.config_doc.clone();
    planned
        .config
        .apply_to(&inputs.files[1].path, &mut after)
        .ok()?;
    if catalog_inputs.is_some_and(|inputs| !inputs.matches(provider, text)) {
        return None;
    }
    Some(actual == after.to_string().parse::<toml::Table>().ok()?)
}

type GuardedFiles = Vec<(LiveFile, Guarded)>;

#[allow(clippy::too_many_arguments)]
fn assemble_pinned(
    store: &DeviceStore,
    vault: &std::sync::RwLockReadGuard<'_, crate::secrets::VaultContext>,
    inputs: Inputs,
    mut planned: Planned,
    prepared: &Prepared,
    provider: Option<&Provider>,
    revision: Option<&CatalogRevision>,
    mut target: PendingTarget,
    preserve: bool,
) -> Result<(GuardedFiles, PendingTarget, bool, Option<String>), AppError> {
    let files = &inputs.files;
    let pre = &inputs.pre;
    let live_auth = &inputs.live_auth;
    let config_doc = &inputs.config_doc;
    let live_managed = inputs.live_managed;
    if let Some(revision) = revision {
        revision.verify(provider.ok_or_else(invalid)?, pre[1].as_deref())?;
    }
    if digest(pre[0].as_deref()) != prepared.auth_pre {
        return Err(invalid());
    }
    if let Some((account, None)) = &prepared.outgoing {
        if live_auth
            .as_ref()
            .is_some_and(|auth| codex_live_auth_is_managed_chatgpt_login(auth, account))
        {
            return Err(invalid());
        }
    }
    if let (Some((account, target)), Some(live)) = (&prepared.target_login, &live_auth) {
        if codex_live_auth_is_managed_chatgpt_login(live, account) {
            let order = auth_time(live)
                .zip(auth_time(target))
                .map(|(live, target)| live.cmp(&target));
            if order == Some(std::cmp::Ordering::Greater)
                || (live.get("tokens") != target.get("tokens")
                    && order != Some(std::cmp::Ordering::Less))
            {
                return Err(invalid());
            }
        }
    }
    let stash_file = DeviceFile::registered(STASH_FILENAME)?;
    let auth_plan = plan_auth(&mut planned, prepared, provider, &inputs, preserve)?;
    // Keep ownership in the original projector's top-field mechanism; never
    // write/import the external catalog. A takeover needs the bound config image.
    let existing_written = state::written(store, vault, app())?;
    if let Some(written) = &existing_written {
        written.validate()?;
    }
    let mut catalog_evidence = existing_written
        .and_then(|w| w.codex)
        .and_then(|c| c.catalog);
    let external_value = external_catalog(config_doc, files[1].path.parent().ok_or_else(invalid)?);
    let external = external_value.and_then(TomlValue::as_str);
    let preserved = revision.is_none()
        && preserve_external_catalog(
            &mut planned,
            config_doc,
            files[1].path.parent().ok_or_else(invalid)?,
        );
    if let Some(revision) = revision {
        if planned.catalog.is_none() || row_catalog_pointer(&planned.config.top).is_some() {
            return Err(AppError::Config(
                "codex.catalog_takeover_unavailable".into(),
            ));
        }
        let previous_pointer =
            external.ok_or_else(|| AppError::Config("codex.catalog_source_changed".into()))?;
        catalog_evidence = Some(state::CatalogTakeover {
            version: 1,
            provider_id: provider.ok_or_else(invalid)?.id.clone(),
            config_pre: revision.config_digest.clone().ok_or_else(invalid)?,
            previous_pointer: previous_pointer.into(),
            managed_pointer: crate::live::project::codex::CATALOG_FILENAME.into(),
            extra: Default::default(),
        });
    }
    let auth_after = match auth_plan.auth {
        None => pre[0].clone(),
        Some(None) => None,
        Some(Some(ref auth)) => Some(sorted_json_bytes(auth)?),
    };
    let config_after = TomlSteps(vec![&planned.config])
        .apply(&files[1].path, pre[1].as_deref())
        .map_err(|e| AppError::Config(e.to_string()))?;
    let catalog_after = planned.catalog.or_else(|| pre[2].clone());
    let marker_after = if let Some((account, auth)) = &prepared.target_login {
        Some(codex_managed_oauth_marker_bytes(auth, account)?)
    } else if pre[0].is_none() || (live_managed && auth_after != pre[0]) {
        None
    } else {
        pre[3].clone()
    };
    let stash_semantics = auth_plan
        .stash
        .as_ref()
        .map(sorted_json_bytes)
        .transpose()?
        .map(|bytes| digest(Some(&bytes)));
    let stash_after = auth_plan
        .stash
        .map(|stash| sorted_json_bytes(&stash).and_then(|bytes| stash_file.encode(vault, &bytes)))
        .transpose()?
        .or_else(|| pre[4].clone());
    let after = [
        auth_after,
        Some(config_after),
        catalog_after,
        marker_after,
        stash_after,
    ];
    let auth_intent = prepared
        .target_login
        .as_ref()
        .map(|(account, auth)| {
            Ok::<_, AppError>(state::ManagedAuthIntent {
                account_id: account.clone(),
                last_refresh_ms: auth_time(auth).ok_or_else(invalid)?,
                digest: digest(Some(&sorted_json_bytes(auth)?)).ok_or_else(invalid)?,
                extra: Default::default(),
            })
        })
        .transpose()?;
    target.written = Some(state::Written {
        codex: Some(state::CodexWritten {
            version: 1,
            auth: auth_intent,
            catalog: catalog_evidence,
            extra: Default::default(),
        }),
        ..Default::default()
    });
    let changes = files
        .iter()
        .cloned()
        .zip(
            pre.iter()
                .zip(after)
                .map(|(pre, after)| guarded(pre.as_deref(), after)),
        )
        .collect();
    Ok((changes, target, preserved, stash_semantics.flatten()))
}

fn run_pinned(
    write: &AppWrite<'_>,
    planned: Planned,
    prepared: &Prepared,
    provider: Option<&Provider>,
    revision: Option<&CatalogRevision>,
    operation: &str,
    target: PendingTarget,
) -> Result<(OperationReport, bool), AppError> {
    if let Some((account, Some(expected))) = &prepared.outgoing {
        ensure_codex_live_auth_unchanged_for_managed_account(account, expected)?;
    }
    let inputs = read_inputs(write, &planned.facts.official_logins)?;
    let (files, target, preserved, _) = assemble_pinned(
        &write.store,
        &write.vault,
        inputs,
        planned,
        prepared,
        provider,
        revision,
        target,
        crate::settings::preserve_codex_official_auth_on_switch(),
    )?;
    let changes = files
        .iter()
        .map(|(file, patch)| FileChange {
            file: file.clone(),
            patch: patch as &dyn LivePatch,
        })
        .collect::<Vec<_>>();
    Ok((write.run(operation, &changes, target)?, preserved))
}

/// Read-only editor entry into the same five-file assembly. Unknown discovery
/// or login generations remain unavailable; no prepare/refresh/adopt is called.
#[allow(clippy::type_complexity)]
pub(crate) fn plan_editor_save(
    service: &ProxyService,
    owner: &Owner<'_>,
    desired: Target<'_>,
    mut target: PendingTarget,
    request_id: &str,
) -> Result<(GuardedFiles, PendingTarget, bool, String, String), AppError> {
    let provider = desired.provider().ok_or_else(invalid)?;
    if codex_has_catalog_model_specs(&provider.settings_config) {
        return Err(invalid());
    }
    // The legacy outgoing-exclusives fallback logs parse diagnostics. A pure
    // editor must reject malformed owner input before reaching that fallback.
    if let Some(previous) = owner.provider() {
        project(previous)?;
    }
    let rows = service.database().get_all_providers(app())?;
    let settings = crate::settings::get_settings();
    let projection = project_for_write(provider)?;
    let endpoint = if is_official(provider) && !settings.unify_codex_session_history {
        let config = service.database().get_global_proxy_config_existing()?;
        Some((config.listen_address, config.listen_port))
    } else {
        None
    };
    let route = direct_route(
        &projection,
        settings.unify_codex_session_history,
        endpoint.as_ref(),
    )?;
    let mut planned = plan_projected(
        owner,
        provider,
        projection,
        route,
        row_facts_from_providers(rows.values()),
    )?;
    if let Target::Proxy { base_url, .. } = desired {
        apply_proxy_projection(&mut planned, base_url, settings.unify_codex_session_history);
    }
    service
        .codex_manager()
        .try_with_live_auth_guard(|generation| {
            let vault = service.database().secret_session().read()?;
            let store = DeviceStore::for_device();
            let pre = files()
                .iter()
                .map(|file| read_current(&file.path))
                .collect::<Result<Vec<_>, _>>()?;
            let text = std::str::from_utf8(pre[1].as_deref().unwrap_or_default())
                .map_err(|_| invalid())?;
            if codex_config_auth_store_mode(text) != CodexAuthStoreMode::File {
                return Err(invalid());
            }
            let catalog = NativeCatalogInputs::capture(provider, text).ok_or_else(invalid)?;
            let owner_catalog = owner
                .provider()
                .map(|row| NativeCatalogInputs::capture(row, text).ok_or_else(invalid))
                .transpose()?;
            let inputs = parse_inputs(files(), pre, &vault, &planned.facts.official_logins)?;
            let mut prepared = Prepared {
                auth_pre: digest(inputs.pre[0].as_deref()),
                ..Default::default()
            };
            let managed = managed_account(provider).filter(|_| is_official(provider));
            let outgoing = owner
                .provider()
                .and_then(managed_account)
                .filter(|account| Some(account) != managed.as_ref());
            let mut store_digest = None;
            if managed.is_some() || outgoing.is_some() {
                let path = crate::secrets::files::CredentialFile::Codex
                    .path(service.database().secret_session());
                let bytes = crate::config_file_io::read_regular_file(&path, 32 * 1024 * 1024)
                    .map_err(|error| AppError::io(&path, error))?;
                if generation.native_store_matches(&vault, bytes.as_deref()) != Some(true) {
                    return Err(invalid());
                }
                store_digest = digest(bytes.as_deref());
            }
            if let Some(account) = &managed {
                let auth = inputs.live_auth.as_ref().ok_or_else(invalid)?;
                if !inputs.live_managed || !generation.matches_live_generation(account, auth) {
                    return Err(invalid());
                }
                prepared.target_login = Some((account.clone(), auth.clone()));
            }
            if let Some(account) = outgoing {
                let refresh = if let Some(auth) = inputs
                    .live_auth
                    .as_ref()
                    .filter(|auth| codex_live_auth_is_managed_chatgpt_login(auth, &account))
                {
                    if !generation.matches_live_generation(&account, auth) {
                        return Err(invalid());
                    }
                    Some(
                        auth.pointer("/tokens/refresh_token")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .to_owned(),
                    )
                } else {
                    None
                };
                prepared.outgoing = Some((account, refresh));
            }
            if matches!(desired, Target::Proxy { .. }) {
                target.state.as_mut().ok_or_else(invalid)?.contract =
                    Some(contract_for_plan(&desired, &planned, managed.as_deref()));
            }
            let (files, target, preserved, stash) = assemble_pinned(
                &store,
                &vault,
                inputs,
                planned,
                &prepared,
                Some(provider),
                None,
                target,
                settings.preserve_codex_official_auth_on_switch,
            )?;
            let generation_revision = generation.native_revision(request_id)?;
            let row_versions = rows
                .values()
                .map(Database::provider_update_digest)
                .collect::<Result<Vec<_>, _>>()?;
            let source = sorted_json_bytes(&serde_json::json!([
                row_versions,
                settings.unify_codex_session_history,
                settings.preserve_codex_official_auth_on_switch,
                endpoint,
                catalog.references(),
                owner_catalog.as_ref().map(NativeCatalogInputs::references),
                store_digest,
                generation_revision,
                stash,
                target.written
            ]))?;
            Ok((
                files,
                target,
                preserved,
                digest(Some(&source)).ok_or_else(invalid)?,
                generation_revision,
            ))
        })
}

pub(crate) fn switch_to(
    state: &AppState,
    previous: Option<&Provider>,
    provider: &Provider,
) -> Result<bool, AppError> {
    // Read-only admission precedes token adoption/network; no held vault guard
    // crosses preparation. The same owner rechecks after prepare, before writes.
    crate::rt::block_on(
        state
            .codex_oauth_manager
            .with_live_auth_guard(&[], |generation| {
                let write = AppWrite::begin_codex(state, generation)?;
                read_inputs(&write, &[]).map(|_| ())
            }),
    )?;
    let owner = previous.map(Owner::Provider).unwrap_or(Owner::None);
    let prepared = prepare(&state.codex_oauth_manager, &owner, provider)?;
    let planned = plan(&state.db, &owner, provider)?;
    run(state, planned, &prepared, provider).map(|(_, preserved)| preserved)
}

fn auth_time(auth: &Value) -> Option<i64> {
    auth.get("last_refresh")?
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
}

#[allow(dead_code)] // Controlled backend entry; UI/runtime registration follows migration admission.
pub(crate) fn recover_pending(state: &AppState) -> Result<Option<RecoveryOutcome>, AppError> {
    let _switch = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app()));
    recover_locked(&state.proxy_service)
}

/// Caller owns the service switch lock. Account preparation remains outside the
/// pinned vault; the original manager guard covers verification and replay.
pub(crate) fn recover_locked(service: &ProxyService) -> Result<Option<RecoveryOutcome>, AppError> {
    recover_locked_with_checks(service, None)
}

pub(crate) fn recover_locked_with_checks(
    service: &ProxyService,
    checks: Option<&operation::AppRecoveryChecks<'_>>,
) -> Result<Option<RecoveryOutcome>, AppError> {
    let db = service.database();
    let (pending, before) = {
        let vault = db.secret_session().read()?;
        let store = DeviceStore::for_device();
        (
            state::pending(&store, &vault, app())?,
            crate::mode::current::validate_known_mode(&store, &vault, &AppType::Codex)?,
        )
    };
    let Some(pending) = pending else {
        return Ok(None);
    };
    let saved = pending
        .target
        .saved_row
        .as_ref()
        .map(operation::saved_provider)
        .transpose()?;
    let live_mode = pending.target.state.as_ref().unwrap_or(&before);
    let live_id = if live_mode.is_proxy() && live_mode.attached {
        live_mode.proxy_route.clone()
    } else {
        pending
            .target
            .pointer
            .clone()
            .or(crate::mode::current::provider_for(
                db,
                &AppType::Codex,
                crate::mode::current::Purpose::Direct,
            )?)
    };
    let account = if pending.files.is_empty() {
        None
    } else {
        let provider = match &live_id {
            Some(id) if saved.as_ref().is_some_and(|p| &p.id == id) => saved.clone(),
            Some(id) => db.get_provider_by_id(id, app())?,
            None => None,
        };
        provider
            .as_ref()
            .filter(|p| is_official(p))
            .and_then(managed_account)
    };
    let ids = account.iter().cloned().collect::<Vec<_>>();
    let result = crate::rt::block_on(service.codex_manager().with_live_auth_guard(
        &ids,
        |generation| {
            let write = AppWrite::open_codex_with_recovery(service, checks, generation)?;
            let verify = |pending: &state::Pending, live: Option<&state::LiveState>| {
                checks.map_or(Ok(()), |checks| {
                    (checks.verify)(&write.vault, pending, live)
                })
            };
            let finished = |pending: &state::Pending, live: Option<&state::LiveState>| {
                write.verify_recovered_target(pending, live)
            };
            let admission = checks.map(|checks| operation::RecoveryAdmission {
                expected_pending: checks.pending,
                verify: &verify,
                finished: &finished,
            });
            if state::pending(&write.store, &write.vault, app())?.as_ref() != Some(&pending) {
                return Err(invalid());
            }
            operation::verify_saved_row(
                db,
                db.secret_session(),
                &write.vault,
                &AppType::Codex,
                &pending.target,
            )?;
            if pending.files.is_empty() {
                let inactive_save = saved.as_ref().is_some_and(|saved| {
                    (before.is_proxy() && !before.attached) || live_id.as_ref() != Some(&saved.id)
                });
                let detached_mode = pending
                    .target
                    .state
                    .as_ref()
                    .is_some_and(|mode| !mode.attached)
                    && matches!(
                        pending.op.as_str(),
                        state::op::ROUTE | state::op::EXIT | state::op::DETACH
                    );
                let detached_selection = pending.op == state::op::APPLY
                    && before.is_proxy()
                    && !before.attached
                    && pending.target.model_preference.is_some()
                    && pending
                        .target
                        .state
                        .as_ref()
                        .is_some_and(|mode| mode.is_proxy() && !mode.attached);
                let order_only = pending.op == state::op::APPLY
                    && pending.target.routing_order.is_some()
                    && pending.target.state.is_none()
                    && pending.target.saved_row.is_none()
                    && pending.target.model_preference.is_none();
                if pending.target.pointer.is_some()
                    || pending.target.written.is_some()
                    || !(detached_mode
                        || detached_selection
                        || order_only
                        || (pending.target.state.is_none() && inactive_save))
                {
                    return Err(invalid());
                }
                return operation::recover_checked_guarded(
                    &write.store,
                    &write.vault,
                    &write.guard,
                    &[],
                    &|target| write.commit(target),
                    &|_| Ok(None),
                    admission.as_ref(),
                );
            }
            let admitted = files();
            operation::recover_checked_guarded(
                &write.store,
                &write.vault,
                &write.guard,
                &admitted,
                &|target| write.commit(target),
                &|journal| {
                    if journal.target != pending.target || journal.files.len() != admitted.len() {
                        return Err(invalid());
                    }
                    let written = journal.target.written.as_ref().ok_or_else(invalid)?;
                    let codex = written.codex.as_ref().ok_or_else(invalid)?;
                    let before = state::written(&write.store, &write.vault, app())?
                        .and_then(|w| w.codex)
                        .and_then(|c| c.catalog);
                    if codex.catalog != before {
                        let file = journal
                            .files
                            .iter()
                            .find(|f| f.path == get_codex_config_path())
                            .ok_or_else(invalid)?;
                        let bytes = read_current(&file.path)?;
                        let bytes = if digest(bytes.as_deref()) == file.planned {
                            bytes
                        } else {
                            file.staged
                                .as_ref()
                                .map(|p| read_current(p))
                                .transpose()?
                                .flatten()
                        };
                        let doc = std::str::from_utf8(bytes.as_deref().ok_or_else(invalid)?)
                            .map_err(|_| invalid())?
                            .parse::<toml_edit::DocumentMut>()
                            .map_err(|_| invalid())?;
                        match &codex.catalog {
                            Some(record)
                                if journal.target.pointer.as_ref() == Some(&record.provider_id)
                                    && file.pre.as_deref() == Some(record.config_pre.as_str())
                                    && doc.get(MODEL_CATALOG_JSON).and_then(Item::as_str)
                                        == Some(record.managed_pointer.as_str()) => {}
                            None if journal.op == state::op::CATALOG
                                && journal.target.pointer.is_none()
                                && doc.get(MODEL_CATALOG_JSON).and_then(Item::as_str)
                                    == before
                                        .as_ref()
                                        .map(|record| record.previous_pointer.as_str()) => {}
                            _ => return Err(invalid()),
                        }
                    }

                    if let Some(account) = &account {
                        let intent = journal
                            .target
                            .written
                            .as_ref()
                            .and_then(|w| w.codex.as_ref())
                            .and_then(|c| c.auth.as_ref())
                            .ok_or_else(invalid)?;
                        if intent.account_id != *account {
                            return Err(invalid());
                        }
                        let index = journal
                            .files
                            .iter()
                            .position(|file| file.path == get_codex_auth_path())
                            .ok_or_else(invalid)?;
                        let auth_file = &journal.files[index];
                        let current = read_current(&auth_file.path)?;
                        let current_digest = digest(current.as_deref());
                        // An exact no-op witness is already authenticated by this
                        // journal, including a newer bundle proven and durably accepted
                        // before a previous process stopped. A restart need not invent
                        // fresh access-token evidence for those exact recorded bytes.
                        if auth_file.pre == auth_file.planned && current_digest == auth_file.planned
                        {
                            let auth: Value = parse_json(current.as_deref().ok_or_else(invalid)?)?;
                            if generation.matches_live_generation(account, &auth)
                                && auth_time(&auth)
                                    .is_some_and(|time| time >= intent.last_refresh_ms)
                            {
                                return Ok(None);
                            }
                            return Err(invalid());
                        }
                        if current_digest.as_deref() != Some(intent.digest.as_str())
                            && current_digest != auth_file.pre
                            || (auth_file.pre == auth_file.planned
                                && auth_file.planned.as_deref() != Some(intent.digest.as_str()))
                        {
                            let auth: Value = parse_json(current.as_deref().ok_or_else(invalid)?)?;
                            if !generation.matches_prepared(account, &auth)
                                || auth_time(&auth)
                                    .is_none_or(|time| time <= intent.last_refresh_ms)
                            {
                                return Err(invalid());
                            }
                            // The marker/config/catalog/stash must independently prove
                            // the same completed original target. No partial file is
                            // silently accepted just because a newer token is valid.
                            for (i, file) in journal.files.iter().enumerate() {
                                if i != index
                                    && digest(read_current(&file.path)?.as_deref()) != file.planned
                                {
                                    return Err(invalid());
                                }
                            }
                            return Ok(Some((index, current_digest.ok_or_else(invalid)?)));
                        }
                        let bytes = if current_digest == auth_file.planned {
                            current
                        } else {
                            auth_file
                                .staged
                                .as_ref()
                                .map(|path| read_current(path))
                                .transpose()?
                                .flatten()
                        };
                        let auth: Value = parse_json(bytes.as_deref().ok_or_else(invalid)?)?;
                        if digest(bytes.as_deref()).as_deref() != Some(intent.digest.as_str())
                            || !generation.matches_live_generation(account, &auth)
                        {
                            return Err(invalid());
                        }
                    } else if journal
                        .target
                        .written
                        .as_ref()
                        .and_then(|w| w.codex.as_ref())
                        .and_then(|c| c.auth.as_ref())
                        .is_some()
                    {
                        return Err(invalid());
                    }
                    Ok(None)
                },
                admission.as_ref(),
            )
        },
    ));
    match result {
        Err(AppError::Config(code))
            if matches!(
                code.as_str(),
                "codex.unverified_live_auth" | "codex.managed_account_missing"
            ) =>
        {
            Ok(Some(RecoveryOutcome::VerificationRequired {
                paths: vec![get_codex_auth_path()],
            }))
        }
        result => result,
    }
}

/// Internal revision-bound choice only. No command/UI is registered here.
pub(crate) struct CatalogRevision {
    pub app: String,
    pub provider_id: String,
    pub config_digest: Option<String>,
}

impl CatalogRevision {
    fn verify(&self, provider: &Provider, config: Option<&[u8]>) -> Result<(), AppError> {
        if self.app != app()
            || self.provider_id != provider.id
            || self.config_digest != digest(config)
        {
            return Err(AppError::Config("codex.catalog_source_changed".into()));
        }
        Ok(())
    }
}

#[allow(dead_code)] // Controlled backend entry; UI/runtime registration follows migration admission.
pub(crate) fn manage_catalog(state: &AppState, revision: &CatalogRevision) -> Result<(), AppError> {
    let _switch = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app()));
    let provider = state
        .db
        .get_provider_by_id(&revision.provider_id, app())?
        .ok_or_else(invalid)?;
    super::validate_provider_selection(&state.db, &AppType::Codex, &provider.id)?;
    crate::rt::block_on(
        state
            .codex_oauth_manager
            .with_live_auth_guard(&[], |generation| {
                let write = AppWrite::begin_codex(state, generation)?;
                read_inputs(&write, &[]).map(|_| ())
            }),
    )?;
    revision.verify(
        &provider,
        read_current(&get_codex_config_path())?.as_deref(),
    )?;
    let previous = crate::mode::current::direct_provider(&state.db, &AppType::Codex)?;
    let owner = previous
        .as_ref()
        .map(Owner::Provider)
        .unwrap_or(Owner::None);
    let prepared = prepare(&state.codex_oauth_manager, &owner, &provider)?;
    let planned = plan(&state.db, &owner, &provider)?;
    run_with_catalog(state, planned, &prepared, &provider, Some(revision)).map(|_| ())
}

#[allow(dead_code)] // Controlled backend entry; UI/runtime registration follows migration admission.
pub(crate) fn restore_catalog(
    state: &AppState,
    revision: &CatalogRevision,
) -> Result<(), AppError> {
    let _switch = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app()));
    let provider =
        crate::mode::current::direct_provider(&state.db, &AppType::Codex)?.ok_or_else(invalid)?;
    crate::rt::block_on(
        state
            .codex_oauth_manager
            .with_live_auth_guard(&[], |generation| {
                restore_catalog_pinned(state, revision, &provider, generation)
            }),
    )
}

fn restore_catalog_pinned(
    state: &AppState,
    revision: &CatalogRevision,
    provider: &Provider,
    generation: &CodexLiveAuthGuard<'_>,
) -> Result<(), AppError> {
    let write = AppWrite::begin_codex(state, generation)?;
    let Inputs {
        files,
        pre,
        config_doc,
        ..
    } = read_inputs(&write, &[])?;
    revision.verify(provider, pre[1].as_deref())?;
    let mut written = state::written(&write.store, &write.vault, app())?.ok_or_else(invalid)?;
    written.validate()?;
    let codex = written.codex.as_mut().ok_or_else(invalid)?;
    let catalog = codex.catalog.as_ref().ok_or_else(invalid)?;
    if config_doc.get(MODEL_CATALOG_JSON).and_then(Item::as_str)
        != Some(catalog.managed_pointer.as_str())
    {
        return Err(AppError::Config("codex.catalog_source_changed".into()));
    }
    let patch = crate::live::patch::toml::TomlPatch {
        set: vec![(
            crate::live::patch::KeyPath::new(&[MODEL_CATALOG_JSON]),
            toml_edit::value(&catalog.previous_pointer),
        )],
        ..Default::default()
    };
    let restored = patch
        .apply(&files[1].path, pre[1].as_deref())
        .map_err(|e| AppError::Config(e.to_string()))?;
    codex.catalog = None;
    let patches = pre
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            guarded(
                bytes.as_deref(),
                if index == 1 {
                    Some(restored.clone())
                } else {
                    bytes.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    let changes = files
        .into_iter()
        .zip(&patches)
        .map(|(file, patch)| FileChange {
            file,
            patch: patch as &dyn LivePatch,
        })
        .collect::<Vec<_>>();
    write.run(
        state::op::CATALOG,
        &changes,
        PendingTarget {
            written: Some(written),
            ..Default::default()
        },
    )?;
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "codex_native_proof_tests.rs"]
mod native_proof_tests;
