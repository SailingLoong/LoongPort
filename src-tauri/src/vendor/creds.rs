//! `loongport_vendor` 表：一行一个「厂商 × 账号」。
//!
//! ## 为什么不复用 `loongport_relay`
//!
//! 那张表有 `site_origin` / `api_base_url` 这些对 vendor 无意义的 NOT NULL 列，
//! 而 vendor 需要它没有的 `vendor_id`；且 `account_id` 类型不同 ——
//! **vendor 是 TEXT**（DeepSeek 给 UUID），relay 是 INTEGER。
//! 合表要么塞一堆可空列 + 一个 kind 判别列，要么互相污染语义，
//! 而**列进了 schema、改它是迁移不是重构**。
//!
//! ## 有意不含的三列（都是 review 撤掉的过度设计）
//!
//! - **`api_key_name`** —— 它恒等于 `key_name_for(account_id, …)`，是纯函数。
//!   存一份就多一个会与命名公式漂移的来源。
//!
//! （曾经还列过一条 `device_id`，那是 2026-08-04 之前的事：那时 Key 名字里带机器
//! 标识。现在**命名是账号粒度的**，机器标识整个概念都没了 —— 见
//! `relay/provision.rs` 的 `key_name_for` 与那边的「Key 爆炸」那段。）
//! - **`refresh_token` / `token_expires_at`** —— DeepSeek 两样都没有。
//!   「schema 不可逆所以现在加」那个论证站不住：加可空列是**局部重构**
//!   （本仓已两次这么干，`relay/creds.rs:181` / `:205`），不是数据迁移。
//!   将来第一家有 refresh 语义的厂商进来时再加，那时才知道真实类型与轮换语义。

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;
use crate::vendor::{Vendor, VendorAccount};

/// 一行「厂商 × 账号」。
#[derive(Debug, Clone)]
pub struct VendorRow {
    pub id: i64,
    pub vendor_id: String,
    /// 厂商侧用户 id。`None` = 还没登录过。**TEXT**（DeepSeek 给 UUID）。
    pub account_id: Option<String>,
    pub account_label: String,
    /// 重登时预填的值（DeepSeek 是手机号）。中立命名，与 relay 同义。
    pub login_identifier: String,
    pub auth_token: String,
    /// **明文 sk**。列表接口拿不回来，所以必须自己存。
    pub api_key: String,
    pub sort_index: i64,
}

const SELECT_COLS: &str = "id, vendor_id, account_id, account_label, login_identifier,
     auth_token, api_key, sort_index";

fn row_to_vendor(row: &rusqlite::Row<'_>) -> rusqlite::Result<VendorRow> {
    Ok(VendorRow {
        id: row.get(0)?,
        vendor_id: row.get(1)?,
        account_id: row.get(2)?,
        account_label: row.get(3)?,
        login_identifier: row.get(4)?,
        auth_token: row.get(5)?,
        api_key: row.get(6)?,
        sort_index: row.get(7)?,
    })
}

