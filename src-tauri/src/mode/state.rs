//! Device mode state and serialized read-modify-write from cc-switch v4.0.2.
//! LoongPort persists only authenticated device-file ciphertext with the caller's
//! existing vault read guard. Corrupt state is never repaired, renamed or reset;
//! untouched future data remains readable without authorizing destructive edits.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, RwLockReadGuard};
use zeroize::Zeroizing;

use crate::error::AppError;
use crate::live::engine::DeviceStore;
use crate::secrets::owned_file::{DeviceFile, DEVICE_STATE_FILE};
use crate::secrets::VaultContext;

#[cfg(test)]
#[path = "state_persistence_tests.rs"]
mod persistence_tests;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveState {
    pub version: u32,
    #[serde(default)]
    pub apps: BTreeMap<String, AppLiveState>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            apps: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Direct,
    Proxy,
}

/// 进入代理时写进客户端文件的契约。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    pub version: u32,
    /// 契约内容的摘要：切换路由时摘要相同，客户端文件就不用动。
    pub key: String,
    /// 契约写进客户端的独有字段。退出代理时按它删除（值相同才删）：路由供应商的行之后可能
    /// 被编辑过，不能到时再按行重新计算。
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub exclusive: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个应用的模式状态。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModeState {
    /// 没有值：这台设备还没运行过有双模式的版本，启动时按旧版遗留的接管状态定下来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// 客户端文件当前是否指向代理。退出 CC Switch 时分离、下次启动再接上。
    #[serde(default, skip_serializing_if = "is_false")]
    pub attached: bool,
    /// 代理模式下路由到的供应商。和直连指针互相独立，退出代理时保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModeUpdateError {
    #[error("Unknown device mode fields require a compatible application version")]
    UnknownFields,
    #[error("Unsupported device contract version: {0}")]
    UnsupportedContractVersion(u32),
}

impl ModeState {
    pub(super) fn validate_for_update(&self) -> Result<(), ModeUpdateError> {
        if !self.extra.is_empty() {
            return Err(ModeUpdateError::UnknownFields);
        }
        if let Some(contract) = &self.contract {
            if !contract.extra.is_empty() {
                return Err(ModeUpdateError::UnknownFields);
            }
            if contract.version != super::contract::CONTRACT_VERSION {
                return Err(ModeUpdateError::UnsupportedContractVersion(
                    contract.version,
                ));
            }
        }
        Ok(())
    }

    pub fn is_proxy(&self) -> bool {
        self.mode == Some(Mode::Proxy)
    }

    /// 代理模式下路由到的就是 `id` 这家。
    pub fn routes_to(&self, id: &str) -> bool {
        self.is_proxy() && self.proxy_route.as_deref() == Some(id)
    }
}

