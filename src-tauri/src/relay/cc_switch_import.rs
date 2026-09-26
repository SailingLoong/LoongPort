//! 从 cc-switch 一键导入（**拷贝，不是迁移**）。
//!
//! ## 源库全程只读
//!
//! `~/.cc-switch/cc-switch.db` 用 `SQLITE_OPEN_READ_ONLY` 打开，导入只写 LoongPort 自己的
//! 库。**绝不动源库** —— 这是「导入」不是「迁移」，cc-switch 可能还在被 cc-switch app 用。
//! 集成测试用「导入前后源文件字节一致」钉着这条。
//!
//! ## 版本闸：cc-switch 比本仓新时收起入口
//!
//! cc-switch 升级可能把源库 `user_version` 推到本仓 [`SCHEMA_VERSION`] 之后，导入路径的
//! 迁移层会拒（「数据库版本过新」）。与其让用户点了才见报错，预览直接给最终事实
//! `can_import = user_version ≤ SCHEMA_VERSION`，前端据此隐藏全部入口 —— 判据与迁移闸
//! 同一个常量，跟上游吸收新 schema 后入口自动恢复，不用改前端。
//!
//! ## 覆盖式：复用上游导入路径
//!
//! providers / MCP / prompts / skills 以 cc-switch 为准整体替换，走
//! [`Database::import_sql_string_from_cc_switch`]（备份 + 原子替换 + 迁移 + authorizer +
//! 版本校验全在里头）；本地站点、设置和代理运行配置通过 preserve 保住。
//! 本地**托管档位**的 provider 记录（`loongport-*`）在暂存库发布前回填。
//!
//! ## 去重以完整配置为准
//!
//! 只有同应用、同连接身份且 `settings_config` / `meta` 完全一致的托管档位才去重。
//! 同站点或同密钥不能证明配置相同：源配置里的模型、角色映射和协议选择必须保留。
//! 完全相同的条目按站点归属计入 `merged_to_relay` 或 `skipped`；其他条目原样导入。
//!
//! ## 与「已手动维护」（`user_edited`）的解耦
//!
//! 导入**不改写任何 settings_config**：cc-switch 的按原样入库；托管档位回填走裸
//! [`Database::save_provider`]（不做 `ProviderService::add` 那套 normalize / live 写入）。
//! `user_edited` 是 providers 表上的**存库列**，只在用户手工编辑时置位、恢复默认时复位，
//! `save_provider` 不碰它 —— 所以导入不改变任何档位的「已手动维护」判定。集成测试用
//! 「回填后 `get_user_edited` 仍为 false」钉着这条。

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

use crate::app_config::AppType;
use crate::database::{Database, SCHEMA_VERSION};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::relay::provider_fingerprint;

/// Import preserves local accounts, preferences and proxy runtime configuration.
///
/// - `loongport_relay` / `loongport_vendor`：登录态 / 明文 sk，cc-switch 里没有这两张
///   表，不保留 = 被替换成空表。
/// - `proxy_config` / `proxy_live_backup`: this process owns its listener, takeover and restore state.
/// - `settings`：LoongPort 的 current-provider / config snippet，不该被 cc-switch 的覆盖
///   （cc-switch 的 current-provider 指它自己的 provider id，照搬会造成悬空指针）。
const PRESERVE_TABLES: &[&str] = &[
    "loongport_relay",
    "loongport_vendor",
    "settings",
    "proxy_config",
    "proxy_live_backup",
];

/// 一条被收编（跳过不导入）的 cc-switch provider。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedProvider {
    pub name: String,
    pub app_type: String,
}

/// 预览：导入前给用户看「会搬什么、跳过什么」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub source_exists: bool,
    /// 这份源库**当前应用能不能导**：源库在，且 `user_version` ≤ 内置 [`SCHEMA_VERSION`]
    /// （与导入路径里迁移层版本闸同一个常量）。前端据此决定入口显隐 —— cc-switch 比
    /// 本仓新时静默收起入口，别把「数据库版本过新」留给用户点了之后才弹。
    pub can_import: bool,
    pub providers: ProviderPlan,
    pub mcp_servers: i64,
    pub prompts: i64,
    pub skills: i64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPlan {
    /// 会导入的 provider 条数（含取不到指纹、原样导入的那些）。
    pub will_import: usize,
    /// 与已有托管档位的连接、模型和协议配置完全相同，无需重复导入。
    pub skipped: Vec<SkippedProvider>,
    /// 与已登录中转站的托管档位配置完全相同，已归并的条目。
    pub merged_to_relay: Vec<SkippedProvider>,
    /// 取不到指纹（base_url / sk 提取失败）的条数，这些原样导入、不参与冲突检测。
    pub cannot_fingerprint: usize,
}

/// 导入结果报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub success: bool,
    /// 导入前自动建的备份文件名（空串 = 无源数据、没走导入）。
    pub backup_id: String,
    pub providers_imported: usize,
    pub providers_skipped: Vec<SkippedProvider>,
    /// 与已登录中转站的托管配置完全相同，已归并的条目。
    pub relays_merged: Vec<SkippedProvider>,
    pub mcp_imported: i64,
    pub prompts_imported: i64,
    pub skills_imported: i64,
    /// 非致命问题（回填失败 / 后置同步失败之类），导入仍算成功但用户该知道。
    pub warnings: Vec<String>,
}

/// cc-switch 源库的路径。
///
/// cc-switch 的配置目录是 `~/.cc-switch/`（它的 `APP_DIR_NAME`），与我们
/// `~/.loongport/` 完全隔离 —— 这份隔离是有意的、别改成共用一个库
/// （见 `TODO.md`「一键从 cc-switch 同步配置与数据」的警告）。
pub fn cc_switch_db_path() -> std::path::PathBuf {
    crate::config::get_home_dir()
        .join(".cc-switch")
        .join("cc-switch.db")
}

/// cc-switch 的一条 provider（带它所属的 app_type）。
struct SourceProvider {
    app_type: AppType,
    provider: Provider,
}

/// 从一份 `settings_config` 里读出的「指纹」。
///
/// 判据是 `域名 + sk` 合起来（TODO.md 冲突归属规则）：单看 sk 会撞（不同站点的 key 格式
/// 相同）、单看域名会把同站点的多个档位误并成一个。
///
/// 比之前 base_url **必须归一化到 origin**：cc-switch 侧是 `https://provider.example/v1`
/// （带 path），托管侧是 `site_origin`（`https://provider.example`），不归一化全漏检。
///
/// 返回 `None` = 取不到（base_url / sk 提取失败，或这个 CLI 还没接线）—— 那条原样导入、
/// 不参与冲突检测。
fn fingerprint_of(provider: &Provider, app_type: &AppType) -> Option<(String, String)> {
    provider_fingerprint::for_provider(provider, app_type)
}

