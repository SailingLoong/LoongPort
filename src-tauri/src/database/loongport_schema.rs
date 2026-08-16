//! LoongPort **自己的** schema 迁移，与上游 cc-switch 的完全分离。
//!
//! ## 为什么不能共用 `PRAGMA user_version`
//!
//! 本仓是 cc-switch 的 fork，要跟着上游升级。上游的迁移用 `user_version` 计数，
//! 而它**现在正好停在 16，下一步就是 17** —— 我们若也往那个计数器上追加，
//! 就是抢占上游的号段。撞上之后的后果是**静默数据损坏**，不是报错：
//!
//! ```text
//! 用户库 stamp = 17（我们建 relay 表时写的）
//!   ↓ 上游发布它自己的 v17（比如加一张表）
//! merge 上游 → schema.rs 里 `17 =>` 分支冲突，无论怎么解都不对：
//!   - 保我们的 ⇒ 上游那张表永远不建
//!   - 保上游的 ⇒ 我们的 relay 表永远不建
//!   - 两个都放 ⇒ 已经 stamp 到 17 的库跳过整段，两张表都不建
//! 而 `user_version` 显示「已是最新」，用户看不到任何异常，
//! 直到碰到用那张表的功能才崩。
//! ```
//!
//! 抬到高号段（1000+）能躲开碰撞，但那只是**赌上游不会用到那个数**，
//! 且两套迁移仍共用一个计数器 —— 谁先跑、谁把号推高，语义上纠缠不清。
//!
//! ## 做法：各记各的版本
//!
//! | 谁的迁移 | 版本存哪 |
//! |---|---|
//! | 上游 cc-switch | `PRAGMA user_version`（**原样归还，我们不再写它**）|
//! | LoongPort | 本模块的 `loongport_schema_version` 表 |
//!
//! 两者**互不影响**：上游把 `user_version` 推到 17、20、99 都与我们无关，
//! 我们加多少步也不会碰它。合并上游的迁移时只是普通的代码合并，不再有语义冲突。
//!
//! ## ⚠️ 加一步迁移的规矩
//!
//! 1. 在 [`LOONGPORT_SCHEMA_VERSION`] 上 +1；
//! 2. 在 [`apply`] 的 `match` 里加一个分支；
//! 3. **`create_tables_on_conn` 里同时把新形态建全** —— 全新库不走迁移链
//!    （那边先 `CREATE TABLE IF NOT EXISTS` 建成最终形态，迁移只服务已存在的库）。
//!
//! **发版之后不许再压紧编号**：那时真有库停在中间版本，压紧等于让它们跳到
//! 新版本号却没建对应的列。当前（2026-08-04）还在测试阶段，所以历史上那几步
//! 被合并成了 v1 —— 见本文件的 git 历史。

use rusqlite::Connection;

use crate::error::AppError;

/// LoongPort 自己的 schema 版本。加迁移时 +1。
///
/// **与 `SCHEMA_VERSION`（上游那个）无关**，两者各自独立计数。
pub(crate) const LOONGPORT_SCHEMA_VERSION: i32 = 15;

/// 存版本号的表。**只有一行**（`id = 1`）。
///
/// 用表而不是 `PRAGMA user_version`：那个 pragma 每库只有一个，已经归上游了。
/// SQLite 还有 `application_id`，但它的语义是「这个文件属于哪个应用」，
/// 拿来当版本号是误用（而且同样只有一个槽）。
const VERSION_TABLE: &str = "loongport_schema_version";

/// 建版本表（幂等）。必须在 [`apply`] 之前跑。
fn ensure_version_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {VERSION_TABLE} (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                version INTEGER NOT NULL
            )"
        ),
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 {VERSION_TABLE} 表失败: {e}")))?;
    Ok(())
}

/// 读当前版本。表不存在或没有行都返回 0（= 还没跑过任何 LoongPort 迁移）。
pub(crate) fn current_version(conn: &Connection) -> Result<i32, AppError> {
    ensure_version_table(conn)?;
    read_stored_version(conn)
}

/// 只读地读版本号：表不存在或没有行都算 0，**绝不建表**。
///
/// 与 [`current_version`] 的差别就是少那一步 `ensure_version_table` ——
/// 预检（[`stored_version_exceeds_supported`]）靠它守住「先于任何写入」。
fn read_stored_version(conn: &Connection) -> Result<i32, AppError> {
    let version: Option<i32> = conn
        .query_row(
            &format!("SELECT version FROM {VERSION_TABLE} WHERE id = 1"),
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(version.unwrap_or(0))
}

/// 启动预检：磁盘上的库是否带着比当前代码支持的更新的 LoongPort 版本。
///
/// [`apply`] 里也有同一道检查，但它跑在 `Database::init` 内部 —— 那时上游
/// `create_tables` 的 DDL 已经落盘，错过了「先于任何 schema 写入」的预检窗口；
/// 撞上时用户看到的是原生 Retry/Exit 对话框，而不是应用内升级恢复界面。
///
/// **只读不写**：以只读模式打开库文件，版本表不存在就当版本 0。
pub(crate) fn stored_version_exceeds_supported(
    db_path: &std::path::Path,
) -> Result<Option<i32>, AppError> {
    if !db_path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| AppError::Database(format!("只读打开数据库失败: {e}")))?;
    let version = read_stored_version(&conn)?;
    Ok((version > LOONGPORT_SCHEMA_VERSION).then_some(version))
}

fn set_version(conn: &Connection, version: i32) -> Result<(), AppError> {
    conn.execute(
        &format!("INSERT OR REPLACE INTO {VERSION_TABLE} (id, version) VALUES (1, ?1)"),
        [version],
    )
    .map_err(|e| AppError::Database(format!("写入 {VERSION_TABLE} 失败: {e}")))?;
    Ok(())
}

