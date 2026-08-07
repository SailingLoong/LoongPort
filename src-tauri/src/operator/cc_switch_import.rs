//! 从 cc-switch 一键导入（**拷贝，不是迁移**）。
//!
//! ## 源库全程只读
//!
//! `~/.cc-switch/cc-switch.db` 用 `SQLITE_OPEN_READ_ONLY` 打开，导入只写 LoongPort 自己的
//! 库。**绝不动源库** —— 这是「导入」不是「迁移」，cc-switch 可能还在被 cc-switch app 用。
//! 集成测试用「导入前后源文件字节一致」钉着这条。
//!
//! ## 覆盖式：复用上游导入路径
//!
//! providers / MCP / prompts / skills 以 cc-switch 为准整体替换，走
//! [`Database::import_sql_string_preserving`]（备份 + 原子替换 + 迁移 + authorizer +
//! 版本校验全在里头）；`loongport_operator` / `loongport_vendor` / `settings` 通过
//! preserve 保住；本地**托管档位**的 provider 记录（`loongport-*`）在导入后回填。
//!
//! ## 冲突归属：`域名 + sk` 指纹
//!
//! 同指纹（`(origin, sk)`，base_url 归一化到 origin 再比）的 cc-switch provider 与托管
//! 档位 ⇒ 托管侧胜，那条不导入、报告列出。**指纹只用于导入这一刻比一次**，不建唯一索引
//! —— sk 会变（provision「只换 sk」），身份仍是派生 provider_id，见 `TODO.md` 冲突归属规则。
//!
//! ## 与「已手动维护」（`is_user_edited`）的解耦
//!
//! 导入**不改写任何 settings_config**：cc-switch 的按原样入库；托管档位回填走裸
//! [`Database::save_provider`]（不做 `ProviderService::add` 那套 normalize / live 写入）。
//! `is_user_edited` 是纯内容函数（比对 settings_config 与再生成的默认值，sk 除外），
//! 不认来源 —— 所以导入不改变任何档位的「已手动维护」判定：跟默认没差别的照样不显示，
//! 有差别的照样显示。集成测试用「回填后 is_user_edited 不变」钉着这条。

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};

/// 导入时保留的本地表：LoongPort 的两张明文凭据表 + settings。
///
/// - `loongport_operator` / `loongport_vendor`：登录态 / 明文 sk，cc-switch 里没有这两张
///   表，不保留 = 被替换成空表。
/// - `settings`：LoongPort 的 current-provider / config snippet，不该被 cc-switch 的覆盖
///   （cc-switch 的 current-provider 指它自己的 provider id，照搬会造成悬空指针）。
const PRESERVE_TABLES: &[&str] = &["loongport_operator", "loongport_vendor", "settings"];

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
    pub source_version: Option<i64>,
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
    /// 因与托管档位同指纹而跳过不导入的。
    pub skipped: Vec<SkippedProvider>,
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
/// 比之前 base_url **必须归一化到 origin**：cc-switch 侧是 `https://bestapi.store/v1`
/// （带 path），托管侧是 `site_origin`（`https://bestapi.store`），不归一化全漏检。
///
/// 返回 `None` = 取不到（base_url / sk 提取失败，或这个 CLI 还没接线）—— 那条原样导入、
/// 不参与冲突检测。
fn fingerprint_of(provider: &Provider, app_type: &AppType) -> Option<(String, String)> {
    let base_url = crate::proxy::providers::get_adapter(app_type)
        .extract_base_url(provider)
        .ok()?;
    let origin = crate::operator::api::normalize_site_origin(&base_url).ok()?;
    let sk = crate::operator::provision::extract_api_key(&provider.settings_config, app_type)?;
    if origin.is_empty() || sk.is_empty() {
        return None;
    }
    Some((origin, sk))
}