/// 一条 cc-switch provider 的分类结果。四类互斥。
#[derive(Debug, Default)]
struct SourceClass {
    /// 取到指纹且不与托管档位冲突 —— 原样导入。
    will_import: Vec<usize>,
    /// 与已有托管档位配置完全相同，跳过重复条目。
    skipped: Vec<usize>,
    /// 与已登录中转站的托管配置完全相同，跳过导入。
    merged_to_relay: Vec<usize>,
    /// 取不到指纹 —— 原样导入、不参与冲突检测。
    cannot_fingerprint: Vec<usize>,
}

/// Deduplicate only equivalent configurations. A shared origin or credential
/// identifies a connection, not the model/protocol settings the user authored.
fn classify_source(
    source: &[SourceProvider],
    managed: &[SourceProvider],
    relay_origins: &HashSet<String>,
) -> SourceClass {
    let mut out = SourceClass::default();
    for (i, source) in source.iter().enumerate() {
        let Some(fingerprint) = fingerprint_of(&source.provider, &source.app_type) else {
            out.cannot_fingerprint.push(i);
            continue;
        };
        let duplicate = managed.iter().any(|managed| {
            managed.app_type == source.app_type
                && fingerprint_of(&managed.provider, &managed.app_type).as_ref()
                    == Some(&fingerprint)
                && managed.provider.settings_config == source.provider.settings_config
                && serde_json::to_value(&managed.provider.meta).ok()
                    == serde_json::to_value(&source.provider.meta).ok()
        });
        if !duplicate {
            out.will_import.push(i);
        } else if source_origin(source).is_some_and(|origin| relay_origins.contains(&origin)) {
            out.merged_to_relay.push(i);
        } else {
            out.skipped.push(i);
        }
    }
    out
}

/// source provider 的 base_url 归一化 origin（站点归并判据用）。
fn source_origin(s: &SourceProvider) -> Option<String> {
    let base_url = crate::proxy::providers::get_adapter(&s.app_type)?
        .extract_base_url(&s.provider)
        .ok()?;
    crate::relay::sub2api::normalize_site_origin(&base_url).ok()
}

/// 已登录中转站的 `api_base_url` 归一化 origin 集合（站点归并判据）。
///
/// `api_base_url` 是「归一后的 codex base_url（带 /v1）」（见 `creds.rs` 模块文档），
/// 与 cc-switch provider 的 base_url 经 `normalize_site_origin` 后可比。
fn managed_relay_origins(db: &Database) -> Result<HashSet<String>, AppError> {
    let vault = db.secrets.read()?;
    let conn = db.conn.lock().unwrap();
    let ops = crate::relay::creds::list(&conn, &vault)?;
    Ok(ops
        .iter()
        .filter_map(|op| crate::relay::sub2api::normalize_site_origin(&op.api_base_url).ok())
        .collect())
}

/// 把一条 `providers` 行还原成 `Provider`。
///
/// 列序与 SELECT 是一份契约（同 `database/dao/providers.rs` 的 `get_all_providers`），
/// `settings_config` / `meta` 解不出 JSON 时回落空值而不是报错 —— 一条坏记录不该让整个
/// 导入中止（同 `loongport_schema.rs` 迁移对坏记录的态度）。
fn provider_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Provider> {
    // 列序以 `read_source` 的 SELECT 为准：第 1 列是 `app_type`（读方单独取走），
    // 所以这里跳过它、从 0 跳到 2。
    let id: String = row.get(0)?;
    let name: String = row.get(2)?;
    let settings_config_str: String = row.get(3)?;
    let website_url: Option<String> = row.get(4)?;
    let category: Option<String> = row.get(5)?;
    let created_at: Option<i64> = row.get(6)?;
    let sort_index: Option<usize> = row.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as usize);
    let notes: Option<String> = row.get(8)?;
    let icon: Option<String> = row.get(9)?;
    let icon_color: Option<String> = row.get(10)?;
    let meta_str: String = row.get(11)?;
    let in_failover_queue: bool = row.get(12)?;

    let settings_config = serde_json::from_str(&settings_config_str).unwrap_or(Value::Null);
    let meta: ProviderMeta = serde_json::from_str(&meta_str).unwrap_or_default();

    Ok(Provider {
        id,
        name,
        settings_config,
        website_url,
        category,
        created_at,
        sort_index,
        notes,
        meta: Some(meta),
        icon,
        icon_color,
        in_failover_queue,
        available_models: None,
    })
}

/// 只读打开 cc-switch 库。
///
/// **必须只读** —— 导入不写源库。`SQLITE_OPEN_READ_ONLY` 之外不加别的 flag，
/// 让 SQLite 对源文件连写锁都不拿。
fn open_source_read_only(path: &Path) -> Result<Connection, AppError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| AppError::Database(format!("无法打开 cc-switch 数据库: {e}")))
}

/// 读 cc-switch 库的全部 providers（各 app_type）+ 三张可搬表的行数。
fn read_source(conn: &Connection) -> Result<Vec<SourceProvider>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, app_type, name, settings_config, website_url, category, created_at, \
                    sort_index, notes, icon, icon_color, meta, in_failover_queue
             FROM providers",
        )
        .map_err(|e| AppError::Database(format!("读取 cc-switch providers 失败: {e}")))?;
    let mapped = stmt
        .query_map([], |row| {
            let app_type_str: String = row.get(1)?;
            let provider = provider_from_row(row)?;
            Ok((app_type_str, provider))
        })
        .map_err(|e| AppError::Database(format!("读取 cc-switch providers 失败: {e}")))?;

    let mut out = Vec::new();
    for r in mapped {
        let (app_type_str, provider) = r.map_err(|e| AppError::Database(e.to_string()))?;
        // cc-switch 的 app_type 理论上都能认（同源 fork）；认不出就跳过并记一条日志，
        // 别让一条未知平台让整个导入中止。
        match app_type_str.parse::<AppType>() {
            Ok(app_type) => out.push(SourceProvider { app_type, provider }),
            Err(e) => log::warn!("[cc-switch-import] 跳过未知 app_type '{app_type_str}': {e}"),
        }
    }
    Ok(out)
}