/// 跑 LoongPort 自己的迁移链。
///
/// **在上游那套迁移之后调用** —— LoongPort 的表可能引用上游的表，
/// 反过来则不会（上游不知道我们存在）。
pub(crate) fn apply(conn: &Connection) -> Result<(), AppError> {
    let mut version = current_version(conn)?;

    if version > LOONGPORT_SCHEMA_VERSION {
        return Err(AppError::Database(format!(
            "LoongPort 数据版本过新（{version}），当前应用仅支持 {LOONGPORT_SCHEMA_VERSION}，请升级应用后再尝试。"
        )));
    }

    while version < LOONGPORT_SCHEMA_VERSION {
        match version {
            // v0 → v1：建 LoongPort 的两张表。
            //
            // ⚠️ 这一步在全新库上是**空转** —— `create_tables_on_conn` 已经建过了
            // （它跑在迁移之前）。它真正服务的是「上游那套表已在、我们的还没建」
            // 那种库，即从 cc-switch 迁过来的用户。
            0 => {
                log::info!("LoongPort 数据迁移 v0 → v1（中转站凭据 + 官网直连账号两张表）");
                crate::relay::creds::create_table(conn)?;
                crate::vendor::creds::create_table(conn)?;
                set_version(conn, 1)?;
            }
            // v1 → v2：把纯生图档位从 codex 栏搬到 codex-image 栏。
            1 => {
                log::info!("LoongPort 数据迁移 v1 → v2（生图档位独立成一栏）");
                move_image_tiers_to_their_own_column(conn)?;
                set_version(conn, 2)?;
            }
            // v2 → v3：providers 加 `user_edited` 列 ——「已手工维护」从内容比对改成存库标记。
            //
            // ⚠️ 全新库也走这一步（`create_tables_on_conn` 建的是上游形态、没有这列），
            // 所以**不要**顺手把列加进上游 `schema.rs` 的 providers CREATE ——
            // 那会扩大与上游 merge 的接触面，而这列本来就是 LoongPort 自己的。
            2 => {
                log::info!(
                    "LoongPort 数据迁移 v2 → v3（「已手工维护」落库为 providers.user_edited）"
                );
                add_user_edited_column(conn)?;
                set_version(conn, 3)?;
            }
            // v3 → v4：`loongport_operator` 改名成 `loongport_relay`（术语统一到「中转站」）。
            3 => {
                log::info!("LoongPort 数据迁移 v3 → v4（loongport_operator → loongport_relay）");
                rename_operator_table_to_relay(conn)?;
                set_version(conn, 4)?;
            }
            // v4 → v5：模型验证结果（主动报告 + 为第二阶段预留的被动聚合列）。
            4 => {
                log::info!("LoongPort 数据迁移 v4 → v5（模型验证结果）");
                crate::relay::model_verification::store::create_results_table(conn)?;
                set_version(conn, 5)?;
            }
            // v5 → v6：运行时自动验证全局设置与代理接管租约。
            5 => {
                log::info!("LoongPort 数据迁移 v5 → v6（运行时验证设置与代理租约）");
                crate::relay::model_verification::store::create_legacy_runtime_tables(conn)?;
                set_version(conn, 6)?;
            }
            // v6 → v7：每个分组最近五条脱敏模型验证结果。
            // 旧表是当前状态快照，不是历史事件，不将它伪装成首条历史。
            6 => {
                log::info!("LoongPort 数据迁移 v6 → v7（模型验证历史）");
                crate::relay::model_verification::history::create_table(conn)?;
                set_version(conn, 7)?;
            }
            // v7 → v8：移除被动运行时验证数据，但保留尚未恢复的代理租约。
            // 租约只能在启动时成功恢复对应 app 的 live config 后删除。
            7 => {
                log::info!("LoongPort 数据迁移 v7 → v8（移除被动运行时验证数据）");
                remove_legacy_runtime_verification_state(conn)?;
                set_version(conn, 8)?;
            }
            // v8 → v9：记录 relay 的协议类型。历史行默认兼容为 sub2api。
            8 => {
                log::info!("LoongPort 数据迁移 v8 → v9（记录 relay backend_kind）");
                add_relay_backend_kind_column(conn)?;
                set_version(conn, 9)?;
            }
            // v9 → v10：记录登录时的真实 User-Agent（不再伪造）。
            9 => {
                log::info!("LoongPort 数据迁移 v9 → v10（loongport_relay 加 user_agent 列）");
                add_relay_user_agent_column(conn)?;
                set_version(conn, 10)?;
            }
            // v10 → v11：存下 Cloudflare 放行 cookie，让 reqwest 也能过托管挑战。
            10 => {
                log::info!("LoongPort 数据迁移 v10 → v11（loongport_relay 加 cf_clearance 列）");
                add_relay_cf_clearance_column(conn)?;
                set_version(conn, 11)?;
            }
            // v11 → v12：providers 加 `tier_rate_multiplier` 列 —— 档位倍率落库。
            //
            // 倍率原来根本不存，每次渲染都靠 `relay_list_tier_rates` 现拉（每个档位
            // 一次 HTTP），而那条命令在每次动作后的 reload 里都会跑。现在 provision
            // 时算好写下来，只有用户主动刷新才更新。与 `user_edited` 同一条路子：
            // LoongPort 自己的列，**不要**加进上游 `schema.rs` 的 providers CREATE。
            11 => {
                log::info!(
                    "LoongPort 数据迁移 v11 → v12（档位倍率落库为 providers.tier_rate_multiplier）"
                );
                add_tier_rate_multiplier_column(conn)?;
                set_version(conn, 12)?;
            }
            12 => {
                log::info!("LoongPort 数据迁移 v12 → v13（中转站倍率刷新时间）");
                add_relay_pricing_synced_at_column(conn)?;
                set_version(conn, 13)?;
            }
            // v13 → v14：中转站余额快照表（扣费对账的唯一采样点）。
            // 全新库由 `create_tables_on_conn` 直接建成本形态，这一步服务已停在 v13 的库。
            13 => {
                log::info!("LoongPort 数据迁移 v13 → v14（中转站余额快照表）");
                crate::relay::reconcile::create_table(conn)?;
                set_version(conn, 14)?;
            }
            // v14 → v15：清洗余额快照的存量重复行。写入侧已改成「只在余额有变时
            // 落行」（`record_balance_snapshot`），这一步把 v14 及之前攒下的
            // 「与上一条余额相同」的行删掉 —— 它们只会把对账窗口切成零信息行。
            14 => {
                log::info!("LoongPort 数据迁移 v14 → v15（余额快照去重：删余额未变的相邻行）");
                dedupe_balance_snapshots(conn)?;
                set_version(conn, 15)?;
            }
            other => {
                return Err(AppError::Database(format!(
                    "未知的 LoongPort 数据版本 {other}，无法迁移到 {LOONGPORT_SCHEMA_VERSION}"
                )));
            }
        }
        version = current_version(conn)?;
    }

    Ok(())
}

/// 删除 `relay_balance_snapshots` 里「与同站上一条余额相同」的行 —— 每个等值
/// 连续段只留最早一条，删完后相邻行必然相异（与 [`crate::relay::reconcile`]
/// 写入侧的去重口径一致）。
///
/// 一趟 `LAG` 就够：被删的行都等于其直接前驱，按序传递 ⇒ 段内全等，留下的
/// 最早一条与下一段的值必不同。
fn dedupe_balance_snapshots(conn: &Connection) -> Result<(), AppError> {
    let deleted = conn
        .execute(
            "DELETE FROM relay_balance_snapshots
             WHERE id IN (
                 SELECT id FROM (
                     SELECT id,
                            LAG(balance_usd) OVER (
                                PARTITION BY relay_id
                                ORDER BY created_at, id
                            ) AS prev_balance
                     FROM relay_balance_snapshots
                 )
                 WHERE prev_balance IS NOT NULL AND prev_balance = balance_usd
             )",
            [],
        )
        .map_err(|e| AppError::Database(format!("清洗余额快照重复行失败: {e}")))?;
    if deleted > 0 {
        log::info!("余额快照去重：删除 {deleted} 行零信息快照");
    }
    Ok(())
}

fn add_relay_backend_kind_column(conn: &Connection) -> Result<(), AppError> {
    if !crate::Database::table_exists(conn, "loongport_relay")? {
        return Ok(());
    }

    if crate::Database::has_column(conn, "loongport_relay", "backend_kind")? {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE loongport_relay
         ADD COLUMN backend_kind TEXT NOT NULL DEFAULT 'sub2api'",
        [],
    )
    .map_err(|e| AppError::Database(format!("给 loongport_relay 加 backend_kind 列失败: {e}")))?;
    Ok(())
}

fn add_relay_user_agent_column(conn: &Connection) -> Result<(), AppError> {
    if !crate::Database::table_exists(conn, "loongport_relay")? {
        return Ok(());
    }

    if crate::Database::has_column(conn, "loongport_relay", "user_agent")? {
        return Ok(());
    }

    conn.execute("ALTER TABLE loongport_relay ADD COLUMN user_agent TEXT", [])
        .map_err(|e| AppError::Database(format!("给 loongport_relay 加 user_agent 列失败: {e}")))?;
    Ok(())
}

fn add_relay_cf_clearance_column(conn: &Connection) -> Result<(), AppError> {
    if !crate::Database::table_exists(conn, "loongport_relay")? {
        return Ok(());
    }

    if crate::Database::has_column(conn, "loongport_relay", "cf_clearance")? {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE loongport_relay ADD COLUMN cf_clearance TEXT",
        [],
    )
    .map_err(|e| AppError::Database(format!("给 loongport_relay 加 cf_clearance 列失败: {e}")))?;
    Ok(())
}

fn add_relay_pricing_synced_at_column(conn: &Connection) -> Result<(), AppError> {
    if !crate::Database::table_exists(conn, "loongport_relay")? {
        return Ok(());
    }

    if crate::Database::has_column(conn, "loongport_relay", "pricing_synced_at")? {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE loongport_relay ADD COLUMN pricing_synced_at INTEGER",
        [],
    )
    .map_err(|e| {
        AppError::Database(format!(
            "给 loongport_relay 加 pricing_synced_at 列失败: {e}"
        ))
    })?;
    Ok(())
}

fn remove_legacy_runtime_verification_state(conn: &Connection) -> Result<(), AppError> {
    if crate::Database::table_exists(conn, "model_verification_history")?
        && crate::Database::table_exists(conn, "providers")?
    {
        conn.execute(
            "DELETE FROM model_verification_history WHERE source = 'runtime'",
            [],
        )
        .map_err(|error| AppError::Database(format!("删除旧运行时验证历史失败: {error}")))?;
    }

    if crate::Database::table_exists(conn, "model_verification_settings")? {
        conn.execute(
            "UPDATE model_verification_settings SET runtime_auto_enabled = 0",
            [],
        )
        .map_err(|error| AppError::Database(format!("关闭旧自动验证设置失败: {error}")))?;
    }

    Ok(())
}

