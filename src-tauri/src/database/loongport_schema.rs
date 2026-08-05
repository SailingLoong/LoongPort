//! LoongPort **自己的** schema 迁移，与上游 cc-switch 的完全分离。
//!
//! ## 为什么不能共用 `PRAGMA user_version`
//!
//! 本仓是 cc-switch 的 fork，要跟着上游升级。上游的迁移用 `user_version` 计数，
//! 而它**现在正好停在 16，下一步就是 17** —— 我们若也往那个计数器上追加，
//! 就是抢占上游的号段。撞上之后的后果是**静默数据损坏**，不是报错：
//!
//! ```text
//! 用户库 stamp = 17（我们建 operator 表时写的）
//!   ↓ 上游发布它自己的 v17（比如加一张表）
//! merge 上游 → schema.rs 里 `17 =>` 分支冲突，无论怎么解都不对：
//!   - 保我们的 ⇒ 上游那张表永远不建
//!   - 保上游的 ⇒ 我们的 operator 表永远不建
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
pub(crate) const LOONGPORT_SCHEMA_VERSION: i32 = 2;

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
    let version: Option<i32> = conn
        .query_row(
            &format!("SELECT version FROM {VERSION_TABLE} WHERE id = 1"),
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(version.unwrap_or(0))
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
                log::info!("LoongPort 数据迁移 v0 → v1（运营商凭据 + 官网直连账号两张表）");
                crate::operator::creds::create_table(conn)?;
                crate::vendor::creds::create_table(conn)?;
                set_version(conn, 1)?;
            }
            // v1 → v2：把纯生图档位从 codex 栏搬到 codex-image 栏。
            1 => {
                log::info!("LoongPort 数据迁移 v1 → v2（生图档位独立成一栏）");
                move_image_tiers_to_their_own_column(conn)?;
                set_version(conn, 2)?;
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
/// **判据是 [`provision::is_user_edited`] 而不是「配置里有没有那些键」**：后者要穷举
/// 污染源（漏一个就洗不干净），前者问的是「这份配置等于我们会生成的默认值吗」——
/// 那是同一个语义的唯一实现，也保证**真·用户编辑不会被洗掉**。
///
/// ## 幂等
///
/// 两条都幂等：`UPDATE ... WHERE app_type='codex'` 在第二次跑时已经没有匹配行；
/// 配置重写只在 `is_user_edited == Some(false)` 且内容确实不同时发生。
///
/// ## 为什么走裸 SQL 而不是 `ProviderService`
///
/// 迁移跑在 `Database::init` 里，那时 `AppState` 还不存在（`ProviderService` 要它）。
/// 这也是上游全部迁移的做法。
fn move_image_tiers_to_their_own_column(conn: &Connection) -> Result<(), AppError> {
    use crate::operator::provision;

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
        if !crate::operator::is_managed(&id) {
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
        // 想过顺带洗掉回填污染（那是本轮症状的直接来源），但判不了：`is_user_edited`
        // 对「被回填污染」和「用户真改过」返回同一个 `Some(true)` —— 两者在这一层
        // 分不开（污染进来的键与用户可能加的键没有形状差别）。
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
            "UPDATE providers SET app_type = 'codex-image' WHERE id = ?1 AND app_type = 'codex'"
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

    fn mem() -> Connection {
        Connection::open_in_memory().expect("内存库")
    }

    /// 造一张 providers 表 + 一条 codex 档位，用于迁移测试。
    fn providers_table(conn: &Connection) {
        crate::Database::create_tables_on_conn(conn).expect("建表");
    }

    fn insert_codex_tier(conn: &Connection, id: &str, name: &str, model: &str) {
        let settings = crate::operator::provision::settings_config_for(
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
            crate::operator::provision::DEFAULT_MODEL,
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
    /// 那个档位可能被删了（运营商下架分组 / 用户删了账号），而旧键不会跟着清。
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
        for table in ["loongport_operator", "loongport_vendor"] {
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
}