/// 读本地库的托管档位 provider 记录（`loongport-*`）。这些在覆盖式导入后会被替换掉，
/// 必须回填 —— 它们的 `settings_config` 里存着 sk（中转站档位的 sk 只在这一处）。
fn read_managed_rows(db: &Database) -> Result<Vec<SourceProvider>, AppError> {
    let mut out = Vec::new();
    for app_type in AppType::all() {
        let providers = db.get_all_providers(app_type.as_str())?;
        for (id, provider) in providers {
            if !crate::relay::is_managed(&id) {
                continue;
            }
            out.push(SourceProvider {
                app_type: app_type.clone(),
                provider,
            });
        }
    }
    Ok(out)
}

fn count_table_if_exists(conn: &Connection, table: &str) -> i64 {
    let has: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has == 0 {
        return 0;
    }
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap_or(0)
}

/// 预览导入：读源库 + 本地托管行，算冲突，但不写任何东西。
pub fn plan_import(db: &Database, source_path: &Path) -> Result<ImportPlan, AppError> {
    if !source_path.exists() {
        return Ok(ImportPlan {
            source_exists: false,
            can_import: false,
            providers: ProviderPlan {
                will_import: 0,
                skipped: Vec::new(),
                merged_to_relay: Vec::new(),
                cannot_fingerprint: 0,
            },
            mcp_servers: 0,
            prompts: 0,
            skills: 0,
            notes: Vec::new(),
        });
    }

    let conn = open_source_read_only(source_path)?;
    let source = read_source(&conn)?;
    let managed = read_managed_rows(db)?;
    let relay_origins = managed_relay_origins(db)?;
    let classified = classify_source(&source, &managed, &relay_origins);

    let version: i64 = conn
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .unwrap_or(0);

    let skipped_list = classified
        .skipped
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();
    let merged_list = classified
        .merged_to_relay
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    let mut notes = Vec::new();
    if !classified.cannot_fingerprint.is_empty() {
        notes.push(format!(
            "{n} 条 provider 取不到指纹（base_url / sk 提取失败，\
             或 hermes / opencode / openclaw 尚未接线），将原样导入、不参与冲突合并",
            n = classified.cannot_fingerprint.len()
        ));
    }

    Ok(ImportPlan {
        source_exists: true,
        can_import: version <= i64::from(SCHEMA_VERSION),
        providers: ProviderPlan {
            will_import: classified.will_import.len() + classified.cannot_fingerprint.len(),
            skipped: skipped_list,
            merged_to_relay: merged_list,
            cannot_fingerprint: classified.cannot_fingerprint.len(),
        },
        mcp_servers: count_table_if_exists(&conn, "mcp_servers"),
        prompts: count_table_if_exists(&conn, "prompts"),
        skills: count_table_if_exists(&conn, "skills"),
        notes,
    })
}

/// Values captured under the import lock, before any database publication.
struct ImportApplicationState {
    app: AppType,
    current: Option<Provider>,
    local_current_id: Option<String>,
    taken_over: bool,
}

fn import_application_states(
    state: &crate::store::AppState,
) -> Result<Vec<ImportApplicationState>, AppError> {
    AppType::all()
        .map(|app| {
            let local_current_id = crate::settings::get_current_provider(&app);
            let current = crate::settings::get_effective_current_provider(&state.db, &app)?
                .map(|id| state.db.get_provider_by_id(&id, app.as_str()))
                .transpose()?
                .flatten();
            let taken_over = if app.supports_local_proxy() {
                futures::executor::block_on(state.db.get_proxy_config_for_app(app.as_str()))?
                    .enabled
                    || futures::executor::block_on(state.db.get_live_backup(app.as_str()))?
                        .is_some()
                    || state
                        .proxy_service
                        .detect_takeover_in_live_config_for_app(&app)
            } else {
                false
            };
            Ok(ImportApplicationState {
                app,
                current,
                local_current_id,
                taken_over,
            })
        })
        .collect()
}