/// 把纯生图档位（`model` 是 `gpt-image-*` 的托管 codex 档位）搬到 `codex-image` 栏。
///
/// ## 为什么需要迁移，而不是等用户点一次「获取密钥」
///
/// 不迁移的话，老档位会**滞留在 codex 栏**直到用户主动刷新，期间两处都能看到生图
/// 档位（codex 栏里那条是旧的、生图栏里空着）—— 比分栏之前更让人困惑。而用户没有
/// 理由知道要去点刷新。
///
/// ## 顺带洗掉被 switch 回填污染的配置
///
/// 这是本轮要治的症状：生图档位当过 `is_current` 的话，`ProviderService::switch` 切走时
/// 会把 live 的 `config.toml` 快照写回它（`[mcp_servers]` / `notify` / `[projects.*]` /
/// `experimental_bearer_token` 全拌进去）⇒ 与默认基准比对不上 ⇒ 界面显示「已手动维护」，
/// 而用户一个字没改过。
///
/// **判据是「已手工维护」（现在存库为 `providers.user_edited`，编辑页置位、恢复默认
/// 复位）而不是「配置里有没有那些键」**：后者要穷举污染源（漏一个就洗不干净）。
///
/// ## 幂等
///
/// 两条都幂等：`UPDATE ... WHERE app_type='codex'` 在第二次跑时已经没有匹配行；
/// 配置重写只在标记「未手工维护」且内容确实不同时发生。
///
/// ## 为什么走裸 SQL 而不是 `ProviderService`
///
/// 迁移跑在 `Database::init` 里，那时 `AppState` 还不存在（`ProviderService` 要它）。
/// 这也是上游全部迁移的做法。
fn move_image_tiers_to_their_own_column(conn: &Connection) -> Result<(), AppError> {
    use crate::relay::provision;

    // ⚠️ **`providers` 表不存在时直接返回**，不报错。
    //
    // 实际启动顺序里它一定在（上游的 `create_tables_on_conn` 跑在本模块之前），
    // 但迁移不该依赖那个假设 —— 一旦上游调整顺序，报错会让 `Database::init` 失败、
    // **app 起不来**，而这一步的语义本来就是「没有档位就没什么可搬的」。
    let has_providers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='providers'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_providers == 0 {
        return Ok(());
    }

    // 先挑出候选：codex 栏下的**托管**档位（`loongport-` 前缀）。
    // 非托管的手工 provider 不动 —— 用户自己配的 gpt-image 档位归他管，
    // 而生图栏只接受托管档位（`is_managed` 是生图工具读 sk 的前提）。
    let mut rows: Vec<(String, String, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, name, settings_config FROM providers WHERE app_type = 'codex'")
            .map_err(|e| AppError::Database(format!("查 codex 档位失败: {e}")))?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| AppError::Database(format!("查 codex 档位失败: {e}")))?;
        for r in mapped {
            rows.push(r.map_err(|e| AppError::Database(format!("读 codex 档位失败: {e}")))?);
        }
    }

    let mut moved = 0usize;
    for (id, name, settings_json) in rows {
        if !crate::relay::is_managed(&id) {
            continue;
        }
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&settings_json) else {
            // 配置存的不是合法 JSON（库被外部改过）⇒ 跳过。迁移不该因为一条坏记录中止，
            // 那会让 app 起不来。
            log::warn!("档位 {id} 的 settings_config 不是合法 JSON，迁移跳过它");
            continue;
        };
        let Some(model) = provision::extract_model(&settings) else {
            continue;
        };
        if !provision::is_image_model(&model) {
            continue;
        }

        // 只搬栏，**不重写配置**。
        //
        // 想过顺带洗掉回填污染（那是本轮症状的直接来源），但当时判不了「被回填污染」
        // 与「用户真改过」—— 现在「已手工维护」已存库（`providers.user_edited`），
        // 但这条迁移跑在库升级时，不该为一次历史数据搬移引入新依赖。
        //
        // 所以按代价定：错洗掉用户的编辑**不可逆**，而留着污染只是多一个「已手动维护」
        // 标记 —— 界面上那条档位有「恢复默认配置」按钮，一点就干净。宁可留标记。
        //
        // 而且分栏本身就止住了污染的源头：生图栏与 codex 栏各有自己的 `is_current`，
        // switch 的回填只碰自己栏里的档位，往后不会再有新的污染。

        // 换栏。主键是 `(id, app_type)`，所以这是一次真正的 UPDATE，不是 INSERT。
        //
        // ⚠️ **可能撞主键**：生图栏下已经有同 id 的行（用户在新版跑过一次 provision
        // 之后又回滚到旧版、再升上来）。那时新栏那条是更新的，删掉旧栏这条即可。
        let exists_in_new: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = ?1 AND app_type = 'codex-image'",
                [&id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let sql = if exists_in_new > 0 {
            "DELETE FROM providers WHERE id = ?1 AND app_type = 'codex'"
        } else {
            // ⚠️ **`is_current` 必须清零**（review 的探针抓出）。
            //
            // 生图档位在 codex 栏本来可能就是当前项 —— 那正是本轮要治的场景
            // （用户点错「启用」把聊天切成了生图档位）。原样带过去的话，生图栏会有
            // **两个** `is_current = 1`：一个来自 codex 栏的旧状态，一个由
            // `inherit_the_old_current_image_tier` 按旧 settings 键设的。
            //
            // 而生图 MCP 那侧是 `SELECT id … WHERE is_current = 1` 取第一行 ⇒ 拿到哪个
            // 取决于 SQLite 的返回顺序 ⇒ **用户选的是 4K 档、出的可能是 1K 的图**，
            // 且换台机器结果可能不同。实测探针：旧键指向 B，实际拿到 A。
            //
            // 清零之后「当前生图档位」只由 `inherit_the_old_current_image_tier` 一处决定。
            "UPDATE providers SET app_type = 'codex-image', is_current = 0 \
             WHERE id = ?1 AND app_type = 'codex'"
        };
        conn.execute(sql, [&id])
            .map_err(|e| AppError::Database(format!("把档位 {id} 搬到生图栏失败: {e}")))?;
        moved += 1;
        log::info!("生图档位「{name}」（{id}，model={model}）已移入生图栏");
    }

    if moved > 0 {
        log::info!("生图档位迁移完成：搬了 {moved} 条到生图栏");
    }

    inherit_the_old_current_image_tier(conn)?;
    Ok(())
}

/// 给 providers 表加 `user_edited` 列（「已手工维护」的存库标记，默认 0 = 没手动维护过）。
///
/// ⚠️ **列不放进上游 `create_tables_on_conn` 的 providers CREATE** —— 全新库靠这一步
/// （迁移链）补上，与 v0→v1 建 loongport 两张表的模式一致，也不扩大与上游 merge 的接触面。
fn add_user_edited_column(conn: &Connection) -> Result<(), AppError> {
    // providers 表不存在时直接返回，不报错 —— 与 `move_image_tiers_to_their_own_column`
    // 同一个理由：迁移不该因为缺表让 `Database::init` 失败、app 起不来。
    let has_providers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='providers'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_providers == 0 {
        return Ok(());
    }

    // 幂等：列已存在（手工加过 / 重跑）就跳过，别让迁移崩掉。
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('providers') WHERE name='user_edited'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_column > 0 {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE providers ADD COLUMN user_edited INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .map_err(|e| AppError::Database(format!("给 providers 加 user_edited 列失败: {e}")))?;
    Ok(())
}

/// 给 providers 表加 `tier_rate_multiplier` 列（档位倍率的存库值，`NULL` = 未知）。
///
/// ## 为什么倍率要落库
///
/// 它原来根本不存：`list_tiers_impl` 恒返回 `None`，界面靠一条单独的命令
/// （每个档位一次 HTTP）异步补，而那条命令挂在每次动作后的 reload 上 ⇒
/// 切一次档位就把所有档位的倍率重查一遍。倍率是服务端定价，不是实时量。
///
/// 现在 provision 时就算好写下来（那一步本来就在拉分组），只有用户主动刷新才更新。
///
/// ## 为什么不叫 `rate_multiplier`
///
/// 上游 `ProviderMeta` 已有一个语义完全不同的 `costMultiplier`（用户自设的成本倍率，
/// 用来算账单展示）。同名两个概念迟早被读错，而这是个静默错误。
///
/// ⚠️ 与 [`add_user_edited_column`] 同一条纪律：**列不放进上游
/// `create_tables_on_conn` 的 providers CREATE**，全新库靠迁移链补上。
fn add_tier_rate_multiplier_column(conn: &Connection) -> Result<(), AppError> {
    // 缺表不报错，理由同 `add_user_edited_column`。
    let has_providers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='providers'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_providers == 0 {
        return Ok(());
    }

    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('providers') WHERE name='tier_rate_multiplier'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_column > 0 {
        return Ok(());
    }

    // 可空且无默认值：`NULL` 就是「还没查过」。给 0 当默认会让 UI 把它当成
    // 「免费档」显示出来 —— 那比「倍率未知」严重得多。
    conn.execute(
        "ALTER TABLE providers ADD COLUMN tier_rate_multiplier REAL",
        [],
    )
    .map_err(|e| AppError::Database(format!("给 providers 加 tier_rate_multiplier 列失败: {e}")))?;
    Ok(())
}

/// 把 `loongport_operator` 改名成 `loongport_relay`（连同它的唯一索引）。
///
/// 纯改名，不动任何一行数据 —— 「运营商」这个词整仓统一成了「中转站 / relay」，
/// 表名是最后一处旧词。
///
/// ## ⚠️ 必须先删掉那张刚建出来的空表
///
/// `create_tables_on_conn` 跑在迁移**之前**，它已经按新形态
/// `CREATE TABLE IF NOT EXISTS loongport_relay` 建了一张**空表**。此时老库里
/// `loongport_operator` 还在 ⇒ 直接 `RENAME TO loongport_relay` 会撞
/// 「table already exists」，迁移失败、app 起不来。
///
/// 所以顺序是：老表在 ⇒ 先删掉那张空的新表，再把老表改过去。
/// **删之前断言它确实是空的** —— 真有数据说明遇到了没预料到的库形态
/// （两张表同时有行），那时宁可报错也不能悄悄删掉用户的登录态。
///
/// ## 索引要显式重建
///
/// SQLite 的 `ALTER TABLE ... RENAME TO` 会把索引带过去，但**不改索引的名字**
/// ⇒ 改完仍叫 `idx_loongport_operator_site_account`。留着它不会出错（唯一约束照旧
/// 生效），但下一个人 `.schema` 一看就会以为还有张 operator 表。删掉重建成新名字。
///
/// ## 幂等
///
/// 老表不存在（全新库、或迁移重跑）时整个函数空转。
fn rename_operator_table_to_relay(conn: &Connection) -> Result<(), AppError> {
    let has_old: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='loongport_operator'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_old == 0 {
        return Ok(());
    }

    let has_new: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='loongport_relay'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_new > 0 {
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM loongport_relay", [], |r| r.get(0))
            .unwrap_or(0);
        if rows > 0 {
            return Err(AppError::Database(
                "loongport_operator 与 loongport_relay 同时有数据，无法自动改名 —— \
                 请备份数据库后联系维护者"
                    .to_string(),
            ));
        }
        conn.execute("DROP TABLE loongport_relay", [])
            .map_err(|e| AppError::Database(format!("删除空的 loongport_relay 表失败: {e}")))?;
    }

    conn.execute(
        "ALTER TABLE loongport_operator RENAME TO loongport_relay",
        [],
    )
    .map_err(|e| AppError::Database(format!("loongport_operator 改名失败: {e}")))?;

    conn.execute(
        "DROP INDEX IF EXISTS idx_loongport_operator_site_account",
        [],
    )
    .map_err(|e| AppError::Database(format!("删除旧的中转站唯一索引失败: {e}")))?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_loongport_relay_site_account
         ON loongport_relay(site_origin, account_id)",
        [],
    )
    .map_err(|e| AppError::Database(format!("重建 loongport_relay 索引失败: {e}")))?;

    Ok(())
}