/// 把读到的 source provider 按「是否与托管档位同指纹」分类。
///
/// 返回三个下标集合：`will_import`（指纹存在且不冲突）、`skipped`（指纹命中托管档位）、
/// `cannot_fingerprint`（指纹取不到，原样导入）。按 app_type 分组比：托管档位在
/// codex / anthropic / gemini / grok 各平台是**不同**的 key（`key_name_for` 带 platform），
/// 所以一条 cc-switch codex provider 只跟托管 codex 行比。
fn classify_source(
    source: &[SourceProvider],
    managed: &[SourceProvider],
) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut managed_fp: HashMap<AppType, HashSet<(String, String)>> = HashMap::new();
    for m in managed {
        if let Some(fp) = fingerprint_of(&m.provider, &m.app_type) {
            managed_fp.entry(m.app_type.clone()).or_default().insert(fp);
        }
    }

    let mut will_import = Vec::new();
    let mut skipped = Vec::new();
    let mut cannot_fingerprint = Vec::new();
    for (i, s) in source.iter().enumerate() {
        match fingerprint_of(&s.provider, &s.app_type) {
            Some(fp)
                if managed_fp
                    .get(&s.app_type)
                    .is_some_and(|set| set.contains(&fp)) =>
            {
                skipped.push(i)
            }
            Some(_) => will_import.push(i),
            None => cannot_fingerprint.push(i),
        }
    }
    (will_import, skipped, cannot_fingerprint)
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
    let sort_index: Option<usize> = row.get(7)?;
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
/// 必须回填 —— 它们的 `settings_config` 里存着 sk（运营商档位的 sk 只在这一处）。
fn read_managed_rows(db: &Database) -> Result<Vec<SourceProvider>, AppError> {
    let mut out = Vec::new();
    for app_type in AppType::all() {
        let providers = db.get_all_providers(app_type.as_str())?;
        for (id, provider) in providers {
            if !crate::operator::is_managed(&id) {
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
            source_version: None,
            providers: ProviderPlan {
                will_import: 0,
                skipped: Vec::new(),
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
    let (will_import, skipped, cannot_fingerprint) = classify_source(&source, &managed);

    let version: i64 = conn
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .unwrap_or(0);

    let skipped_list = skipped
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    let mut notes = Vec::new();
    if !cannot_fingerprint.is_empty() {
        notes.push(format!(
            "{n} 条 provider 取不到指纹（base_url / sk 提取失败，\
             或 hermes / opencode / openclaw 尚未接线），将原样导入、不参与冲突合并",
            n = cannot_fingerprint.len()
        ));
    }

    Ok(ImportPlan {
        source_exists: true,
        source_version: Some(version),
        providers: ProviderPlan {
            will_import: will_import.len() + cannot_fingerprint.len(),
            skipped: skipped_list,
            cannot_fingerprint: cannot_fingerprint.len(),
        },
        mcp_servers: count_table_if_exists(&conn, "mcp_servers"),
        prompts: count_table_if_exists(&conn, "prompts"),
        skills: count_table_if_exists(&conn, "skills"),
        notes,
    })
}

/// 执行导入。返回报告；失败时（含源库不可读 / 版本不兼容）返回 Err，用户可凭
/// `restore_db_backup` 恢复 —— 导入前的备份由 `import_sql_string_preserving` 自动建。
pub fn execute_import(db: Arc<Database>, source_path: &Path) -> Result<ImportReport, AppError> {
    if !source_path.exists() {
        return Err(AppError::Config(
            "未检测到 cc-switch 数据（~/.cc-switch/cc-switch.db）。".to_string(),
        ));
    }

    let conn = open_source_read_only(source_path)?;
    let source = read_source(&conn)?;
    let managed = read_managed_rows(&db)?;
    let (will_import, skipped, cannot_fingerprint) = classify_source(&source, &managed);

    let mcp = count_table_if_exists(&conn, "mcp_servers");
    let prompts = count_table_if_exists(&conn, "prompts");
    let skills = count_table_if_exists(&conn, "skills");

    // 源库里既没有 provider 也没有 MCP ⇒ 没什么可搬的，别走进导入路径
    // （`validate_cc_switch_sql_export` 对 provider/mcp 全空会报错，而那是「没东西」不是错）。
    if source.is_empty() && mcp == 0 {
        return Ok(ImportReport {
            success: true,
            backup_id: String::new(),
            providers_imported: 0,
            providers_skipped: Vec::new(),
            mcp_imported: 0,
            prompts_imported: 0,
            skills_imported: 0,
            warnings: Vec::new(),
        });
    }

    // 覆盖式导入：dump 源库（只读）→ 走同一条导入路径。备份 + 原子替换 + 迁移 +
    // authorizer + 版本校验全在 `import_sql_string_preserving` 里。
    let sql = Database::dump_sql(&conn, &[])?;
    let backup_id = db.import_sql_string_preserving(&sql, PRESERVE_TABLES)?;

    // 回填托管档位 + 删掉与托管档位同指纹的 cc-switch 重复行。
    //
    // ⚠️ 回填走**裸** `save_provider`，不做 `ProviderService::add` 的 normalize ——
    // settings_config 必须原样写回，否则「已手动维护」的纯内容判定会被改掉。
    let mut warnings = Vec::new();
    for m in &managed {
        if let Err(e) = db.save_provider(m.app_type.as_str(), &m.provider) {
            warnings.push(format!("托管档位「{}」回填失败: {e}", m.provider.name));
            log::error!(
                "[cc-switch-import] 回填托管档位「{}」失败: {e}",
                m.provider.name
            );
        }
    }
    for &i in &skipped {
        let s = &source[i];
        if let Err(e) = db.delete_provider(s.app_type.as_str(), &s.provider.id) {
            warnings.push(format!("删除重复档位「{}」失败: {e}", s.provider.name));
            log::error!(
                "[cc-switch-import] 删除重复档位「{}」失败: {e}",
                s.provider.name
            );
        }
    }

    if let Err(e) = crate::commands::sync_support::run_post_import_sync(db) {
        warnings.push(format!("导入后同步失败: {e}"));
        log::warn!("[cc-switch-import] post-import sync: {e}");
    }

    let skipped_list = skipped
        .iter()
        .map(|&i| SkippedProvider {
            name: source[i].provider.name.clone(),
            app_type: source[i].app_type.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    Ok(ImportReport {
        success: true,
        backup_id,
        providers_imported: will_import.len() + cannot_fingerprint.len(),
        providers_skipped: skipped_list,
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
    use crate::operator::provision;
    use rusqlite::Connection;
    use serde_json::json;
    use serial_test::serial;

    /// 造一份「codex 形状」的 settings_config（auth.OPENAI_API_KEY + config TOML）。
    fn codex_settings(base_url: &str, sk: &str) -> Value {
        provision::settings_config_for(&AppType::Codex, sk, "BestAPI", base_url, "gpt-5.6-sol")
            .expect("codex 必须有形状")
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
        // `https://bestapi.store/v1`（带 path）与裸 `https://bestapi.store` 必须归一成同一个。
        let a = provider(
            "a",
            "A",
            codex_settings("https://bestapi.store/v1", "sk-1"),
            Some("https://bestapi.store"),
        );
        let b = provider(
            "b",
            "B",
            codex_settings("https://bestapi.store", "sk-1"),
            Some("https://bestapi.store"),
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
            codex_settings("https://bestapi.store/v1", "sk-1"),
            Some("https://bestapi.store"),
        );
        let b = provider(
            "b",
            "B",
            codex_settings("https://bestapi.store/v1", "sk-2"),
            Some("https://bestapi.store"),
        );
        assert_ne!(
            fingerprint_of(&a, &AppType::Codex),
            fingerprint_of(&b, &AppType::Codex),
            "同站不同 sk 必须算不同指纹 —— 单看域名会把多个档位误并成一个"
        );
    }

    #[test]
    fn fingerprint_is_none_when_sk_is_missing() {
        // grokbuild 还没接线（extract_api_key 返回 None）⇒ 取不到指纹。
        let p = provider("a", "A", json!({"config": "[models]\n..."}), None);
        assert_eq!(fingerprint_of(&p, &AppType::GrokBuild), None);
    }

    #[test]
    fn classify_skips_managed_fingerprint_and_keeps_the_rest() {
        let managed = [SourceProvider {
            app_type: AppType::Codex,
            provider: provider(
                "loongport-aaaaaaaaaaaaaaaa",
                "托管档",
                codex_settings("https://bestapi.store/v1", "sk-managed"),
                Some("https://bestapi.store"),
            ),
        }];

        let source = vec![
            SourceProvider {
                app_type: AppType::Codex,
                // 同站同 sk ⇒ 命中托管 ⇒ 跳过。
                provider: provider(
                    "bestapi",
                    "BestAPI",
                    codex_settings("https://bestapi.store/v1", "sk-managed"),
                    Some("https://bestapi.store"),
                ),
            },
            SourceProvider {
                app_type: AppType::Codex,
                // 同站不同 sk ⇒ 不是同一个东西 ⇒ 导入。
                provider: provider(
                    "bestapi-2",
                    "BestAPI 2",
                    codex_settings("https://bestapi.store/v1", "sk-other"),
                    Some("https://bestapi.store"),
                ),
            },
            SourceProvider {
                app_type: AppType::GrokBuild,
                // 取不到指纹 ⇒ 原样导入。
                provider: provider("grok-x", "GrokX", json!({"config": "x"}), None),
            },
        ];

        let (will, skipped, cannot) = classify_source(&source, &managed);
        assert_eq!(will, vec![1], "不同 sk 的那条该导入");
        assert_eq!(skipped, vec![0], "同指纹那条该跳过");
        assert_eq!(cannot, vec![2], "取不到指纹那条该归入 cannot");
    }

    #[test]
    fn classify_is_per_app_type_not_global() {
        // 托管档位在 claude 栏的同 sk，不该让 codex 栏的同 sk 被误并 —— 各平台是不同 key。
        let managed = [SourceProvider {
            app_type: AppType::Claude,
            provider: provider(
                "loongport-bbbbbbbbbbbbbbbb",
                "托管 Claude",
                json!({"env": {"ANTHROPIC_BASE_URL": "https://bestapi.store/anthropic", "ANTHROPIC_AUTH_TOKEN": "sk-x"}}),
                Some("https://bestapi.store"),
            ),
        }];
        let source = [SourceProvider {
            app_type: AppType::Codex,
            provider: provider(
                "bestapi",
                "BestAPI",
                codex_settings("https://bestapi.store/v1", "sk-x"),
                Some("https://bestapi.store"),
            ),
        }];
        let (will, skipped, _) = classify_source(&source, &managed);
        assert_eq!(
            skipped,
            Vec::<usize>::new(),
            "codex 与 claude 是不同平台，不该跨栏并"
        );
        assert_eq!(will, vec![0]);
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
    fn create_source_db(path: &std::path::Path) {
        let conn = Connection::open(path).expect("建源库");
        conn.execute_batch("PRAGMA user_version=16;").unwrap();

        create_providers_table(&conn);
        // 与托管档位（sk-managed @ bestapi.store）同指纹 ⇒ 导入时跳过。
        insert_provider(
            &conn,
            "codex",
            &provider(
                "bestapi",
                "BestAPI",
                codex_settings("https://bestapi.store/v1", "sk-managed"),
                Some("https://bestapi.store"),
            ),
        );
        // 不同 sk ⇒ 导入。
        insert_provider(
            &conn,
            "codex",
            &provider(
                "other",
                "Other",
                codex_settings("https://bestapi.store/v1", "sk-other"),
                Some("https://bestapi.store"),
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
            "INSERT INTO settings (key, value) VALUES ('currentProviderCodex', 'bestapi')",
            [],
        )
        .unwrap();
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

        // ── LoongPort 侧：内存库 + 一条托管 codex 档位 + operator 行 + settings ──
        let db = Arc::new(Database::memory().expect("内存库"));
        let managed_settings = codex_settings("https://bestapi.store/v1", "sk-managed");
        db.save_provider(
            "codex",
            &provider(
                "loongport-aaaaaaaaaaaaaaaa",
                "托管档",
                managed_settings.clone(),
                Some("https://bestapi.store"),
            ),
        )
        .unwrap();
        {
            let conn = crate::database::lock_conn!(db.conn);
            crate::operator::creds::save_site(
                &conn,
                "https://bestapi.store",
                "BestAPI",
                "https://bestapi.store/v1",
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
        create_source_db(src.path());
        let before = std::fs::read(src.path()).expect("读源库字节");

        let report = {
            let _guard = TestHomeGuard::set(tempfile::tempdir().unwrap().path());
            execute_import(db.clone(), src.path()).expect("导入不该失败")
        };

        // 1. 源库只读 —— 导入不是迁移。
        let after = std::fs::read(src.path()).expect("重读源库字节");
        assert_eq!(after, before, "cc-switch.db 绝不能被改动");

        // 2. providers：非冲突的进来了、托管档位回填了、同指纹的没进来。
        let providers = db.get_all_providers("codex").expect("读 codex 档位");
        assert!(
            providers.contains_key("other"),
            "非冲突的 cc-switch provider 该被导入"
        );
        assert!(
            providers.contains_key("loongport-aaaaaaaaaaaaaaaa"),
            "托管档位该被回填"
        );
        assert!(
            !providers.contains_key("bestapi"),
            "与托管档位同指纹的条目不导入"
        );

        // 3. LoongPort 自己的表 / settings 保留。
        {
            let conn = crate::database::lock_conn!(db.conn);
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM loongport_operator", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "loongport_operator 该原样保留");
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

        // 5. 报告。
        assert!(report.success);
        assert_eq!(report.providers_imported, 1, "只有 other 一条该导入");
        assert_eq!(report.providers_skipped.len(), 1);
        assert_eq!(report.providers_skipped[0].name, "BestAPI");
        assert_eq!(report.mcp_imported, 1, "MCP 该搬进来");
        Ok(())
    }
}