/// 建表 + 索引。
///
/// ⚠️ **索引与建表要放在一起判**：`create_tables_on_conn` 在迁移**之前**跑
/// （`Database::init` 先建表再迁移），升级的库上 `CREATE TABLE IF NOT EXISTS`
/// 会跳过。本表是本版本新增的、没有旧形态，所以这里可以直接建索引 ——
/// 但**将来给它加列时要照 `relay/creds.rs:218` 那个 `is_v18_shape` 模式**，
/// 否则索引引用新列会当场报 `no such column` 让 app 起不来（那边踩过）。
///
/// ## ⚠️ 没有 `is_current` 列，别再加回来（2026-08-04 删）
///
/// 曾经有一个（`is_current INTEGER NOT NULL DEFAULT 0`），但**从来没有任何代码写它** ——
/// 只有建表那句 `DEFAULT 0`，所以它恒为 `false`。当前态的唯一事实源是 `providers` 表里
/// 那条记录（上游 `ProviderService::current`）；前端拿 DTO 的 `provider_id` 与它比即可
/// （见 `commands::vendor::VendorAccountRow::provider_id`）。
///
/// **趁这张表还没进任何发布版删掉的**：删它当时只是改一句 `CREATE TABLE`（零迁移），
/// 而发版之后就得写 `ALTER TABLE` + 处理已装机的库 + 上面那套 `is_v18_shape` 形态判断
/// ——从重构变成迁移。留着一个恒假的列比没有更糟：读代码的人会以为它有语义，
/// 而任何「顺手用它判当前态」的代码都会静默地永远走 false 分支。
pub fn create_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS loongport_vendor (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            vendor_id TEXT NOT NULL,
            account_id TEXT,
            account_label TEXT NOT NULL DEFAULT '',
            login_identifier TEXT NOT NULL DEFAULT '',
            auth_token TEXT NOT NULL DEFAULT '',
            api_key TEXT NOT NULL DEFAULT '',
            sort_index INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 loongport_vendor 表失败: {e}")))?;

    // 去重键。SQLite 把 NULL 视为互不相等 ⇒ 多条未登录行不受约束，
    // 由 `save_account` 收口（与 relay 的 `save_site` 同一模式）。
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_loongport_vendor_account
         ON loongport_vendor(vendor_id, account_id)",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 loongport_vendor 索引失败: {e}")))?;

    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<VendorRow>, AppError> {
    let sql = format!("SELECT {SELECT_COLS} FROM loongport_vendor ORDER BY sort_index, id");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(format!("准备查询失败: {e}")))?;
    let rows = stmt
        .query_map([], row_to_vendor)
        .map_err(|e| AppError::Database(format!("查询 vendor 列表失败: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| AppError::Database(format!("读取 vendor 行失败: {e}")))?);
    }
    Ok(out)
}

pub fn get(conn: &Connection, row_id: i64) -> Result<Option<VendorRow>, AppError> {
    let sql = format!("SELECT {SELECT_COLS} FROM loongport_vendor WHERE id = ?1");
    conn.query_row(&sql, params![row_id], row_to_vendor)
        .optional()
        .map_err(|e| AppError::Database(format!("查询 vendor 行失败: {e}")))
}

/// 存一个账号（登录成功后调）。同 `(vendor_id, account_id)` 已存在则**更新**。
pub fn save_account(
    conn: &Connection,
    vendor: Vendor,
    token: &str,
    acct: &VendorAccount,
) -> Result<i64, AppError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM loongport_vendor WHERE vendor_id = ?1 AND account_id = ?2",
            params![vendor.vendor_id(), &acct.account_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("查询已有账号失败: {e}")))?;

    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE loongport_vendor
                 SET auth_token = ?1, account_label = ?2, login_identifier = ?3, updated_at = ?4
                 WHERE id = ?5",
                params![token, &acct.label, &acct.login_identifier, now, id],
            )
            .map_err(|e| AppError::Database(format!("更新账号失败: {e}")))?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO loongport_vendor
                    (vendor_id, account_id, account_label, login_identifier,
                     auth_token, sort_index, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    vendor.vendor_id(),
                    &acct.account_id,
                    &acct.label,
                    &acct.login_identifier,
                    token,
                    now,
                    now
                ],
            )
            .map_err(|e| AppError::Database(format!("保存账号失败: {e}")))?;
            conn.last_insert_rowid()
        }
    };
    Ok(id)
}

pub fn set_api_key(conn: &Connection, row_id: i64, api_key: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE loongport_vendor SET api_key = ?1 WHERE id = ?2",
        params![api_key, row_id],
    )
    .map_err(|e| AppError::Database(format!("保存密钥失败: {e}")))?;
    Ok(())
}

/// 清登录态。
///
/// ⚠️ **不清 `api_key`** —— 它是厂商侧的独立凭据，网页登录态过期**不影响它**，
/// 用户仍能正常使用这条 provider。清掉等于无端废掉一把好 key。
/// ⚠️ **不清 `login_identifier`** —— 那正是重登前的那一步，清掉等于让用户重输手机号。
pub fn clear_token(conn: &Connection, row_id: i64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE loongport_vendor SET auth_token = '' WHERE id = ?1",
        params![row_id],
    )
    .map_err(|e| AppError::Database(format!("清除登录态失败: {e}")))?;
    Ok(())
}

/// 保存行的手工顺序。`ids` 是拖动后的完整顺序，下标即新的 `sort_index`。
///
/// **这是唯一会写 `sort_index` 的地方** —— 只有用户拖动才改顺序（`list` 排的就是它）。
/// 理由与 `relay::creds::reorder` 相同：顺序若跟着 `is_current` 之类会变的东西排，
/// 用户点一下某行就会看到整个列表跳动。
///
/// ⚠️ **只排本表的行**：中转站行在另一张表里，两类行没有共同的序（spec §6.2 已裁决
/// 不可跨类拖动）。
pub fn reorder(conn: &Connection, ids: &[i64]) -> Result<(), AppError> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    for (idx, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE loongport_vendor SET sort_index = ?1 WHERE id = ?2",
            params![idx as i64, id],
        )
        .map_err(|e| AppError::Database(format!("更新排序失败: {e}")))?;
    }
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交排序失败: {e}")))?;
    Ok(())
}