/// CC Switch 上次写进客户端文件、切走时要按记录删掉的东西。
///
/// 不能按 live 现在的内容去找：客户端自己会改。Grok 的 `/settings` 会把 `models.default`
/// 改成内置模型，按它找表就会漏删上一家的表；而 CC Switch 默认的表名 `grok-4.5` 正好是
/// 内置模型 ID，留下的表会覆盖内置模型，把官方请求连同第三方 Key 发到第三方地址。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Written {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexWritten>,
    /// Grok Build `config.toml` 里 CC Switch 写的 `[model."<名称>"]` 表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Original-operation identity evidence, not a token cache or a new epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedAuthIntent {
    pub account_id: String,
    pub last_refresh_ms: i64,
    pub digest: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexWritten {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogTakeover>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ManagedAuthIntent>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// R5 evidence for the current explicitly chosen catalog takeover. Stored in
/// the same encrypted intent/Written owner, never in client staleness history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogTakeover {
    pub version: u32,
    pub provider_id: String,
    pub config_pre: String,
    pub previous_pointer: String,
    pub managed_pointer: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Written {
    /// Known evidence belongs to its original native writer, not just this
    /// generic serialization envelope. Absence is handled by the app caller.
    pub(crate) fn validate_for_app(&self, app: &str) -> Result<(), AppError> {
        self.validate()?;
        let matches = match app {
            "codex" => self.codex.is_some() && self.tables.is_empty(),
            "grokbuild" => self.codex.is_none(),
            _ => false,
        };
        if !matches {
            return Err(AppError::Config("mode.verification_required".into()));
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), AppError> {
        let invalid = || AppError::Config("mode.verification_required".into());
        if !self.extra.is_empty() {
            return Err(invalid());
        }
        if let Some(codex) = &self.codex {
            if !self.tables.is_empty() || codex.version != 1 || !codex.extra.is_empty() {
                return Err(invalid());
            }
            if let Some(catalog) = &codex.catalog {
                if catalog.version != 1
                    || !catalog.extra.is_empty()
                    || catalog.provider_id.trim().is_empty()
                    || catalog.previous_pointer.trim().is_empty()
                    || !crate::codex_config::is_our_model_catalog_filename(&catalog.managed_pointer)
                    || catalog.config_pre.len() != 64
                    || !catalog
                        .config_pre
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(invalid());
                }
            }
            if let Some(auth) = &codex.auth {
                if auth.account_id.trim().is_empty()
                    || auth.last_refresh_ms <= 0
                    || !auth.extra.is_empty()
                    || auth.digest.len() != 64
                    || !auth
                        .digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

/// 代理模式的 Stack 模型：这些供应商的模型以带前缀的 id 发布给客户端，选中后请求直达那一家
/// （`mode::stack`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StackState {
    /// Stack 模式：代理模式下发布 Stack 模型、不做故障转移（界面上和路由模式二选一，见
    /// `controller::enter`）。只在代理模式下有意义：每次进入代理时按用户选的模式写定，
    /// 退出代理时不动，名单也留着。
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    /// 当前 Stack 里的供应商 id，按加入顺序。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// key 登记簿：key → 供应商 id。一经分配永久归这家，移除成员、删除供应商都不回收：
    /// 客户端会一直带着选中过的 id，key 改了指向，旧 id 就会被悄悄发到另一家。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl StackState {
    pub fn is_empty(&self) -> bool {
        !self.enabled && self.members.is_empty() && self.keys.is_empty() && self.extra.is_empty()
    }

    /// 这家在登记簿里的 key。
    pub fn key_of(&self, provider_id: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(_, id)| id.as_str() == provider_id)
            .map(|(key, _)| key.as_str())
    }

    pub fn is_member(&self, provider_id: &str) -> bool {
        self.members.iter().any(|id| id == provider_id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppLiveState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub attached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    /// 没有值：这台设备上还没有新版写过这个应用的文件（升级前旧版写的，按行推断）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<Written>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    #[serde(default, skip_serializing_if = "StackState::is_empty")]
    pub stack: StackState,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AppLiveState {
    fn is_empty(&self) -> bool {
        self.pending.is_none()
            && self.written.is_none()
            && self.stack.is_empty()
            && self.mode_state() == ModeState::default()
            && self.extra.is_empty()
    }

    pub fn mode_state(&self) -> ModeState {
        ModeState {
            mode: self.mode,
            attached: self.attached,
            proxy_route: self.proxy_route.clone(),
            contract: self.contract.clone(),
            extra: self.extra.clone(),
        }
    }

    /// Preserve unknown data for reads, but never interpret future mode semantics
    /// as a supported transition. In particular, flattened fields must not be
    /// merged into app-owned pending/written/stack fields.
    pub fn set_mode_state(&mut self, state: ModeState) -> Result<(), ModeUpdateError> {
        self.mode_state().validate_for_update()?;
        state.validate_for_update()?;
        self.mode = state.mode;
        self.attached = state.attached;
        self.proxy_route = state.proxy_route;
        self.contract = state.contract;
        Ok(())
    }
}

/// Preserve future operation names for inspection. Recovery must separately
/// admit supported operations; an unknown name never authorizes replay.
pub mod op {
    /// 切换供应商（会改指针）。
    pub const SWITCH: &str = "switch";
    /// 把当前供应商重新写进客户端文件（编辑、同步等）。
    pub const APPLY: &str = "apply";
    /// 进入代理模式：客户端文件写成代理契约。
    pub const ENTER: &str = "enter";
    /// 退出代理模式：客户端文件写回直连供应商。
    pub const EXIT: &str = "exit";
    /// 退出 CC Switch 时把客户端指回直连，模式不变。
    pub const DETACH: &str = "detach";
    /// 启动时把客户端重新指向代理。
    pub const ATTACH: &str = "attach";
    /// 代理模式下换路由（契约变了时同一操作里先改写客户端）。
    pub const ROUTE: &str = "route";
    /// 增删 Stack 模型（契约变了时同一操作里先改写客户端）。
    pub const STACK: &str = "stack";
    /// Codex 改用 CC Switch 生成的模型目录（用户在 Stack 提示上点的）：一律重写客户端。
    pub const CATALOG: &str = "catalog";

    /// Persistence recognizes the upstream vocabulary; operation admission still
    /// decides which of these transitions the current product supports.
    pub(super) fn is_known(value: &str) -> bool {
        matches!(
            value,
            SWITCH | APPLY | ENTER | EXIT | DETACH | ATTACH | ROUTE | STACK | CATALOG
        )
    }
}

/// 一次操作的写前意图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub op: String,
    pub files: Vec<PendingFile>,
    #[serde(default)]
    pub target: PendingTarget,
    /// 已经开始发布：换进第一个文件之前记下，所以只要有文件发布过它就一定在。发布过的
    /// 文件之后可能又被客户端改掉（Codex 刷新登录），单看文件内容就分不出发布开始过没有，
    /// 恢复时靠它决定前滚还是丢弃。
    #[serde(default, skip_serializing_if = "is_false")]
    pub published: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFile {
    /// LoongPort publication policy. Missing legacy metadata is unknown, never public.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    pub path: PathBuf,
    /// 写前内容的 hash；`None` 表示写前文件不存在。
    pub pre: Option<String>,
    /// 写后内容的 hash；`None` 表示这个操作要删掉它。
    #[serde(default)]
    pub planned: Option<String>,
    /// 已写好写后内容、等着 rename 的临时文件；删文件时没有。
    #[serde(default)]
    pub staged: Option<PathBuf>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// R3 row publication belongs to the same encrypted file intent, not a rollback snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRow {
    pub before: String,
    pub provider: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_model_preference: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelPreferenceAction {
    Clear {},
    Set { model: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingOrderTarget {
    pub profile_name: String,
    pub provider_ids: Vec<String>,
    pub before: crate::database::order_profiles::OrderSnapshot,
    pub planned: crate::database::order_profiles::OrderSnapshot,
}

/// 文件都写完之后要落定的状态。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_preference: Option<ModelPreferenceAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_order: Option<RoutingOrderTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_row: Option<SavedRow>,
    /// 直连指针：切换成功后当前供应商是谁。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// 模式状态：有值时整体替换这个应用的 mode、attached、proxy_route、contract。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ModeState>,
    /// 写入记录：有值时整体替换。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<Written>,
    /// Stack 模型：有值时整体替换这个应用的 `stack`（成员和登记簿一起）。不放进 `state`：
    /// `state` 会整体替换，不认识 `stack` 的版本写下的 pending 前滚时就会把名单清空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<StackState>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PendingTarget {
    /// 只改直连指针（`None` 表示不改）。
    pub fn pointer(pointer: Option<String>) -> Self {
        Self {
            pointer,
            ..Self::default()
        }
    }

    /// 只落定模式状态。
    pub fn mode(state: ModeState) -> Self {
        Self {
            state: Some(state),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pointer.is_none()
            && self.model_preference.is_none()
            && self.routing_order.is_none()
            && self.saved_row.is_none()
            && self.state.is_none()
            && self.written.is_none()
            && self.stack.is_none()
            && self.extra.is_empty()
    }
}

/// Errors deliberately omit parser details, paths, and state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateDecodeError {
    #[error("Invalid device mode state")]
    InvalidData,
    #[error("Unsupported device mode state version: {0}")]
    UnsupportedVersion(u32),
}

/// Decode already-decrypted device bytes. This is not recovery/write admission.
pub fn decode(bytes: &[u8]) -> Result<LiveState, StateDecodeError> {
    serde_json::from_slice::<super::unique_keys::UniqueKeys>(bytes)
        .map_err(|_| StateDecodeError::InvalidData)?;
    let state: LiveState =
        serde_json::from_slice(bytes).map_err(|_| StateDecodeError::InvalidData)?;
    if state.version != STATE_VERSION {
        return Err(StateDecodeError::UnsupportedVersion(state.version));
    }
    Ok(state)
}

/// One device state file contains every app, so all read-modify-write operations
/// share the upstream lock. The vault guard must be acquired before this lock.
fn state_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

/// A transient view of the original file, not a second state store. Untouched
/// app/root payloads retain their exact JSON bytes, including future numbers.
#[derive(Serialize)]
pub(crate) struct PreservedState {
    version: u32,
    #[serde(default)]
    apps: BTreeMap<String, Box<serde_json::value::RawValue>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Box<serde_json::value::RawValue>>,
}

fn invalid_state() -> AppError {
    AppError::Config(StateDecodeError::InvalidData.to_string())
}

fn read_preserved(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<PreservedState, AppError> {
    let file = DeviceFile::registered(DEVICE_STATE_FILE)?;
    let Some(bytes) = store.read_device(vault, &file)? else {
        return Ok(PreservedState {
            version: STATE_VERSION,
            apps: BTreeMap::new(),
            extra: BTreeMap::new(),
        });
    };
    decode_preserved(&bytes)
}

fn decode_preserved(bytes: &[u8]) -> Result<PreservedState, AppError> {
    serde_json::from_slice::<super::unique_keys::UniqueKeys>(bytes).map_err(|_| invalid_state())?;
    // RawValue must be captured by the JSON parser itself. serde's flatten
    // deserialization buffers unknown values and loses their original bytes.
    let mut fields: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).map_err(|_| invalid_state())?;
    let version = fields.remove("version").ok_or_else(invalid_state)?;
    let version: u32 = serde_json::from_str(version.get()).map_err(|_| invalid_state())?;
    if version != STATE_VERSION {
        return Err(AppError::Config(
            StateDecodeError::UnsupportedVersion(version).to_string(),
        ));
    }
    let apps = fields
        .remove("apps")
        .map(|raw| serde_json::from_str(raw.get()).map_err(|_| invalid_state()))
        .transpose()?
        .unwrap_or_default();
    Ok(PreservedState {
        version,
        apps,
        extra: fields,
    })
}

/// Backup/review validates the authenticated shared envelope only. This never
/// authorizes an unknown app, rewrites its bytes or relaxes ordinary decode.
pub(crate) fn validate_envelope(bytes: &[u8]) -> Result<(), AppError> {
    decode_preserved(bytes).map(|_| ())
}

/// One transient authenticated file view for the original upgrade reviewer.
pub(crate) fn read_review_snapshot(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<PreservedState, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    read_preserved(store, vault)
}

fn selected_app(state: &PreservedState, app: &str) -> Result<Option<AppLiveState>, AppError> {
    state
        .apps
        .get(app)
        .map(|raw| serde_json::from_str(raw.get()).map_err(|_| invalid_state()))
        .transpose()
}

impl PreservedState {
    pub(crate) fn has_shared_extensions(&self) -> bool {
        !self.extra.is_empty()
    }

    pub(crate) fn app_names(&self) -> impl Iterator<Item = &str> {
        self.apps.keys().map(String::as_str)
    }

    pub(crate) fn app_view(&self, app: &str) -> Result<LiveState, AppError> {
        let apps = selected_app(self, app)?
            .map(|entry| (app.to_owned(), entry))
            .into_iter()
            .collect();
        let extra = self
            .extra
            .iter()
            .map(|(key, value)| {
                serde_json::from_str(value.get())
                    .map(|value| (key.clone(), value))
                    .map_err(|_| invalid_state())
            })
            .collect::<Result<_, _>>()?;
        Ok(LiveState {
            version: self.version,
            apps,
            extra,
        })
    }
}

/// Read only the selected typed app. Missing mode stays unknown; malformed or
/// future peer apps are retained opaquely, never interpreted as empty/Direct.
pub(crate) fn load_app(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<LiveState, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    read_preserved(store, vault)?.app_view(app)
}

/// Change one compatible subtree under the original file lock. A future target
/// app is refused; unrelated payloads are never decoded, normalized or removed.
pub(crate) fn update_app<R>(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
    change: impl FnOnce(&mut AppLiveState) -> Result<R, AppError>,
) -> Result<R, AppError> {
    update_app_checked(store, vault, app, &|_| Ok(()), change)
}

fn update_app_checked<R>(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
    check: &dyn Fn(&LiveState) -> Result<(), AppError>,
    change: impl FnOnce(&mut AppLiveState) -> Result<R, AppError>,
) -> Result<R, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut raw = read_preserved(store, vault)?;
    check(&raw.app_view(app)?)?;
    let before = selected_app(&raw, app)?.unwrap_or_default();
    validate_app_for_update(&before)?;
    let mut after = before.clone();
    let result = change(&mut after)?;
    validate_app_for_update(&after)?;
    if after == before {
        return Ok(result);
    }
    if after.is_empty() {
        raw.apps.remove(app);
    } else {
        raw.apps.insert(
            app.to_owned(),
            serde_json::value::to_raw_value(&after)
                .map_err(|source| AppError::JsonSerialize { source })?,
        );
    }
    let bytes = Zeroizing::new(
        serde_json::to_vec_pretty(&raw).map_err(|source| AppError::JsonSerialize { source })?,
    );
    store.write_device(vault, &DeviceFile::registered(DEVICE_STATE_FILE)?, &bytes)?;
    Ok(result)
}

/// Conditional journal removal uses the same state lock as publication. A
/// replacement operation or changed app evidence must never be acknowledged.
pub(crate) fn clear_pending_checked(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
    expected: &Pending,
    check: &dyn Fn(&LiveState) -> Result<(), AppError>,
) -> Result<(), AppError> {
    update_app_checked(
        store,
        vault,
        app,
        &|live| {
            if live.apps.get(app).and_then(|entry| entry.pending.as_ref()) != Some(expected) {
                return Err(AppError::Config("mode.verification_required".into()));
            }
            check(live)
        },
        |entry| {
            if entry.pending.as_ref() != Some(expected) {
                return Err(AppError::Config("mode.verification_required".into()));
            }
            entry.pending = None;
            Ok(())
        },
    )
}

/// Absence is the only empty-state case. Authentication, parse and version errors
/// leave the original file untouched for controlled inspection and recovery.
pub(crate) fn load(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<LiveState, AppError> {
    let file = DeviceFile::registered(DEVICE_STATE_FILE)?;
    match store.read_device(vault, &file)? {
        Some(bytes) => decode(&bytes).map_err(|error| AppError::Config(error.to_string())),
        None => Ok(LiveState::default()),
    }
}

fn save(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    state: &LiveState,
) -> Result<(), AppError> {
    let bytes = Zeroizing::new(
        serde_json::to_vec_pretty(state).map_err(|source| AppError::JsonSerialize { source })?,
    );
    store.write_device(vault, &DeviceFile::registered(DEVICE_STATE_FILE)?, &bytes)
}

fn unsupported_update() -> AppError {
    AppError::Config("mode.unsupported_state_update".into())
}

/// Validate only changed app subtrees. A future app cannot be rewritten or
/// removed, but it must not block a supported operation on an unrelated app.
fn validate_change(before: &LiveState, after: &LiveState) -> Result<(), AppError> {
    if after.version != STATE_VERSION || before.extra != after.extra {
        return Err(unsupported_update());
    }
    for (app, old) in &before.apps {
        if after.apps.get(app) != Some(old) {
            validate_app_for_update(old)?;
        }
    }
    for (app, new) in &after.apps {
        if before.apps.get(app) != Some(new) {
            validate_app_for_update(new)?;
        }
    }
    Ok(())
}

/// App-local write admission also binds both saved and pending evidence.
/// Generic storage validation stays permissive for untouched peer subtrees.
pub(crate) fn validate_app_evidence_for_update(
    name: &str,
    app: &AppLiveState,
) -> Result<(), AppError> {
    validate_app_for_update(app)?;
    for written in app.written.iter().chain(
        app.pending
            .iter()
            .filter_map(|pending| pending.target.written.as_ref()),
    ) {
        written.validate_for_app(name)?;
    }
    Ok(())
}

pub(crate) fn validate_app_for_update(app: &AppLiveState) -> Result<(), AppError> {
    app.mode_state()
        .validate_for_update()
        .map_err(|_| unsupported_update())?;
    if app
        .written
        .as_ref()
        .is_some_and(|value| value.validate().is_err())
        || !app.stack.extra.is_empty()
    {
        return Err(unsupported_update());
    }
    if let Some(pending) = &app.pending {
        if !op::is_known(&pending.op)
            || !pending.extra.is_empty()
            || pending
                .files
                .iter()
                .any(|file| !file.extra.is_empty() || file.private.is_none())
            || !pending.target.extra.is_empty()
            || pending
                .target
                .written
                .as_ref()
                .is_some_and(|value| value.validate().is_err())
            || pending
                .target
                .stack
                .as_ref()
                .is_some_and(|value| !value.extra.is_empty())
        {
            return Err(unsupported_update());
        }
        if let Some(mode) = &pending.target.state {
            mode.validate_for_update()
                .map_err(|_| unsupported_update())?;
        }
    }
    Ok(())
}

/// Fallible in-memory changes are committed only after compatibility validation.
/// Callers must keep external effects out of this closure; it owns only state.
pub(crate) fn update<R>(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    change: impl FnOnce(&mut LiveState) -> Result<R, AppError>,
) -> Result<R, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let before = load(store, vault)?;
    let mut state = before.clone();
    let result = change(&mut state)?;
    validate_change(&before, &state)?;
    state.apps.retain(|_, app| !app.is_empty());
    save(store, vault, &state)?;
    Ok(result)
}

pub(crate) fn pending(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<Option<Pending>, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(selected_app(&read_preserved(store, vault)?, app)?.and_then(|state| state.pending))
}

pub(crate) fn set_pending(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
    pending: Option<Pending>,
) -> Result<(), AppError> {
    update_app(store, vault, app, |state| {
        state.pending = pending;
        Ok(())
    })
}

/// Read several app modes from one authenticated snapshot.
pub(crate) fn mode_states<const N: usize>(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    apps: [&str; N],
) -> Result<[ModeState; N], AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let state = read_preserved(store, vault)?;
    let selected = apps.map(|app| {
        selected_app(&state, app)
            .map(|entry| entry.map(|entry| entry.mode_state()).unwrap_or_default())
    });
    // Keep a fallible result for each requested app without typing its peers.
    let mut selected = selected.into_iter();
    let mut failure = None;
    let modes = std::array::from_fn(|_| match selected.next().expect("fixed app count") {
        Ok(mode) => mode,
        Err(error) => {
            failure = Some(error);
            ModeState::default()
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(modes),
    }
}

pub(crate) fn mode_state(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<ModeState, AppError> {
    let [mode] = mode_states(store, vault, [app])?;
    Ok(mode)
}

/// None denotes that this version has not written this app's native files.
pub(crate) fn written(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<Option<Written>, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(selected_app(&read_preserved(store, vault)?, app)?.and_then(|state| state.written))
}

pub(crate) fn stack(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<StackState, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(selected_app(&read_preserved(store, vault)?, app)?
        .map(|state| state.stack)
        .unwrap_or_default())
}

pub(crate) fn stack_mode(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &str,
) -> Result<bool, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(selected_app(&read_preserved(store, vault)?, app)?
        .is_some_and(|state| state.mode == Some(Mode::Proxy) && state.stack.enabled))
}

pub(crate) fn apps_with_pending(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<Vec<String>, AppError> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(load(store, vault)?
        .apps
        .into_iter()
        .filter(|(_, state)| state.pending.is_some())
        .map(|(app, _)| app)
        .collect())
}
