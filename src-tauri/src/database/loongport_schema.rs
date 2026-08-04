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
pub(crate) const LOONGPORT_SCHEMA_VERSION: i32 = 1;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        Connection::open_in_memory().expect("内存库")
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