pub fn remove(conn: &Connection, row_id: i64) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM loongport_vendor WHERE id = ?1",
        params![row_id],
    )
    .map_err(|e| AppError::Database(format!("删除账号失败: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        create_table(&conn).expect("建表");
        conn
    }

    fn acct(id: &str) -> VendorAccount {
        VendorAccount {
            account_id: id.to_string(),
            label: format!("账号 {id}"),
            login_identifier: "13800000000".to_string(),
        }
    }

    #[test]
    fn saving_the_same_account_twice_updates_one_row() {
        let conn = setup();
        let first =
            save_account(&conn, Vendor::DeepSeek, "tok-1", &acct("uuid-a")).expect("第一次");
        let second =
            save_account(&conn, Vendor::DeepSeek, "tok-2", &acct("uuid-a")).expect("第二次");
        assert_eq!(first, second, "同一个账号重登要合并成一行，不是新建");
        assert_eq!(list(&conn).expect("列表").len(), 1);
        assert_eq!(
            get(&conn, first).expect("取").expect("有").auth_token,
            "tok-2"
        );
    }

    #[test]
    fn two_accounts_of_the_same_vendor_are_separate_rows() {
        let conn = setup();
        let a = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-a")).expect("a");
        let b = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-b")).expect("b");
        assert_ne!(a, b, "同一厂商的两个账号是两行");
        assert_eq!(list(&conn).expect("列表").len(), 2);
    }

    #[test]
    fn clearing_the_token_keeps_the_api_key() {
        let conn = setup();
        let id = save_account(&conn, Vendor::DeepSeek, "tok", &acct("uuid-a")).expect("存");
        set_api_key(&conn, id, "sk-plaintext").expect("存 key");

        clear_token(&conn, id).expect("清 token");

        let row = get(&conn, id).expect("取").expect("有");
        assert!(row.auth_token.is_empty(), "token 要清掉");
        assert_eq!(
            row.api_key, "sk-plaintext",
            "⚠️ api_key 是独立凭据，网页登录态过期不影响它 —— 清掉等于无端废掉一把好 key"
        );
    }

    #[test]
    fn clearing_the_token_keeps_the_login_identifier() {
        let conn = setup();
        let id = save_account(&conn, Vendor::DeepSeek, "tok", &acct("uuid-a")).expect("存");
        clear_token(&conn, id).expect("清");
        assert_eq!(
            get(&conn, id).expect("取").expect("有").login_identifier,
            "13800000000",
            "清 token 正是重登前那一步，清掉预填值等于让用户重输一遍手机号"
        );
    }

    #[test]
    fn list_is_ordered_by_sort_index() {
        let conn = setup();
        let a = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-a")).expect("a");
        let b = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-b")).expect("b");
        conn.execute(
            "UPDATE loongport_vendor SET sort_index = 5 WHERE id = ?1",
            [a],
        )
        .expect("改序");
        conn.execute(
            "UPDATE loongport_vendor SET sort_index = 1 WHERE id = ?1",
            [b],
        )
        .expect("改序");
        let ids: Vec<i64> = list(&conn).expect("列表").iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![b, a]);
    }

    #[test]
    fn reorder_writes_the_dragged_order() {
        let conn = setup();
        let a = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-a")).expect("a");
        let b = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-b")).expect("b");
        let c = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-c")).expect("c");

        reorder(&conn, &[c, a, b]).expect("排序");

        let ids: Vec<i64> = list(&conn).expect("列表").iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![c, a, b], "list 的顺序要跟着 sort_index 走");
    }

    /// 传进来的 id 里混进别人的行（前端把两类行拼在一个数组里）时不能报错、
    /// 也不能影响本表的相对顺序 —— `UPDATE ... WHERE id = ?` 命不中就是 no-op。
    #[test]
    fn reorder_ignores_ids_that_are_not_in_this_table() {
        let conn = setup();
        let a = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-a")).expect("a");
        let b = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-b")).expect("b");

        reorder(&conn, &[9999, b, 8888, a]).expect("不该报错");

        let ids: Vec<i64> = list(&conn).expect("列表").iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![b, a]);
    }

    #[test]
    fn removing_a_row_leaves_the_others() {
        let conn = setup();
        let a = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-a")).expect("a");
        let b = save_account(&conn, Vendor::DeepSeek, "t", &acct("uuid-b")).expect("b");
        remove(&conn, a).expect("删");
        let ids: Vec<i64> = list(&conn).expect("列表").iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![b]);
    }

    #[test]
    fn create_table_is_idempotent() {
        let conn = setup();
        create_table(&conn).expect("再建一次不该报错");
    }

    /// 老库（v20，没有本表）升级后必须有这张表且可写。
    #[test]
    fn an_upgraded_database_gets_the_table() {
        let conn = Connection::open_in_memory().expect("内存库");
        // 模拟 v20：只建 relay 的表，不建 vendor 的
        crate::relay::creds::create_table(&conn).expect("relay 表");
        assert!(
            list(&conn).is_err(),
            "前提：升级前本表不存在（否则这条闸没有判别力）"
        );

        create_table(&conn).expect("迁移建表");
        assert!(list(&conn).is_ok(), "迁移后必须可读");
        save_account(
            &conn,
            Vendor::DeepSeek,
            "t",
            &VendorAccount {
                account_id: "u".into(),
                label: "l".into(),
                login_identifier: "1".into(),
            },
        )
        .expect("迁移后必须可写");
    }
}