/// 执行导入。返回报告；失败时（含源库不可读 / 版本不兼容）返回 Err，用户可凭
/// `restore_db_backup` 恢复 —— 导入前的备份由 `import_sql_string_from_cc_switch` 自动建。
pub fn execute_import(
    app_state: crate::store::AppState,
    source_path: &Path,
) -> Result<ImportReport, AppError> {
    let db = app_state.db.clone();
    if !source_path.exists() {
        return Err(AppError::Config(
            "未检测到 cc-switch 数据（~/.cc-switch/cc-switch.db）。".to_string(),
        ));
    }

    let mut conn = open_source_read_only(source_path)?;
    // Classification and the exported SQL must observe the same source revision,
    // even when the source application edits its database during the import.
    let source_transaction = conn
        .transaction()
        .map_err(|error| AppError::Database(error.to_string()))?;
    let source = read_source(&source_transaction)?;
    let import_guard =
        futures::executor::block_on(app_state.proxy_service.lock_configuration_import());
    let applications = import_application_states(&app_state)?;
    let managed = read_managed_rows(&db)?;
    let relay_origins = managed_relay_origins(&db)?;
    let classified = classify_source(&source, &managed, &relay_origins);
    let mut existing = std::collections::HashSet::new();
    for app in AppType::all() {
        for id in db.get_all_providers(app.as_str())?.keys() {
            existing.insert((app.as_str().to_owned(), id.clone()));
        }
    }

    let mcp = count_table_if_exists(&source_transaction, "mcp_servers");
    let prompts = count_table_if_exists(&source_transaction, "prompts");
    let skills = count_table_if_exists(&source_transaction, "skills");

    // 源库里既没有 provider 也没有 MCP ⇒ 没什么可搬的，别走进导入路径
    // （`validate_cc_switch_sql_export` 对 provider/mcp 全空会报错，而那是「没东西」不是错）。
    if source.is_empty() && mcp == 0 {
        return Ok(ImportReport {
            success: true,
            backup_id: String::new(),
            providers_imported: 0,
            providers_skipped: Vec::new(),
            relays_merged: Vec::new(),
            mcp_imported: 0,
            prompts_imported: 0,
            skills_imported: 0,
            warnings: Vec::new(),
        });
    }

    // 覆盖式导入：dump 源库（只读）→ 走同一条导入路径。备份 + 原子替换 + 迁移 +
    // authorizer + 版本校验全在 `import_sql_string_from_cc_switch` 里。
    let sql = Database::dump_sql(&source_transaction, &[])?;
    source_transaction
        .commit()
        .map_err(|error| AppError::Database(error.to_string()))?;
    let mut targets = Vec::new();
    let backup_id = db.import_sql_string_from_cc_switch(&sql, PRESERVE_TABLES, |staged| {
        // Remove source duplicates before restoring local managed rows, including
        // the case where a source happens to use the same provider ID.
        for &index in classified.skipped.iter().chain(&classified.merged_to_relay) {
            let source = &source[index];
            staged.delete_provider(source.app_type.as_str(), &source.provider.id)?;
        }
        for managed in &managed {
            staged.save_provider(managed.app_type.as_str(), &managed.provider)?;
        }
        for &index in classified.will_import.iter().chain(&classified.cannot_fingerprint) {
            let imported = &source[index];
            if !existing.contains(&(imported.app_type.as_str().to_owned(), imported.provider.id.clone())) {
                crate::proxy::application_routing::note_provider_created(staged, imported.app_type.as_str(), &imported.provider.id)?;
            }
        }
        for before in &applications {
            let retained = before.current.as_ref().map(|provider| {
                staged.get_provider_by_id(&provider.id, before.app.as_str())
            }).transpose()?.flatten();
            let target = match retained {
                Some(provider) => Some(provider),
                None => staged.get_current_provider(before.app.as_str())?
                    .map(|id| staged.get_provider_by_id(&id, before.app.as_str())).transpose()?.flatten(),
            };
            if before.taken_over && target.is_none() {
                return Err(AppError::Config(format!(
                    "Import would remove the current provider for active {} routing; select a replacement before importing",
                    before.app.as_str(),
                )));
            }
            if let Some(provider) = &target {
                staged.set_current_provider(before.app.as_str(), &provider.id)?;
            }
            let previous_model = before.current.as_ref().and_then(|provider| {
                crate::relay::provider_config::selected_model(&before.app, &provider.settings_config)
            });
            let next_model = target.as_ref().and_then(|provider| {
                crate::relay::provider_config::selected_model(&before.app, &provider.settings_config)
            });
            if before.current.as_ref().map(|provider| &provider.id) != target.as_ref().map(|provider| &provider.id)
                || previous_model != next_model {
                crate::proxy::auto_strategy::set_model_pref(staged, before.app.as_str(), None)?;
            }
            if let Some(provider) = &target {
                crate::services::ProviderService::validate_imported_current_provider(
                    staged, &before.app, provider, before.taken_over,
                )?;
            }
            targets.push(target);
        }
        Ok(())
    })?;

    let mut warnings = Vec::new();
    for (before, target) in applications.iter().zip(&targets) {
        if let Err(error) = crate::services::ProviderService::sync_imported_current_provider(
            &app_state,
            &before.app,
            target.as_ref(),
            before.current.as_ref(),
            before.local_current_id.as_deref(),
            before.taken_over,
        ) {
            warnings.push(format!("导入后同步 {} 失败: {error}", before.app.as_str()));
        }
    }
    drop(import_guard);
    #[cfg(feature = "gui")]
    if let Err(e) = crate::commands::sync_support::run_post_import_sync_after_providers(&app_state)
    {
        warnings.push(format!("导入后同步失败: {e}"));
        log::warn!("[cc-switch-import] post-import sync: {e}");
    }

    let skipped_list = classified
        .skipped
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();
    let merged_list = classified
        .merged_to_relay
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    Ok(ImportReport {
        success: true,
        backup_id,
        providers_imported: classified.will_import.len() + classified.cannot_fingerprint.len(),
        providers_skipped: skipped_list,
        relays_merged: merged_list,
        mcp_imported: mcp,
        prompts_imported: prompts,
        skills_imported: skills,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;

    use rusqlite::Connection;
    use serde_json::json;
    use serial_test::serial;
    use std::sync::Arc;

    /// 造一份「codex 形状」的 settings_config（auth.OPENAI_API_KEY + config TOML）。
    fn codex_settings(base_url: &str, sk: &str) -> Value {
        crate::relay::provider_config::settings_config_for(
            &AppType::Codex,
            sk,
            "Example Provider",
            base_url,
            "gpt-5.6-sol",
        )
        .expect("codex 必须有形状")
    }

    /// 造一份「grokbuild 形状」的 settings_config（sk 藏在 config 字段的 TOML 文本里）。
    fn grok_settings(base_url: &str, sk: &str) -> Value {
        crate::relay::provider_config::settings_config_for(
            &AppType::GrokBuild,
            sk,
            "GrokX",
            base_url,
            "grok-4.5",
        )
        .expect("grokbuild 必须有形状")
    }

    fn provider(
        id: &str,
        name: &str,
        settings_config: Value,
        website_url: Option<&str>,
    ) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config,
            website_url: website_url.map(str::to_string),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
            available_models: None,
        }
    }

    /// 一个空库上的 `providers` 表（与 `database/schema.rs` 同形），造 cc-switch fixture 用。
    fn create_providers_table(conn: &Connection) {
        conn.execute(
            "CREATE TABLE providers (
                id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                name TEXT NOT NULL,
                settings_config TEXT NOT NULL,
                website_url TEXT,
                category TEXT,
                created_at INTEGER,
                sort_index INTEGER,
                notes TEXT,
                icon TEXT,
                icon_color TEXT,
                meta TEXT NOT NULL DEFAULT '{}',
                is_current BOOLEAN NOT NULL DEFAULT 0,
                in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
                PRIMARY KEY (id, app_type)
            )",
            [],
        )
        .unwrap();
    }

    fn insert_provider(conn: &Connection, app_type: &str, p: &Provider) {
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, website_url, meta)
             VALUES (?1, ?2, ?3, ?4, ?5, '{}')",
            params![
                p.id,
                app_type,
                p.name,
                serde_json::to_string(&p.settings_config).unwrap(),
                p.website_url
            ],
        )
        .unwrap();
    }

    #[test]
    fn fingerprint_normalizes_base_url_to_origin() {
        // `https://provider.example/v1`（带 path）与裸 `https://provider.example` 必须归一成同一个。
        let a = provider(
            "a",
            "A",
            codex_settings("https://provider.example/v1", "sk-1"),
            Some("https://provider.example"),
        );
        let b = provider(
            "b",
            "B",
            codex_settings("https://provider.example", "sk-1"),
            Some("https://provider.example"),
        );
        assert_eq!(
            fingerprint_of(&a, &AppType::Codex),
            fingerprint_of(&b, &AppType::Codex),
            "同站同 sk、只是 base_url 一个带 path 一个不带，必须算同一个指纹"
        );
    }

    #[test]
    fn fingerprint_distinguishes_different_sks_on_the_same_origin() {
        let a = provider(
            "a",
            "A",
            codex_settings("https://provider.example/v1", "sk-1"),
            Some("https://provider.example"),
        );
        let b = provider(
            "b",
            "B",
            codex_settings("https://provider.example/v1", "sk-2"),
            Some("https://provider.example"),
        );
        assert_ne!(
            fingerprint_of(&a, &AppType::Codex),
            fingerprint_of(&b, &AppType::Codex),
            "同站不同 sk 必须算不同指纹 —— 单看域名会把多个档位误并成一个"
        );
    }

    #[test]
    fn fingerprint_is_none_when_sk_is_missing() {
        // grokbuild 的 sk 藏在 config 字段的 TOML 文本里：坏 TOML 解析不出，
        // env_key 形状（凭据在进程环境变量里）则按定义不参与指纹。
        let broken = provider("a", "A", json!({"config": "not toml {"}), None);
        assert_eq!(fingerprint_of(&broken, &AppType::GrokBuild), None);

        let env_key_only = provider(
            "b",
            "B",
            json!({"config": "[models]\ndefault = \"p\"\n\n[model.p]\nmodel = \"m\"\nbase_url = \"https://grok.example/v1\"\nname = \"n\"\nenv_key = \"GROK_SK\"\napi_backend = \"openai-compliant\"\ncontext_window = 1000000\n"}),
            None,
        );
        assert_eq!(fingerprint_of(&env_key_only, &AppType::GrokBuild), None);
    }

    /// grokbuild 接线后，指纹必须从 TOML 文本里提出 (origin, sk)：
    /// 同站同 sk 归一成同一指纹、同站不同 sk 分开 —— 与 codex 同一套判据。
    #[test]
    fn fingerprint_works_for_grokbuild_toml_shape() {
        let a = provider(
            "a",
            "A",
            grok_settings("https://grok.example/v1", "sk-1"),
            Some("https://grok.example"),
        );
        let b = provider(
            "b",
            "B",
            grok_settings("https://grok.example", "sk-1"),
            Some("https://grok.example"),
        );
        assert_eq!(
            fingerprint_of(&a, &AppType::GrokBuild),
            fingerprint_of(&b, &AppType::GrokBuild),
            "同站同 sk、base_url 带不带 path 必须算同一个指纹"
        );

        let c = provider(
            "c",
            "C",
            grok_settings("https://grok.example/v1", "sk-2"),
            Some("https://grok.example"),
        );
        assert_ne!(
            fingerprint_of(&a, &AppType::GrokBuild),
            fingerprint_of(&c, &AppType::GrokBuild),
            "同站不同 sk 必须算不同指纹"
        );
    }

    /// 托管 Grok 档位与导入源配置完全相同，只保留已有档位。
    /// （接线前 grokbuild 取不到指纹，这条判重对它不生效）。
    #[test]
    fn edited_source_configuration_is_not_discarded_as_a_managed_duplicate() {
        let managed = [SourceProvider {
            app_type: AppType::Codex,
            provider: provider(
                "loongport-managed",
                "Managed",
                codex_settings("https://relay.example/v1", "test-key"),
                None,
            ),
        }];
        let mut edited = managed[0].provider.clone();
        edited.id = "custom".into();
        edited.settings_config["config"] = edited.settings_config["config"]
            .as_str()
            .unwrap()
            .replace("gpt-5.6-sol", "custom-model")
            .into();
        let source = [SourceProvider {
            app_type: AppType::Codex,
            provider: edited,
        }];
        for origins in [
            HashSet::new(),
            HashSet::from(["https://relay.example".to_string()]),
        ] {
            let plan = classify_source(&source, &managed, &origins);
            assert_eq!(plan.will_import, vec![0]);
            assert!(plan.skipped.is_empty());
            assert!(plan.merged_to_relay.is_empty());
        }
    }

    #[test]
    fn classify_dedups_grokbuild_by_fingerprint() {
        let managed = [SourceProvider {
            app_type: AppType::GrokBuild,
            provider: provider(
                "loongport-aaaaaaaaaaaaaaaa",
                "托管档",
                grok_settings("https://grok.example/v1", "sk-managed"),
                Some("https://grok.example"),
            ),
        }];
        let source = vec![
            SourceProvider {
                app_type: AppType::GrokBuild,
                provider: provider(
                    "grok-dup",
                    "GrokDup",
                    grok_settings("https://grok.example/v1", "sk-managed"),
                    Some("https://grok.example"),
                ),
            },
            SourceProvider {
                app_type: AppType::GrokBuild,
                provider: provider(
                    "grok-own",
                    "GrokOwn",
                    grok_settings("https://grok.example/v1", "sk-other"),
                    Some("https://grok.example"),
                ),
            },
        ];

        let out = classify_source(&source, &managed, &HashSet::new());
        assert_eq!(out.will_import, vec![1], "不同 sk 的那条该导入");
        assert_eq!(out.skipped, vec![0], "相同配置无需重复导入");
        assert!(out.cannot_fingerprint.is_empty());
        assert!(out.merged_to_relay.is_empty());
    }

    #[test]
    fn classify_skips_managed_fingerprint_and_keeps_the_rest() {
        let managed = [SourceProvider {
            app_type: AppType::Codex,
            provider: provider(
                "loongport-aaaaaaaaaaaaaaaa",
                "托管档",
                codex_settings("https://provider.example/v1", "sk-managed"),
                Some("https://provider.example"),
            ),
        }];

        let source = vec![
            SourceProvider {
                app_type: AppType::Codex,
                // 与已有托管配置完全相同，无需重复导入。
                provider: provider(
                    "example-provider",
                    "Example Provider",
                    codex_settings("https://provider.example/v1", "sk-managed"),
                    Some("https://provider.example"),
                ),
            },
            SourceProvider {
                app_type: AppType::Codex,
                // 同站不同 sk ⇒ 不是同一个东西 ⇒ 导入。
                provider: provider(
                    "example-provider-2",
                    "Example Provider 2",
                    codex_settings("https://provider.example/v1", "sk-other"),
                    Some("https://provider.example"),
                ),
            },
            SourceProvider {
                app_type: AppType::GrokBuild,
                // 取不到指纹 ⇒ 原样导入。
                provider: provider("grok-x", "GrokX", json!({"config": "x"}), None),
            },
        ];

        let out = classify_source(&source, &managed, &HashSet::new());
        assert_eq!(out.will_import, vec![1], "不同 sk 的那条该导入");
        assert_eq!(out.skipped, vec![0], "相同配置无需重复导入");
        assert_eq!(
            out.cannot_fingerprint,
            vec![2],
            "取不到指纹那条该归入 cannot"
        );
        assert!(out.merged_to_relay.is_empty(), "没登录中转站，不该有归并");
    }

    #[test]
    fn classify_preserves_distinct_configurations_on_an_existing_relay() {
        // A known relay does not own independently authored client configurations.
        let relay_origins: HashSet<String> = ["https://api.relay.example".to_string()]
            .into_iter()
            .collect();
        let source = [
            SourceProvider {
                app_type: AppType::Codex,
                // 同站、sk 与任何托管档位都不同 —— 旧逻辑会当成新 provider 导入。
                provider: provider(
                    "example-relay",
                    "Example Relay",
                    codex_settings("https://api.relay.example/v1", "sk-a90e"),
                    Some("https://api.relay.example"),
                ),
            },
            SourceProvider {
                app_type: AppType::Codex,
                // 别的站点 ⇒ 照常导入。
                provider: provider(
                    "other",
                    "Other",
                    codex_settings("https://provider.example/v1", "sk-b"),
                    Some("https://provider.example"),
                ),
            },
        ];

        let out = classify_source(&source, &[], &relay_origins);
        assert!(out.merged_to_relay.is_empty());
        assert_eq!(out.will_import, vec![0, 1]);
    }

    #[test]
    fn classify_preserves_a_distinct_api_path_on_an_existing_relay() {
        // 中转站存的是裸 origin，cc-switch 那条带 `/v1` —— 归一后必须命中。
        let relay_origins: HashSet<String> = ["https://api.relay.example".to_string()]
            .into_iter()
            .collect();
        let source = [SourceProvider {
            app_type: AppType::Claude,
            provider: provider(
                "example-relay-claude",
                "Example Relay Claude",
                json!({"env": {"ANTHROPIC_BASE_URL": "https://api.relay.example/anthropic", "ANTHROPIC_AUTH_TOKEN": "sk-z"}}),
                Some("https://api.relay.example"),
            ),
        }];
        let out = classify_source(&source, &[], &relay_origins);
        assert!(out.merged_to_relay.is_empty());
        assert_eq!(out.will_import, vec![0]);
    }

    #[test]
    fn classify_is_per_app_type_not_global() {
        // 托管档位在 claude 栏的同 sk，不该让 codex 栏的同 sk 被误并 —— 各平台是不同 key。
        let managed = [SourceProvider {
            app_type: AppType::Claude,
            provider: provider(
                "loongport-bbbbbbbbbbbbbbbb",
                "托管 Claude",
                json!({"env": {"ANTHROPIC_BASE_URL": "https://provider.example/anthropic", "ANTHROPIC_AUTH_TOKEN": "sk-x"}}),
                Some("https://provider.example"),
            ),
        }];
        let source = [SourceProvider {
            app_type: AppType::Codex,
            provider: provider(
                "example-provider",
                "Example Provider",
                codex_settings("https://provider.example/v1", "sk-x"),
                Some("https://provider.example"),
            ),
        }];
        let out = classify_source(&source, &managed, &HashSet::new());
        assert_eq!(
            out.skipped,
            Vec::<usize>::new(),
            "codex 与 claude 是不同平台，不该跨栏并"
        );
        assert_eq!(out.will_import, vec![0]);
    }

    // ─── 集成测试 ────────────────────────────────────────────

    /// 把 `CC_SWITCH_TEST_HOME` 指到临时目录并在测试结束时恢复 —— 导入路径里的备份 /
    /// 设置读写走 `get_home_dir()`，不指到临时目录就会碰真机数据。
    struct TestHomeGuard(Option<std::ffi::OsString>);
    impl TestHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", path);
            TestHomeGuard(prev)
        }
    }
    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    /// 建一个 cc-switch 源库文件：与托管档位同指纹的 codex provider、一个不同 sk 的、
    /// 一条 MCP、一条 cc-switch 自己的 settings（不该盖掉 LoongPort 的）。
    /// `user_version` 由调用方给 —— 版本闸测试要拿「比本仓新 / 在范围内」两种形态。
    fn create_source_db(path: &std::path::Path, user_version: i64) {
        let conn = Connection::open(path).expect("建源库");
        conn.execute_batch(&format!("PRAGMA user_version={user_version};"))
            .unwrap();

        create_providers_table(&conn);
        // 与托管档位连接和设置完全相同，导入时跳过重复条目。
        insert_provider(
            &conn,
            "codex",
            &provider(
                "example-provider",
                "Example Provider",
                codex_settings("https://provider.example/v1", "sk-managed"),
                Some("https://provider.example"),
            ),
        );
        // 同站点不同凭据仍是独立配置，必须导入。
        insert_provider(
            &conn,
            "codex",
            &provider(
                "other",
                "Other",
                codex_settings("https://provider.example/v1", "sk-other"),
                Some("https://provider.example"),
            ),
        );
        // 别的站点，与任何中转站 / 托管档位都不沾 ⇒ 照常导入。
        insert_provider(
            &conn,
            "codex",
            &provider(
                "elsewhere",
                "Elsewhere",
                codex_settings("https://other-vendor.example/v1", "sk-elsewhere"),
                Some("https://other-vendor.example"),
            ),
        );

        conn.execute(
            "CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL,
                description TEXT, homepage TEXT, docs TEXT, tags TEXT NOT NULL DEFAULT '[]',
                enabled_claude BOOLEAN NOT NULL DEFAULT 0, enabled_codex BOOLEAN NOT NULL DEFAULT 0,
                enabled_gemini BOOLEAN NOT NULL DEFAULT 0, enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
                enabled_opencode BOOLEAN NOT NULL DEFAULT 0, enabled_hermes BOOLEAN NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (id, name, server_config) VALUES ('mcp-1', 'MCP One', '{}')",
            [],
        )
        .unwrap();

        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('currentProviderCodex', 'example-provider')",
            [],
        )
        .unwrap();
    }

    /// cc-switch 比本仓新（源库 `user_version` 超过 [`SCHEMA_VERSION`]）⇒ 预览必须给
    /// `can_import=false` —— 前端据此隐藏全部入口，别让用户点了才见「数据库版本过新」。
    /// 纯读路径（不碰 home 目录），不用 `#[serial]`。
    #[test]
    fn plan_import_gates_on_source_schema_version() {
        let db = Database::memory().expect("内存库");

        // 比本仓支持的新：上游又发版推了 schema。
        let newer = tempfile::NamedTempFile::new().unwrap();
        create_source_db(newer.path(), i64::from(SCHEMA_VERSION) + 1);
        let plan = plan_import(&db, newer.path()).expect("预览不该失败");
        assert!(plan.source_exists, "源库在");
        assert!(!plan.can_import, "超版本的源库必须判不可导");

        // 恰好等于 SCHEMA_VERSION：可导（≤ 是闭区间）。
        let current = tempfile::NamedTempFile::new().unwrap();
        create_source_db(current.path(), i64::from(SCHEMA_VERSION));
        let plan = plan_import(&db, current.path()).expect("预览不该失败");
        assert!(plan.can_import, "等于 SCHEMA_VERSION 必须可导");

        // 没有源库：不可导（入口本就不显）。
        let plan = plan_import(&db, Path::new("/nonexistent/cc-switch.db")).expect("预览不该失败");
        assert!(!plan.source_exists);
        assert!(!plan.can_import);
    }

    fn source_with_current_provider(path: &Path, selected: &Provider, current: bool) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "user_version", 16).unwrap();
        create_providers_table(&conn);
        insert_provider(&conn, "claude", selected);
        if current {
            conn.execute("UPDATE providers SET is_current=1", [])
                .unwrap();
        }
    }

    fn imported_claude(model: &str) -> Provider {
        provider(
            "imported",
            "Imported",
            json!({"env": {
                "ANTHROPIC_BASE_URL":"https://provider.example/v1",
                "ANTHROPIC_AUTH_TOKEN":"fixture-key",
                "ANTHROPIC_MODEL":model,
                "ANTHROPIC_DEFAULT_HAIKU_MODEL":"old-model"
            }}),
            None,
        )
    }

    #[test]
    #[serial]
    fn repeated_direct_import_preserves_the_selected_model_in_native_configuration() {
        let home = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(home.path());
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let state = crate::store::AppState::new(db.clone()).unwrap();
        let original = imported_claude("default-model");
        db.save_provider("claude", &original).unwrap();
        db.set_current_provider("claude", &original.id).unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(&original.id)).unwrap();
        crate::proxy::auto_strategy::set_model_pref(&db, "claude", Some("old-model")).unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        let mut edited = original.clone();
        edited.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!("https://updated.example/v1");
        source_with_current_provider(source.path(), &edited, true);

        execute_import(state, source.path()).unwrap();

        let live: Value =
            crate::config::read_json_file(&crate::config::get_claude_settings_path()).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "old-model");
        assert_eq!(
            live["env"]["ANTHROPIC_BASE_URL"],
            "https://updated.example/v1"
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref(&db, "claude").as_deref(),
            Some("old-model")
        );
        assert_eq!(
            db.get_provider_by_id(&original.id, "claude")
                .unwrap()
                .unwrap()
                .settings_config["env"]["ANTHROPIC_MODEL"],
            "default-model"
        );
    }

    #[test]
    #[serial]
    fn import_rejects_official_or_blocked_active_targets_before_publication() {
        let home = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(home.path());
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let state = crate::store::AppState::new(db.clone()).unwrap();
        let original = imported_claude("original-model");
        db.save_provider("claude", &original).unwrap();
        db.set_current_provider("claude", &original.id).unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(&original.id)).unwrap();
        let mut runtime =
            futures::executor::block_on(db.get_proxy_config_for_app("claude")).unwrap();
        runtime.enabled = true;
        futures::executor::block_on(db.update_proxy_config_for_app(runtime)).unwrap();
        futures::executor::block_on(
            db.save_live_backup("claude", &original.settings_config.to_string()),
        )
        .unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        source_with_current_provider(source.path(), &imported_claude("replacement-model"), true);
        Connection::open(source.path())
            .unwrap()
            .execute("UPDATE providers SET category='official'", [])
            .unwrap();
        assert!(execute_import(state.clone(), source.path()).is_err());
        assert_eq!(
            db.get_provider_by_id(&original.id, "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            original.settings_config
        );

        let mut blocked = imported_claude("blocked-model");
        blocked.id = "blocked".into();
        db.save_provider("claude", &blocked).unwrap();
        crate::proxy::application_routing::set_tier_blocked(&db, "claude", &blocked.id, true)
            .unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        source_with_current_provider(source.path(), &blocked, true);
        assert!(execute_import(state, source.path()).is_err());
        assert_eq!(
            db.get_current_provider("claude").unwrap().as_deref(),
            Some(original.id.as_str())
        );
        let backup = futures::executor::block_on(db.get_live_backup("claude"))
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&backup.original_config).unwrap(),
            original.settings_config
        );
    }

    #[test]
    #[serial]
    fn repeated_import_keeps_local_takeover_and_projects_the_changed_current_model() {
        let home = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(home.path());
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let state = crate::store::AppState::new(db.clone()).unwrap();
        let original = imported_claude("old-model");
        db.save_provider("claude", &original).unwrap();
        db.set_current_provider("claude", &original.id).unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(&original.id)).unwrap();
        crate::proxy::auto_strategy::set_model_pref(&db, "claude", Some("old-model")).unwrap();
        let mut runtime =
            futures::executor::block_on(db.get_proxy_config_for_app("claude")).unwrap();
        runtime.enabled = true;
        runtime.auto_failover_enabled = true;
        futures::executor::block_on(db.update_proxy_config_for_app(runtime)).unwrap();
        futures::executor::block_on(
            db.save_live_backup("claude", &original.settings_config.to_string()),
        )
        .unwrap();
        let mut taken_over = original.settings_config.clone();
        taken_over["env"]["ANTHROPIC_BASE_URL"] = json!("http://127.0.0.1:15721");
        taken_over["env"]["ANTHROPIC_AUTH_TOKEN"] = json!("PROXY_MANAGED");
        crate::config::write_json_file(&crate::config::get_claude_settings_path(), &taken_over)
            .unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        source_with_current_provider(source.path(), &original, true);
        execute_import(state.clone(), source.path()).unwrap();
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref(&db, "claude").as_deref(),
            Some("old-model")
        );
        let edited = imported_claude("new-model");
        Connection::open(source.path())
            .unwrap()
            .execute(
                "UPDATE providers SET settings_config=?1 WHERE id='imported'",
                [edited.settings_config.to_string()],
            )
            .unwrap();
        let source_before = std::fs::read(source.path()).unwrap();
        execute_import(state, source.path()).unwrap();
        let runtime = futures::executor::block_on(db.get_proxy_config_for_app("claude")).unwrap();
        assert!(runtime.enabled && runtime.auto_failover_enabled);
        let live: Value =
            crate::config::read_json_file(&crate::config::get_claude_settings_path()).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "new-model");
        assert_eq!(live["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:15721");
        let backup = futures::executor::block_on(db.get_live_backup("claude"))
            .unwrap()
            .unwrap();
        let backup: Value = serde_json::from_str(&backup.original_config).unwrap();
        assert_eq!(backup["env"]["ANTHROPIC_MODEL"], "new-model");
        assert_eq!(
            backup["env"]["ANTHROPIC_BASE_URL"],
            "https://provider.example/v1"
        );
        assert!(crate::proxy::auto_strategy::get_model_pref(&db, "claude").is_none());
        assert_eq!(std::fs::read(source.path()).unwrap(), source_before);
    }

    #[test]
    #[serial]
    fn import_rejects_removing_an_active_target_without_a_source_selection_before_publish() {
        let home = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(home.path());
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let state = crate::store::AppState::new(db.clone()).unwrap();
        let original = imported_claude("old-model");
        db.save_provider("claude", &original).unwrap();
        db.set_current_provider("claude", &original.id).unwrap();
        crate::settings::set_current_provider(&AppType::Claude, Some(&original.id)).unwrap();
        let mut runtime =
            futures::executor::block_on(db.get_proxy_config_for_app("claude")).unwrap();
        runtime.enabled = true;
        futures::executor::block_on(db.update_proxy_config_for_app(runtime)).unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        let mut other = imported_claude("new-model");
        other.id = "another".into();
        source_with_current_provider(source.path(), &other, false);
        let error = execute_import(state, source.path()).unwrap_err();
        assert!(error.to_string().contains("current provider"));
        assert_eq!(
            db.get_current_provider("claude").unwrap().as_deref(),
            Some("imported")
        );
        assert!(db
            .get_provider_by_id("another", "claude")
            .unwrap()
            .is_none());
        assert!(
            futures::executor::block_on(db.get_proxy_config_for_app("claude"))
                .unwrap()
                .enabled
        );
    }

    /// ⭐ 核心闸：导入把 cc-switch 的搬进来，同时托管档位回填、冲突项删掉、
    /// LoongPort 自己的表/settings 保留、**源库字节不变**、**「已手动维护」判定不变**。
    /// ⚠️ `#[serial]`：本测试要临时改进程级 `CC_SWITCH_TEST_HOME`（备份/设置读写用），
    /// 而 `opencode_config` / `openclaw_config` 等测试也在改同一个 env var ——
    /// 不加 serial 会让两者并发撞车（实测踩过，见 git log）。与其它改 env 的 serial 测试串行。
    #[test]
    #[serial]
    fn execute_import_merges_managed_tiers_and_keeps_source_read_only(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // ⚠️ **别在整条测试身上挂 `CC_SWITCH_TEST_HOME`** —— 那是个进程级 env var，
        // 而 `openclaw_config` / `grok_config` 等测试也在改它，整条占着会把并发撞车
        // 变成一个必然失败。只在 `execute_import` 期间指到临时目录，完事立刻还原
        // （见下面的作用域块）。

        // ── LoongPort 侧：内存库 + 一条托管 codex 档位 + relay 行 + settings ──
        let db = Arc::new(Database::memory().expect("内存库"));
        let managed_settings = codex_settings("https://provider.example/v1", "sk-managed");
        db.save_provider(
            "codex",
            &provider(
                "loongport-aaaaaaaaaaaaaaaa",
                "托管档",
                managed_settings.clone(),
                Some("https://provider.example"),
            ),
        )
        .unwrap();
        {
            let conn = crate::database::lock_conn!(db.conn);
            crate::relay::creds::save_site(
                &conn,
                "https://provider.example",
                "Example Provider",
                "https://provider.example/v1",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('loongport_keep', 'yes')",
                [],
            )
            .unwrap();
        }

        // ── cc-switch 源库文件 ──
        let src = tempfile::NamedTempFile::new().expect("临时源库");
        // 16（旧于当前 SCHEMA_VERSION）：顺带覆盖「导入路径把旧版本迁上来」的链路。
        create_source_db(src.path(), 16);
        let before = std::fs::read(src.path()).expect("读源库字节");

        let report = {
            let _guard = TestHomeGuard::set(tempfile::tempdir().unwrap().path());
            execute_import(crate::store::AppState::new(db.clone()).unwrap(), src.path())
                .expect("导入不该失败")
        };

        // 1. 源库只读 —— 导入不是迁移。
        let after = std::fs::read(src.path()).expect("重读源库字节");
        assert_eq!(after, before, "cc-switch.db 绝不能被改动");

        // 2. 不同配置导入，托管档位保留，相同配置跳过。
        let providers = db.get_all_providers("codex").expect("读 codex 档位");
        assert!(
            providers.contains_key("elsewhere"),
            "非冲突的 cc-switch provider 该被导入"
        );
        assert!(
            providers.contains_key("loongport-aaaaaaaaaaaaaaaa"),
            "托管档位该被回填"
        );
        assert!(
            !providers.contains_key("example-provider"),
            "站点已由中转站组维护的条目不导入"
        );
        assert!(
            providers.contains_key("other"),
            "A distinct credential must remain available after import"
        );
        let merged: Vec<&str> = report
            .relays_merged
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            merged.len(),
            1,
            "Only the equivalent configuration is merged: {merged:?}"
        );

        // 3. LoongPort 自己的表 / settings 保留。
        {
            let conn = crate::database::lock_conn!(db.conn);
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM loongport_relay", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "loongport_relay 该原样保留");
            let keep: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key='loongport_keep'",
                    [],
                    |r| r.get(0),
                )
                .ok();
            assert_eq!(
                keep.as_deref(),
                Some("yes"),
                "LoongPort 自己的 settings 该保留"
            );
            let cc: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key='currentProviderCodex'",
                    [],
                    |r| r.get(0),
                )
                .ok();
            assert!(
                cc.is_none(),
                "cc-switch 的 currentProvider 不该盖掉 LoongPort 的 settings"
            );
        }

        // 4. 「已手工维护」解耦：导入**不写** `user_edited` 存库标记 —— 那是「用户手工
        //    编辑」的专属来源（编辑页置位、恢复默认复位）。导入是拷贝不是编辑，
        //    回填的托管档位配置原样、标记仍为 false。
        let reinserted = providers.get("loongport-aaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(
            reinserted.settings_config, managed_settings,
            "回填不改 settings_config —— 导入不该改写托管档位的配置"
        );
        assert!(
            !db.get_user_edited("codex", "loongport-aaaaaaaaaaaaaaaa")
                .expect("读标记"),
            "导入不置「已手工维护」标记 —— 它只在用户手工编辑时置位"
        );

        let chain = crate::proxy::application_routing::chain_ids(&db, "codex").unwrap();
        assert_eq!(
            chain.first().map(String::as_str),
            Some("loongport-aaaaaaaaaaaaaaaa")
        );
        assert!(chain.contains(&"other".to_string()));
        assert!(chain.contains(&"elsewhere".to_string()));

        // 5. 报告。
        assert!(report.success);
        assert_eq!(report.providers_imported, 2);
        assert!(
            report.providers_skipped.is_empty(),
            "相同配置按中转站归属计入归并结果，不重复计入其他跳过项"
        );
        assert_eq!(report.mcp_imported, 1, "MCP 该搬进来");
        Ok(())
    }
}