/// 把上一版那个 settings 键里记的「当前生图档位」变成新栏的 `is_current`。
///
/// ## 为什么必须做（实测抓出来的缺口）
///
/// 上一版用 `settings` 表的 `loongport_current_image_tier` 记「用哪个档位生图」。
/// 分栏之后那个概念由 `providers.is_current` 表达，而**换栏的 UPDATE 不会顺带设它**
/// ⇒ 升级后 `is_current` 全是 0 ⇒ 用户明明选过 1K 档，生图却报「还没有选定用哪个
/// 档位生图」。他得再点一次 —— 那是个没必要的回退。
///
/// 实测维护者的库：`loongport_current_image_tier = loongport-9ac36958c41ffe96`，
/// 而只搬栏之后那两条生图档位的 `is_current` 都是 0。
///
/// ## 只写库那一层，不写 settings.json
///
/// 设备级那层（`~/.loongport/settings.json` 的 `currentProviderCodexImage`）由主程序
/// 的 `switch` 维护，迁移跑在 `Database::init` 里、碰不到那个文件（也不该碰：迁移的
/// 作用域是数据库）。而 `get_effective_current_provider` 在设备级缺失时**正是回落到
/// 库里的 `is_current`** —— 所以只写这一层就够了，用户下次切换时那一层会自动补上。
///
/// ## 清掉旧键
///
/// 做减法（CLAUDE.md：清包袱不在旧的旁边加一层）：值已经搬到新位置，留着它只会让
/// 将来读代码的人怀疑「是不是还有第二个来源」。
fn inherit_the_old_current_image_tier(conn: &Connection) -> Result<(), AppError> {
    const OLD_KEY: &str = "loongport_current_image_tier";

    // `settings` 表可能不存在 —— 与上面那道 `providers` 守卫同一个理由：
    // 报错会让 `Database::init` 失败、app 起不来，而这一步没什么非做不可的。
    let has_settings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='settings'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_settings == 0 {
        return Ok(());
    }

    let old: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [OLD_KEY],
            |r| r.get(0),
        )
        .ok()
        .filter(|v: &String| !v.is_empty());

    let Some(provider_id) = old else {
        return Ok(());
    };

    // 只在那条档位真的落到了新栏时才设 —— 它可能已经被删掉了（旧键不会跟着清）。
    let affected = conn
        .execute(
            "UPDATE providers SET is_current = 1 WHERE id = ?1 AND app_type = 'codex-image'",
            [&provider_id],
        )
        .map_err(|e| AppError::Database(format!("继承当前生图档位失败: {e}")))?;

    if affected > 0 {
        log::info!("当前生图档位沿用旧设置：{provider_id}");
    } else {
        log::info!("旧设置里的生图档位 {provider_id} 已不存在，跳过继承");
    }

    // 旧键一律清掉（哪怕上面没匹配上）—— 它已经没有任何读者了。
    let _ = conn.execute("DELETE FROM settings WHERE key = ?1", [OLD_KEY]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    fn mem() -> Connection {
        Connection::open_in_memory().expect("内存库")
    }

    fn migrated_database_at_version(version: i32) -> Connection {
        assert_eq!(version, 12, "这个夹具只描述 v12 的 relay 表形态");
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE loongport_relay (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                account_id INTEGER,
                account_label TEXT NOT NULL DEFAULT '',
                login_identifier TEXT NOT NULL DEFAULT '',
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                sort_index INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                backend_kind TEXT NOT NULL DEFAULT 'sub2api',
                user_agent TEXT,
                cf_clearance TEXT
            );",
        )
        .expect("造 v12 relay 表");
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, version).expect("设置版本");
        conn
    }

    /// 造一张 providers 表 + 一条 codex 档位，用于迁移测试。
    fn providers_table(conn: &Connection) {
        crate::Database::create_tables_on_conn(conn).expect("建表");
    }

    fn legacy_providers_table(conn: &Connection) {
        conn.execute(
            "CREATE TABLE providers (
                id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                PRIMARY KEY (id, app_type)
            )",
            [],
        )
        .expect("造 v4 providers 表");
    }

    fn v8_relay_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE loongport_relay (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                account_id INTEGER,
                account_label TEXT NOT NULL DEFAULT '',
                login_identifier TEXT NOT NULL DEFAULT '',
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                sort_index INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE UNIQUE INDEX idx_loongport_relay_site_account
            ON loongport_relay(site_origin, account_id);",
        )
        .expect("造 v8 relay 表");
    }

    fn relay_table_shape(conn: &Connection) -> Vec<(String, String, i64, Option<String>, i64)> {
        conn.prepare("PRAGMA table_info('loongport_relay')")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    #[test]
    fn v12_to_v13_adds_nullable_pricing_synced_at() {
        let conn = migrated_database_at_version(12);
        apply(&conn).unwrap();
        assert!(Database::has_column(&conn, "loongport_relay", "pricing_synced_at").unwrap());
        let not_null: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('loongport_relay') \
                 WHERE name='pricing_synced_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(not_null, 0);
    }

    /// ⭐ v13→v14 建中转站余额快照表：停在 v13 的老库升级后必须有表且可写。
    #[test]
    fn v13_to_v14_creates_the_balance_snapshot_table() {
        let conn = mem();
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 13).expect("设为 v13");
        assert!(
            !Database::table_exists(&conn, "relay_balance_snapshots").expect("查表"),
            "前提：升级前本表不存在"
        );

        apply(&conn).expect("迁移到 v14");

        assert!(
            Database::table_exists(&conn, "relay_balance_snapshots").expect("查表"),
            "v13 → v14 必须建出余额快照表"
        );
        conn.execute(
            "INSERT INTO relay_balance_snapshots (relay_id, balance_usd, source, created_at)
             VALUES (1, 1.5, 'balance_query', 0)",
            [],
        )
        .expect("迁移后必须可写");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// ⭐ v14→v15 清洗余额快照重复行：与同站上一条余额相同的行删掉、每个等值段留最早
    /// 一条；余额回到旧值的行（与最新一条不同）必须留下。
    #[test]
    fn v14_to_v15_deletes_consecutive_duplicate_balance_snapshots() {
        let conn = mem();
        crate::relay::reconcile::create_table(&conn).expect("建快照表");
        // relay 7：10 → 10（删）→ 8 → 10；relay 8：5 → 5（删）。跨站互不影响。
        for (relay_id, created_at, balance) in [
            (7, 100, 10.0),
            (7, 200, 10.0),
            (7, 300, 8.0),
            (7, 400, 10.0),
            (8, 150, 5.0),
            (8, 250, 5.0),
        ] {
            conn.execute(
                "INSERT INTO relay_balance_snapshots (relay_id, balance_usd, source, created_at)
                 VALUES (?1, ?2, 'balance_query', ?3)",
                rusqlite::params![relay_id, balance, created_at],
            )
            .expect("造 v14 存量数据");
        }
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 14).expect("设为 v14");

        apply(&conn).expect("迁移到 v15");

        let rows: Vec<(i64, f64)> = conn
            .prepare(
                "SELECT relay_id, balance_usd FROM relay_balance_snapshots
                      ORDER BY relay_id, created_at",
            )
            .expect("查表")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("读行")
            .collect::<rusqlite::Result<_>>()
            .expect("收集");
        assert_eq!(
            rows,
            vec![(7, 10.0), (7, 8.0), (7, 10.0), (8, 5.0)],
            "等值段留最早一条；回到旧值（≠最新一条）的行要留"
        );
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    #[test]
    fn v8_to_v9_adds_backend_kind_and_defaults_existing_rows_to_sub2api() {
        let conn = mem();
        v8_relay_table(&conn);
        conn.execute(
            "INSERT INTO loongport_relay (
                site_origin, site_name, api_base_url, account_id, account_label, auth_token
             ) VALUES (
                'https://legacy.example', 'Legacy', 'https://legacy.example', 7, '旧账号', 'tok'
             )",
            [],
        )
        .expect("插入 v8 relay 行");
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 8).expect("设为 v8");

        apply(&conn).expect("迁移到 v9");

        let row: (String, String, String) = conn
            .query_row(
                "SELECT site_origin, auth_token, backend_kind
                 FROM loongport_relay WHERE account_id = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("读取迁移后的 relay 行");
        assert_eq!(
            row,
            (
                "https://legacy.example".to_string(),
                "tok".to_string(),
                "sub2api".to_string(),
            ),
            "迁移必须保留旧行，并把历史 relay 明确归为 sub2api"
        );
        // 断言的是「迁到最新版」而不是某个具体数字：写死版本号会让每加一次迁移
        // 都得回来改这行（而它想钉的事实从来没变过）。
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// v10 → v11 加上 cf_clearance 列，且重复跑不炸（幂等）。
    #[test]
    fn v10_to_v11_adds_cf_clearance_column_idempotently() {
        let conn = mem();
        apply(&conn).expect("initial migrate");
        assert!(crate::Database::has_column(&conn, "loongport_relay", "cf_clearance").unwrap());

        // 迁移必须可重入：启动时会无条件跑一遍。
        apply(&conn).expect("second migrate must be a no-op");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    #[test]
    fn v9_migration_does_not_treat_schema_inspection_failure_as_missing_table() {
        let conn = mem();
        v8_relay_table(&conn);
        let _ = conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "sqlite_master",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));

        let error = add_relay_backend_kind_column(&conn)
            .expect_err("schema inspection failure must abort the migration")
            .to_string();

        assert!(
            error.contains("读取表名失败") || error.contains("查询表名失败"),
            "unexpected migration error: {error}"
        );
    }

    #[test]
    fn fresh_and_migrated_relay_tables_have_the_same_v9_shape() {
        let migrated = mem();
        v8_relay_table(&migrated);
        ensure_version_table(&migrated).expect("建版本表");
        set_version(&migrated, 8).expect("设为 v8");
        apply(&migrated).expect("迁移到 v9");

        let fresh = mem();
        crate::relay::creds::create_table(&fresh).expect("按最新形态建表");

        let migrated_shape = relay_table_shape(&migrated);
        let fresh_shape = relay_table_shape(&fresh);
        assert_eq!(
            migrated_shape, fresh_shape,
            "全新数据库与 v8 迁移后的 relay 表必须是同一种形态"
        );
        assert!(
            fresh_shape.iter().any(|column| {
                column.0 == "backend_kind"
                    && column.1 == "TEXT"
                    && column.2 == 1
                    && column.3.as_deref() == Some("'sub2api'")
            }),
            "v9 relay 表必须包含 NOT NULL、默认 sub2api 的 backend_kind"
        );
    }

    #[test]
    fn v4_to_latest_creates_model_verification_tables_without_touching_user_version() {
        let conn = mem();
        legacy_providers_table(&conn);
        conn.execute("PRAGMA user_version = 16", [])
            .expect("设置上游版本");
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 4).expect("设为 v4");

        apply(&conn).expect("迁移到最新版本");

        let result_table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'model_verification_results'
                )",
                [],
                |row| row.get(0),
            )
            .expect("查询模型验证表");
        let settings_table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'model_verification_settings'
                )",
                [],
                |row| row.get(0),
            )
            .expect("查询运行时验证设置表");
        let leases_table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'model_verification_proxy_leases'
                )",
                [],
                |row| row.get(0),
            )
            .expect("查询运行时验证租约表");
        let history_table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'model_verification_history'
                )",
                [],
                |row| row.get(0),
            )
            .expect("查询模型验证历史表");
        let user_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("读取上游版本");

        assert!(result_table_exists, "v4 → v5 必须创建模型验证结果表");
        assert!(settings_table_exists, "v5 → v6 必须创建运行时验证设置表");
        assert!(leases_table_exists, "v5 → v6 必须创建运行时验证租约表");
        assert!(history_table_exists, "v6 → v7 必须创建模型验证历史表");
        assert_eq!(user_version, 16, "LoongPort 迁移不许修改上游版本号");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    #[test]
    fn v5_to_latest_is_idempotent_and_seeds_setting() {
        let conn = mem();
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 5).expect("设为 v5");

        apply(&conn).expect("第一次迁移");
        apply(&conn).expect("第二次迁移不应报错");

        let setting: (i64, i64) = conn
            .query_row(
                "SELECT runtime_auto_enabled, singleton
                 FROM model_verification_settings",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取默认设置");
        assert_eq!(setting, (0, 1));
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    #[test]
    fn v7_to_v8_removes_runtime_state_but_preserves_unresolved_proxy_leases() {
        let conn = mem();
        legacy_providers_table(&conn);
        conn.execute(
            "INSERT INTO providers (id, app_type) VALUES ('provider-a', 'codex')",
            [],
        )
        .expect("插入旧档位");
        conn.execute_batch(
            "CREATE TABLE model_verification_history (
                id INTEGER PRIMARY KEY,
                provider_id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL CHECK (source IN ('active', 'runtime')),
                verdict TEXT NOT NULL,
                evidence_level TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                rules_version INTEGER NOT NULL,
                checked_at INTEGER NOT NULL,
                FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
            );
            CREATE INDEX idx_model_verification_history_scope_order
            ON model_verification_history (provider_id, app_type, checked_at DESC, id DESC);
            INSERT INTO model_verification_history (
                provider_id, app_type, model, source, verdict, evidence_level,
                facts_json, rules_version, checked_at
            ) VALUES
                ('provider-a', 'codex', 'gpt-test', 'active', 'match', 'strong', '[]', 1, 100),
                ('provider-a', 'codex', 'gpt-test', 'runtime', 'mismatch', 'strong', '[]', 1, 200);",
        )
        .expect("创建并填充 v7 验证历史");
        crate::relay::model_verification::store::create_legacy_runtime_tables(&conn)
            .expect("创建旧运行时表");
        conn.execute(
            "UPDATE model_verification_settings SET runtime_auto_enabled = 1 WHERE singleton = 1",
            [],
        )
        .expect("开启旧自动验证设置");
        conn.execute(
            "INSERT INTO model_verification_proxy_leases (app_type, acquired_at)
             VALUES ('codex', 123)",
            [],
        )
        .expect("插入未恢复的旧代理租约");
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 7).expect("设为 v7");

        apply(&conn).expect("迁移到 v8");

        let active_history: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_verification_history WHERE source = 'active'",
                [],
                |row| row.get(0),
            )
            .expect("统计主动验证历史");
        let runtime_history: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_verification_history WHERE source = 'runtime'",
                [],
                |row| row.get(0),
            )
            .expect("统计运行时验证历史");
        let runtime_auto_enabled: i64 = conn
            .query_row(
                "SELECT runtime_auto_enabled FROM model_verification_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("读取旧自动验证设置");
        let unresolved_lease: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_verification_proxy_leases WHERE app_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .expect("统计未恢复的代理租约");

        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
        assert_eq!(active_history, 1, "主动验证历史必须保留");
        assert_eq!(runtime_history, 0, "被动运行时验证历史必须删除");
        assert_eq!(runtime_auto_enabled, 0, "旧自动验证设置必须关闭");
        assert_eq!(unresolved_lease, 1, "未恢复的代理租约必须留给启动清理");
    }

    #[test]
    fn v6_to_v7_keeps_current_results_but_starts_history_empty() {
        let conn = mem();
        legacy_providers_table(&conn);
        conn.execute(
            "INSERT INTO providers (id, app_type) VALUES ('provider-a', 'codex')",
            [],
        )
        .expect("插入旧档位");
        crate::relay::model_verification::store::create_results_table(&conn).expect("创建旧结果表");
        conn.execute(
            "INSERT INTO model_verification_results (
                provider_id, app_type, model, verdict, evidence_level,
                rules_version, updated_at
             ) VALUES (
                'provider-a', 'codex', 'gpt-test', 'inconclusive',
                'insufficient', 1, 123
             )",
            [],
        )
        .expect("插入旧结果快照");
        ensure_version_table(&conn).expect("建版本表");
        set_version(&conn, 6).expect("设为 v6");

        apply(&conn).expect("迁移到 v7");

        let current_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_verification_results",
                [],
                |row| row.get(0),
            )
            .expect("统计当前结果");
        let history_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_verification_history",
                [],
                |row| row.get(0),
            )
            .expect("统计验证历史");

        assert_eq!(current_count, 1, "当前结果仍是运行状态，不应丢弃");
        assert_eq!(history_count, 0, "旧快照不能伪造为历史事件");
    }

    fn insert_codex_tier(conn: &Connection, id: &str, name: &str, model: &str) {
        let settings = crate::relay::provision::settings_config_for(
            &crate::app_config::AppType::Codex,
            "sk-test",
            name,
            "https://api.x.dev/v1",
            model,
        )
        .expect("codex 必须有形状");
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, website_url, category)
             VALUES (?1, 'codex', ?2, ?3, 'https://x.dev', 'aggregator')",
            rusqlite::params![id, name, serde_json::to_string(&settings).unwrap()],
        )
        .expect("插档位");
    }

    fn app_type_of(conn: &Connection, id: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT app_type FROM providers WHERE id = ?1 ORDER BY app_type")
            .unwrap();
        let rows = stmt
            .query_map([id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows
    }

    /// ⭐ **纯生图档位被搬到生图栏，聊天档位留在原处。**
    ///
    /// 这是迁移的全部意义：不搬的话老档位滞留在 codex 栏，用户在两处都看到生图档位
    /// （旧的那条在 codex 栏、生图栏空着）—— 比分栏之前更让人困惑，而他没有理由知道
    /// 要去点一次「获取密钥」。
    #[test]
    fn image_tiers_move_and_chat_tiers_stay() {
        let conn = mem();
        providers_table(&conn);
        // 一条纯生图 + 一条聊天，都是托管 id（16 位小写 hex）。
        insert_codex_tier(&conn, "loongport-aaaaaaaaaaaaaaaa", "生图档", "gpt-image-2");
        insert_codex_tier(
            &conn,
            "loongport-bbbbbbbbbbbbbbbb",
            "聊天档",
            crate::relay::provision::DEFAULT_MODEL,
        );

        apply(&conn).expect("迁移");

        assert_eq!(
            app_type_of(&conn, "loongport-aaaaaaaaaaaaaaaa"),
            vec!["codex-image"],
            "生图档位没被搬进生图栏"
        );
        assert_eq!(
            app_type_of(&conn, "loongport-bbbbbbbbbbbbbbbb"),
            vec!["codex"],
            "聊天档位被误搬了 —— 用户会少一个能对话的档位"
        );
    }

    /// **非托管的 provider 不动** —— 用户自己配的 gpt-image 档位归他管。
    ///
    /// 生图栏只接受托管档位（`is_managed` 是生图工具按 provider_id 去库里读 sk 的前提），
    /// 把一条手工 provider 搬进去只会让它在那一页里点不动。
    #[test]
    fn hand_made_providers_are_left_alone() {
        let conn = mem();
        providers_table(&conn);
        insert_codex_tier(&conn, "my-own-image-provider", "我自己配的", "gpt-image-2");

        apply(&conn).expect("迁移");

        assert_eq!(
            app_type_of(&conn, "my-own-image-provider"),
            vec!["codex"],
            "手工 provider 被搬进了生图栏"
        );
    }

    /// **可重复执行**：第二次跑不该报错、也不该改变结果。
    ///
    /// 迁移链本身有版本号挡着，但备份导入 / 回滚再升级都会让它重跑一次，
    /// 而 `apply_is_idempotent` 只验了空库。
    #[test]
    fn the_image_tier_migration_is_idempotent() {
        let conn = mem();
        providers_table(&conn);
        insert_codex_tier(&conn, "loongport-cccccccccccccccc", "生图档", "gpt-image-2");

        move_image_tiers_to_their_own_column(&conn).expect("第一次");
        move_image_tiers_to_their_own_column(&conn).expect("第二次不该报错");

        assert_eq!(
            app_type_of(&conn, "loongport-cccccccccccccccc"),
            vec!["codex-image"],
        );
    }

    /// **两栏都已有同 id 的行时，删旧栏那条而不是撞主键。**
    ///
    /// 场景：用户在新版跑过 provision（生图栏已有这条）、回滚到旧版、再升上来
    /// （旧版又往 codex 栏写了一条）。裸 `UPDATE` 会撞 `(id, app_type)` 主键
    /// ⇒ 迁移报错 ⇒ `Database::init` 返回 Err ⇒ **app 起不来**。
    #[test]
    fn a_duplicate_in_the_new_column_does_not_break_the_migration() {
        let conn = mem();
        providers_table(&conn);
        let id = "loongport-dddddddddddddddd";
        insert_codex_tier(&conn, id, "生图档（旧栏）", "gpt-image-2");
        // 生图栏里已经有一条更新的。
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config)
             VALUES (?1, 'codex-image', '生图档（新栏）', '{}')",
            [id],
        )
        .expect("插新栏那条");

        move_image_tiers_to_their_own_column(&conn).expect("不该撞主键");

        assert_eq!(
            app_type_of(&conn, id),
            vec!["codex-image"],
            "旧栏那条该被删掉，只留新栏的"
        );
        // 留下的必须是新栏原本那条（更新的），不是被 UPDATE 搬过去的旧的。
        let name: String = conn
            .query_row(
                "SELECT name FROM providers WHERE id = ?1 AND app_type = 'codex-image'",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "生图档（新栏）", "新栏那条被旧的覆盖了");
    }

    /// **坏记录不该让迁移中止** —— 那会让 `Database::init` 失败、app 起不来。
    #[test]
    fn a_corrupt_settings_config_does_not_abort_the_migration() {
        let conn = mem();
        providers_table(&conn);
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config)
             VALUES ('loongport-eeeeeeeeeeeeeeee', 'codex', '坏记录', 'not json at all')",
            [],
        )
        .unwrap();
        insert_codex_tier(&conn, "loongport-ffffffffffffffff", "生图档", "gpt-image-2");

        apply(&conn).expect("坏记录不该让整次迁移失败");

        // 坏的那条原样留着，好的那条照样搬走了。
        assert_eq!(
            app_type_of(&conn, "loongport-eeeeeeeeeeeeeeee"),
            vec!["codex"]
        );
        assert_eq!(
            app_type_of(&conn, "loongport-ffffffffffffffff"),
            vec!["codex-image"],
        );
    }

    /// ⭐ **用户上一版选好的生图档位要沿用，不该让他再点一次。**
    ///
    /// 实测抓出来的缺口：换栏的 UPDATE 不碰 `is_current`，所以只搬栏的话升级后
    /// 那一栏一个当前项都没有 ⇒ 生图报「还没有选定用哪个档位生图」，而用户明明选过。
    #[test]
    fn the_previously_selected_image_tier_is_inherited() {
        let conn = mem();
        providers_table(&conn);
        let chosen = "loongport-1111111111111111";
        let other = "loongport-2222222222222222";
        insert_codex_tier(&conn, chosen, "1K生图", "gpt-image-2");
        insert_codex_tier(&conn, other, "4K生图", "gpt-image-2");
        // 上一版那个 settings 键。
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('loongport_current_image_tier', ?1)",
            [chosen],
        )
        .expect("插旧键");

        apply(&conn).expect("迁移");

        let current: Option<String> = conn
            .query_row(
                "SELECT id FROM providers WHERE app_type = 'codex-image' AND is_current = 1",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(
            current.as_deref(),
            Some(chosen),
            "用户上一版选的生图档位没被沿用 —— 他得再点一次"
        );
        // 旧键清掉了（做减法：值已经搬到新位置）。
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'loongport_current_image_tier'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0, "旧键该被清掉，留着会让人怀疑有第二个来源");
    }

    /// 旧键指向一个**已经不存在**的档位时，不该设错任何一条的 `is_current`。
    ///
    /// 那个档位可能被删了（中转站下架分组 / 用户删了账号），而旧键不会跟着清。
    #[test]
    fn a_dangling_old_key_selects_nothing() {
        let conn = mem();
        providers_table(&conn);
        insert_codex_tier(&conn, "loongport-3333333333333333", "生图档", "gpt-image-2");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('loongport_current_image_tier', 'loongport-deaddeaddeaddead')",
            [],
        )
        .unwrap();

        apply(&conn).expect("迁移");

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE app_type = 'codex-image' AND is_current = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "旧键指向的档位已不存在，不该随便挑一条设成当前项");
    }

    /// ⭐ **生图栏里只能有一个当前项。**
    ///
    /// ## 这条闸守的是 review 探针抓出的一个不可复现的 bug
    ///
    /// 生图档位在 codex 栏本来可能就是 `is_current`（用户点错「启用」的结果 ——
    /// 那正是本轮要治的场景）。换栏时若原样带过去，生图栏就有**两个** `is_current = 1`：
    /// 一个是 codex 栏的旧状态，一个是 `inherit_the_old_current_image_tier` 按旧
    /// settings 键设的。
    ///
    /// 而生图 MCP 那侧是 `SELECT id … WHERE is_current = 1` 取第一行 ⇒ 拿到哪个取决于
    /// SQLite 的返回顺序 ⇒ **用户选的是 4K 档，出的可能是 1K 的图**，而且换台机器
    /// 结果可能不同。实测：旧键指向 B，而未修时实际拿到 A。
    #[test]
    fn the_image_column_ends_up_with_exactly_one_current() {
        let conn = mem();
        providers_table(&conn);
        let a = "loongport-aaaa1111aaaa1111";
        let b = "loongport-bbbb2222bbbb2222";
        insert_codex_tier(&conn, a, "A生图", "gpt-image-2");
        insert_codex_tier(&conn, b, "B生图", "gpt-image-2");
        // A 在 codex 栏是当前项（用户点错「启用」留下的状态）。
        conn.execute("UPDATE providers SET is_current = 1 WHERE id = ?1", [a])
            .unwrap();
        // 而上一版那个 settings 键指向 B —— B 才是用户真正选来生图的那个。
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('loongport_current_image_tier', ?1)",
            [b],
        )
        .unwrap();

        apply(&conn).expect("迁移");

        let mut stmt = conn
            .prepare(
                "SELECT id FROM providers WHERE app_type = 'codex-image' AND is_current = 1 \
                 ORDER BY id",
            )
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![b.to_string()],
            "生图栏该只有一个当前项、且必须是用户真正选的那个（旧 settings 键指向的）"
        );
    }

    #[test]
    fn a_fresh_database_reports_version_zero() {
        let conn = mem();
        assert_eq!(current_version(&conn).unwrap(), 0);
    }

    #[test]
    fn apply_brings_a_fresh_database_to_the_latest_version() {
        let conn = mem();
        apply(&conn).expect("迁移");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
        // 两张表都得建出来。
        for table in ["loongport_relay", "loongport_vendor"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table} 该被建出来");
        }
    }

    /// ⭐ v2→v3 给 providers 加 `user_edited` 列（「已手工维护」存库）。
    ///
    /// ⚠️ 前提钉住：`create_tables_on_conn`（上游建表）**不带**这列 —— 它由 LoongPort
    /// 自己的迁移加，不扩大与上游 merge 的接触面。哪天有人把列加进上游 CREATE，
    /// 这条的前提断言会先红，提醒「别那样做」。
    #[test]
    fn v2_to_v3_adds_user_edited_column_to_providers() {
        let conn = mem();
        crate::Database::create_tables_on_conn(&conn).unwrap();
        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('providers') \
                 WHERE name='user_edited'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            has, 0,
            "前提：create_tables 不该带 user_edited —— 那是 LoongPort 迁移的活"
        );

        // 模拟一个停在 v2 的老库。
        ensure_version_table(&conn).unwrap();
        set_version(&conn, 2).unwrap();

        apply(&conn).unwrap();

        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('providers') \
                 WHERE name='user_edited'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has, 1, "迁移后 providers 必须有 user_edited 列");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// ⭐ v11→v12 给 providers 加 `tier_rate_multiplier` 列（档位倍率落库）。
    ///
    /// 与上面那条同形。前提同样钉住：上游建表**不带**这列。
    ///
    /// 列必须**可空**：`NULL` 表示「还没查过倍率」。给它一个 `NOT NULL DEFAULT 0`
    /// 会让所有历史档位一夜之间显示成「0 倍」——用户会当成最便宜的一档去用。
    #[test]
    fn v11_to_v12_adds_tier_rate_multiplier_column_to_providers() {
        let conn = mem();
        crate::Database::create_tables_on_conn(&conn).unwrap();
        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('providers') \
                 WHERE name='tier_rate_multiplier'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            has, 0,
            "前提：create_tables 不该带 tier_rate_multiplier —— 那是 LoongPort 迁移的活"
        );

        // 模拟一个停在 v11 的老库。
        ensure_version_table(&conn).unwrap();
        set_version(&conn, 11).unwrap();

        apply(&conn).unwrap();

        let notnull: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('providers') \
                 WHERE name='tier_rate_multiplier' AND \"notnull\" = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            notnull, 1,
            "迁移后 providers 必须有一个**可空**的 tier_rate_multiplier 列"
        );
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// ⭐ v3→v4 把 `loongport_operator` 改名成 `loongport_relay`，**一行数据都不能丢**。
    ///
    /// ⚠️ 这条特意先跑 `create_tables_on_conn` 再造老表 —— 复现真实启动顺序：建表在迁移
    /// **之前**，所以迁移开跑时新名字的空表已经存在。少了这一步，`RENAME TO` 撞
    /// 「table already exists」的那个坑测不出来。
    #[test]
    fn v3_to_v4_renames_the_operator_table_and_keeps_its_rows() {
        let conn = mem();
        // 建表先跑（新形态：loongport_relay 空表），与真实启动顺序一致。
        crate::Database::create_tables_on_conn(&conn).unwrap();

        // 再造一张停在 v3 的老库该有的 loongport_operator，塞两行。
        conn.execute(
            "CREATE TABLE loongport_operator (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                account_id INTEGER,
                account_label TEXT NOT NULL DEFAULT '',
                login_identifier TEXT NOT NULL DEFAULT '',
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                sort_index INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE UNIQUE INDEX idx_loongport_operator_site_account
             ON loongport_operator(site_origin, account_id)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO loongport_operator (site_origin, account_label, auth_token)
             VALUES ('https://a.example', '甲', 'tok-a'), ('https://b.example', '乙', 'tok-b')",
            [],
        )
        .unwrap();

        ensure_version_table(&conn).unwrap();
        set_version(&conn, 3).unwrap();

        apply(&conn).unwrap();

        // 老表没了，新表在。
        let old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='loongport_operator'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0, "改完不该还留着 loongport_operator");

        // 两行原样在，凭据没被那张空表冲掉。
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT site_origin, auth_token FROM loongport_relay ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                ("https://a.example".to_string(), "tok-a".to_string()),
                ("https://b.example".to_string(), "tok-b".to_string()),
            ],
            "改名是纯改名 —— 行内容必须原样保留"
        );

        // 索引跟着改了名（RENAME 不改索引名，得显式重建）。
        let idx: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_loongport_%'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            idx.contains(&"idx_loongport_relay_site_account".to_string()),
            "新索引该在：{idx:?}"
        );
        assert!(
            !idx.contains(&"idx_loongport_operator_site_account".to_string()),
            "旧索引名该没了：{idx:?}"
        );

        // 唯一约束还管用（同站同账号插不进第二条）。
        conn.execute(
            "INSERT INTO loongport_relay (site_origin, account_id) VALUES ('https://c.example', 7)",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO loongport_relay (site_origin, account_id) \
                 VALUES ('https://c.example', 7)",
                [],
            )
            .is_err(),
            "去重索引重建后必须仍然生效"
        );

        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    #[test]
    fn apply_is_idempotent() {
        let conn = mem();
        apply(&conn).expect("第一次");
        apply(&conn).expect("第二次不该报错");
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// ⭐ **本模块存在的全部理由：不碰 `PRAGMA user_version`。**
    ///
    /// 那个计数器归上游 cc-switch。我们写它就是抢占上游号段，而撞上之后是
    /// **静默缺表**（版本号显示已最新、表实际没建），不是报错 —— 见模块文档。
    ///
    /// 会红的改法：图省事把版本改回存 `user_version`。
    #[test]
    fn loongport_migrations_never_touch_the_upstream_version_counter() {
        let conn = mem();
        // 先把上游那个计数器设成一个可辨认的值。
        conn.execute("PRAGMA user_version = 16;", []).unwrap();

        apply(&conn).expect("迁移");

        let upstream: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            upstream, 16,
            "LoongPort 的迁移不许动 user_version —— 那是上游的计数器，\
             抢占它会在上游发新迁移时造成静默缺表"
        );
        // 而我们自己的版本确实推进了。
        assert_eq!(current_version(&conn).unwrap(), LOONGPORT_SCHEMA_VERSION);
    }

    /// ⭐ **每一条建库路径都必须跑 [`apply`]** —— 漏一条就是一个版本号停在 0 的库。
    ///
    /// ## 这条闸守的是本模块的**镜像面**
    ///
    /// `loongport_migrations_never_touch_the_upstream_version_counter` 守「号占了表没建」；
    /// 这条守「表建了号没占」。两者是同一件事的两面，都会静默失效。
    ///
    /// ## 漏了的后果（今天无症状，加第二步迁移那天爆）
    ///
    /// v1 是纯 `CREATE TABLE IF NOT EXISTS`，重跑幂等，所以今天漏了看不出来。但一旦
    /// v1→v2 是 `ALTER TABLE ... ADD COLUMN`：导入过备份的库里表**已是 v2 形态**
    /// （`create_tables_on_conn` 建的就是最终形态）而版本记着 0 ⇒ 启动时 apply 从 0
    /// 跑到 2 ⇒ 撞 `duplicate column name` ⇒ `Database::init` 返回 Err，**app 起不来**。
    ///
    /// 更坏的变体：若某步迁移做的是数据回填（`UPDATE ... SET`）而非 DDL，在已经正确的
    /// 表上重跑一次会**静默改数据**。这是它值得现在修而不是等到 v2 的理由。
    ///
    /// ## 为什么扫源码而不是逐条调用
    ///
    /// 逐条调用只能验「我知道的那几条」，挡不住**上游将来新增第四条建库路径** ——
    /// 而那正是同一个 bug 的下一次发作。所以这里的判据是：
    /// **凡是调 `apply_schema_migrations_on_conn`（上游迁移的入口）的地方，
    /// 附近必须也调 `loongport_schema::apply`**。
    ///
    /// 会红的改法：删掉 `backup.rs` 或 `Database::memory()` 里那一行；
    /// 或上游新增一条建库路径而我们没跟上。
    #[test]
    fn every_database_entry_point_also_runs_the_loongport_migrations() {
        // 三份源码里找「上游迁移入口」的**调用**点（定义处不算）。
        let sources = [
            ("database/mod.rs", include_str!("mod.rs")),
            ("database/backup.rs", include_str!("backup.rs")),
        ];

        let mut checked = 0;
        for (name, src) in sources {
            for (lineno, line) in src.lines().enumerate() {
                let is_call = (line.contains("apply_schema_migrations_on_conn(")
                    || line.contains("db.apply_schema_migrations()")
                    || line.contains("db.create_tables()"))
                    // 定义、文档、注释都不是调用点
                    && !line.contains("fn ")
                    && !line.trim_start().starts_with("//");
                if !is_call {
                    continue;
                }
                checked += 1;

                // 同一个函数体内（往后 25 行）必须出现 LoongPort 的迁移调用。
                let window: String = src
                    .lines()
                    .skip(lineno)
                    .take(25)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    window.contains("loongport_schema::apply(")
                        || window.contains("loongport_schema::apply ("),
                    "{name}:{} 附近有建库/迁移调用却没跟着跑 loongport_schema::apply —— \
                     那条路径建出来的库版本号会停在 0。原文：{}",
                    lineno + 1,
                    line.trim()
                );
            }
        }

        // 别让这条闸因为"一个调用点都没匹配到"而空转变绿（那是假闸）。
        assert!(
            checked >= 3,
            "只找到 {checked} 个建库/迁移调用点，少于已知的 3 条（init / memory / 导入）——\
             要么匹配规则失效了，要么真的少了一条路径，两种都得看"
        );
    }

    /// 版本比代码新时要**明确报错**，不能静默继续。
    ///
    /// 用户装了新版又降级回旧版就是这种情况。静默继续意味着拿旧代码去读新形态的表，
    /// 后果不可预测；报错让他知道该升级应用。
    #[test]
    fn a_future_version_is_a_visible_error() {
        let conn = mem();
        ensure_version_table(&conn).unwrap();
        set_version(&conn, LOONGPORT_SCHEMA_VERSION + 5).unwrap();

        let err = apply(&conn).expect_err("版本过新必须报错");
        assert!(
            err.to_string().contains("过新"),
            "错误里要说清是「版本过新」，实际：{err}"
        );
    }

    // ============================================================
    // 启动预检（stored_version_exceeds_supported）
    // ============================================================

    fn temp_db_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().expect("临时库文件")
    }

    /// ⭐ 预检必须抓到「磁盘上的版本比代码新」—— 这是它存在的全部理由。
    ///
    /// 场景就是它修的缺口：用户跑过新版（stamp 到更高版本）又打开旧版，
    /// 检查原本只在 `Database::init` 内部才有，用户看到的是原生 Retry/Exit
    /// 对话框而不是应用内升级恢复界面。
    #[test]
    fn the_precheck_flags_a_database_from_a_newer_build() {
        let file = temp_db_file();
        {
            let conn = Connection::open(file.path()).expect("打开临时库");
            ensure_version_table(&conn).expect("建版本表");
            set_version(&conn, LOONGPORT_SCHEMA_VERSION + 1).expect("写入未来版本");
        }

        assert_eq!(
            stored_version_exceeds_supported(file.path()).unwrap(),
            Some(LOONGPORT_SCHEMA_VERSION + 1)
        );
    }

    #[test]
    fn the_precheck_leaves_current_databases_alone() {
        let file = temp_db_file();
        {
            let conn = Connection::open(file.path()).expect("打开临时库");
            apply(&conn).expect("迁移到最新");
        }

        assert_eq!(stored_version_exceeds_supported(file.path()).unwrap(), None);
    }

    /// ⭐ 预检**只读不写**：没有版本表的库不会被它顺手建出一张来。
    ///
    /// 预检的约束是「先于任何 schema 写入」—— 建表本身也是写入。
    #[test]
    fn the_precheck_never_creates_the_version_table() {
        let file = temp_db_file();
        {
            let conn = Connection::open(file.path()).expect("打开临时库");
            conn.execute("CREATE TABLE unrelated (id INTEGER)", [])
                .expect("造一张无关表");
        }

        assert_eq!(stored_version_exceeds_supported(file.path()).unwrap(), None);

        let conn = Connection::open(file.path()).expect("重开临时库");
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [VERSION_TABLE],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "预检不许建版本表 —— 那是写入");
    }

    #[test]
    fn the_precheck_ignores_a_missing_file() {
        assert_eq!(
            stored_version_exceeds_supported(std::path::Path::new(
                "/nonexistent/loongport-precheck.db"
            ))
            .unwrap(),
            None
        );
    }

    /// ⭐ **预检闸**：lib.rs 里查上游计数器的预检旁边，必须也查 LoongPort 自己的。
    ///
    /// 会红的改法：上游合并把 lib.rs 的预检区域改形、或有人删掉 LoongPort 这条
    /// 预检 —— 两者都会让用户重新看到原生对话框而非升级恢复界面。
    #[test]
    fn the_startup_precheck_covers_the_loongport_counter_too() {
        let src = include_str!("../lib.rs");

        let mut checked = 0;
        for (lineno, line) in src.lines().enumerate() {
            let is_precheck = line.contains("stored_user_version_exceeds_supported(")
                && !line.contains("loongport_schema::")
                && !line.contains("fn ")
                && !line.trim_start().starts_with("//");
            if !is_precheck {
                continue;
            }
            checked += 1;
            let window: String = src
                .lines()
                .skip(lineno)
                .take(40)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                window.contains("loongport_schema::stored_version_exceeds_supported("),
                "lib.rs:{} 的上游版本预检附近没有跟着查 LoongPort 自己的版本表。原文：{}",
                lineno + 1,
                line.trim()
            );
        }
        assert!(checked >= 1, "一个上游预检调用点都没找到 —— 匹配规则失效了");
    }
}
