//! 运营商站点与凭据的持久化。
//!
//! 一张表 `loongport_operator`，一行一个「站点 × 账号」：
//!
//! | 列 | 含义 |
//! |---|---|
//! | `id` | 自增主键 |
//! | `site_origin` | 面板 origin，如 `https://bestapi.store` |
//! | `site_name` | 展示名，来自探测结果 |
//! | `api_base_url` | 归一后的 codex `base_url`（带 `/v1`） |
//! | `account_id` | 服务端的用户 id。**登录后才知道**，未登录时为 `NULL` |
//! | `account_label` | 给人看的账号名（昵称优先，回落邮箱） |
//! | `login_identifier` | 重登时预填进登录框的值。**给机器填表单用**，见字段注释 |
//! | `device_id` | 本机 UUID v4，用于 Key 命名。**全局共用一个** |
//! | `auth_token` / `refresh_token` / `token_expires_at` | 登录凭据 |
//! | `is_current` | 当前选中的那一行（同时只有一行为 1） |
//!
//! ## 去重是「域名 + 账号」，不是只看域名
//!
//! 同一个站上可以挂多个账号（自己的号 + 测试号），所以 `(site_origin, account_id)` 才是
//! 唯一键。但 `account_id` **登录之后才拿得到** —— 所以流程是：
//!
//! 1. 添加站点时先建一行，`account_id` 为 `NULL`（"这个站，还不知道是谁"）；
//! 2. 登录成功、拉到 profile 后回填 `account_id`；
//! 3. 回填时若发现已有同 `(site_origin, account_id)` 的行，**合并**掉刚建的这条。
//!
//! 唯一索引建在 `(site_origin, account_id)` 上。SQLite 的唯一索引把 `NULL` 视为互不相等，
//! 所以「同一个站的多条未登录行」不会被约束拦住 —— 那正是我们要的（用户可能连点两次添加），
//! 由 [`save_site`] 自己收口成一行。
//!
//! ## device_id 全局一个，不按站分
//!
//! 它进 Key 名字表示「这台机器」，与站点无关。按站分会让同一台机器在不同站上有不同身份，
//! 没有意义。

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

/// 一个站点 × 账号（含凭据）。
#[derive(Debug, Clone)]
pub struct Operator {
    pub id: i64,
    pub site_origin: String,
    pub site_name: String,
    /// codex 用的 API 基址，已归一到带 `/v1`。
    ///
    /// ⚠️ **这是「codex 的 base」，不是「这个站的 base」** —— Anthropic / Gemini 在同一个站上
    /// 要不同的 base 路径。多平台展开那轮要么新增按平台分的列、要么改成 JSON 列，
    /// 那时是**一次迁移**（已持久化的列），不是重构。
    ///
    /// 现在不提前分化：当前只有 codex 会填它，按平台存一组 base 的那层抽象没有实现填充它
    /// （尺子3：这层现在就有实现填充它吗？没有 ⇒ 只切边界不加抽象）。
    pub api_base_url: String,
    /// 服务端的用户 id。`None` = 还没登录过。
    pub account_id: Option<i64>,
    /// 给人看的账号名：昵称优先，回落邮箱。**不参与去重**（去重认 `account_id`）——
    /// 用户在运营商那边改昵称不该让我们把同一个账号当成两个。
    pub account_label: String,
    /// 重新登录时预填进登录框的那个值。**给机器填表单用，不是给人看的**
    /// （给人看的是 [`Operator::account_label`]）。
    ///
    /// ## 为什么叫 `login_identifier` 而不是 `account_email`
    ///
    /// 各家运营商的登录标识不同名也不同语义，实测两个上游就已经分岔：
    ///
    /// | 运营商 | 字段 | 校验 | 语义 |
    /// |---|---|---|---|
    /// | sub2api | `email` | `binding:"required,email"` | 必须邮箱格式 |
    /// | new-api | `username` | 无 | 用户名，也可能是邮箱 |
    ///
    /// 叫 `account_email` 会把 sub2api 的实现细节固化进 schema，而**列名进了持久化 schema，
    /// 改它是迁移不是重构**。用中立名字现在零成本，将来接 new-api 不必动库。
    /// （`login_identifier` 对齐 OIDC 的 `login_hint` —— 那是「预填登录框的值」的正式术语。）
    ///
    /// 空串 = 还没登录过，或是 v18 之前的旧库还没回填（那时不预填，让用户自己输）。
    pub login_identifier: String,
    pub device_id: String,
    pub auth_token: String,
    pub refresh_token: Option<String>,
    /// Unix 秒。`None` 表示服务端没给（降级态：可用但不可续期）。
    pub token_expires_at: Option<i64>,
    pub is_current: bool,
}

impl Operator {
    /// 凭据是否还能用于发请求。
    ///
    /// **过期判定留 60 秒余量**：正好卡在边界上发请求会拿到 401，白跑一趟。
    pub fn token_looks_valid(&self, now_unix: i64) -> bool {
        if self.auth_token.is_empty() {
            return false;
        }
        match self.token_expires_at {
            // 服务端没给过期时间是已知的降级态，此时只能乐观地用，过期了靠 401 发现。
            None => true,
            Some(exp) => exp > now_unix + 60,
        }
    }

    /// 用户看到的名字。同一个站挂多个账号时要能分辨。
    pub fn display_label(&self) -> String {
        if self.account_label.is_empty() {
            self.site_name.clone()
        } else {
            format!("{} · {}", self.site_name, self.account_label)
        }
    }
}

/// 这张表是不是已经是 v18 的形态（有 `account_id` 列）。
fn is_v18_shape(conn: &Connection) -> bool {
    conn.prepare("SELECT account_id FROM loongport_operator LIMIT 0")
        .is_ok()
}

/// 这张表是不是已经是 v19 的形态（有 `login_identifier` 列）。
fn is_v19_shape(conn: &Connection) -> bool {
    conn.prepare("SELECT login_identifier FROM loongport_operator LIMIT 0")
        .is_ok()
}

/// v18 → v19：补 `login_identifier` 列。
///
/// 与 v17→v18 不同，这次不必重建表 —— 只是加一列，`ALTER TABLE ADD COLUMN` 就够
/// （那次要重建是因为 v17 有 `CHECK (id = 1)`，SQLite 改不动列约束）。
///
/// 旧库补上的列是空串：**已登录的行不会因此掉线**（登录标识只用于「重新登录时预填」，
/// 不参与鉴权也不参与去重），只是下次要重登时不预填，等那次登录成功后自然回填。
pub fn migrate_v18_to_v19(conn: &Connection) -> Result<(), AppError> {
    // 全新库走 create_table 那条路，表里本来就有这一列。
    if !is_v18_shape(conn) {
        return create_table(conn);
    }
    // 迁移可能因为上一次中断而重跑。
    if is_v19_shape(conn) {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE loongport_operator
         ADD COLUMN login_identifier TEXT NOT NULL DEFAULT ''",
        [],
    )
    .map_err(|e| AppError::Database(format!("添加 login_identifier 列失败: {e}")))?;

    Ok(())
}

/// 建表 + 索引。全新库与迁移都走它。
///
/// ## 为什么索引要单独判一次表形态
///
/// **`create_tables_on_conn` 在迁移之前跑**（`Database::init` 先建表再迁移）。升级的库上
/// `CREATE TABLE IF NOT EXISTS` 会跳过 —— 那时表还是 v17 的形态、没有 `account_id` 列，
/// 而索引引用了它，`CREATE INDEX` 当场报 `no such column: account_id`，整个 app 起不来
/// （实测踩过，用户看到的是「数据库错误」弹窗）。
///
/// 所以索引只在表确实是 v18 形态时建；旧表那一路由 [`migrate_v17_to_v18`] 建完新表后补上。
pub fn create_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS loongport_operator (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            site_origin TEXT NOT NULL,
            site_name TEXT NOT NULL DEFAULT '',
            api_base_url TEXT NOT NULL DEFAULT '',
            account_id INTEGER,
            account_label TEXT NOT NULL DEFAULT '',
            login_identifier TEXT NOT NULL DEFAULT '',
            device_id TEXT NOT NULL,
            auth_token TEXT NOT NULL DEFAULT '',
            refresh_token TEXT,
            token_expires_at INTEGER,
            is_current INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 loongport_operator 表失败: {e}")))?;

    if is_v18_shape(conn) {
        // 去重键。SQLite 把 NULL 视为互不相等 ⇒ 多条未登录行不受约束，由 save_site 收口。
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_loongport_operator_site_account
             ON loongport_operator(site_origin, account_id)",
            [],
        )
        .map_err(|e| AppError::Database(format!("创建 loongport_operator 索引失败: {e}")))?;
    }

    Ok(())
}

/// v17 的单行表迁到 v18 的多行表。
///
/// v17 的表有 `CHECK (id = 1)`，改不动列约束 —— SQLite 的 `ALTER TABLE` 动不了 CHECK。
/// 所以建新表、搬那一行、换名。
pub fn migrate_v17_to_v18(conn: &Connection) -> Result<(), AppError> {
    // 老表不存在（全新库走 create_table 那条路）时什么都不用做。
    let has_old: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='loongport_operator'",
            [],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("检查旧表失败: {e}")))?
        .unwrap_or(false);
    if !has_old {
        return create_table(conn);
    }

    // 已经是新表就不用迁。迁移可能因为上一次中断而重跑。
    if is_v18_shape(conn) {
        // 但索引可能还没建上（`create_table` 在旧形态下会跳过它），补一次。
        return create_table(conn);
    }

    conn.execute(
        "ALTER TABLE loongport_operator RENAME TO loongport_operator_v17",
        [],
    )
    .map_err(|e| AppError::Database(format!("重命名旧表失败: {e}")))?;

    create_table(conn)?;

    // 搬那一行。老表没有 account_id / account_label，留空；它是当前选中项（就它一个）。
    conn.execute(
        "INSERT INTO loongport_operator
            (site_origin, site_name, api_base_url, device_id,
             auth_token, refresh_token, token_expires_at, is_current, updated_at)
         SELECT site_origin, site_name, api_base_url, device_id,
                auth_token, refresh_token, token_expires_at, 1, updated_at
         FROM loongport_operator_v17",
        [],
    )
    .map_err(|e| AppError::Database(format!("迁移运营商数据失败: {e}")))?;

    conn.execute("DROP TABLE loongport_operator_v17", [])
        .map_err(|e| AppError::Database(format!("删除旧表失败: {e}")))?;

    Ok(())
}

const SELECT_COLS: &str =
    "id, site_origin, site_name, api_base_url, account_id, account_label, login_identifier, \
     device_id, auth_token, refresh_token, token_expires_at, is_current";

fn row_to_operator(row: &rusqlite::Row<'_>) -> rusqlite::Result<Operator> {
    Ok(Operator {
        id: row.get(0)?,
        site_origin: row.get(1)?,
        site_name: row.get(2)?,
        api_base_url: row.get(3)?,
        account_id: row.get(4)?,
        account_label: row.get(5)?,
        login_identifier: row.get(6)?,
        device_id: row.get(7)?,
        auth_token: row.get(8)?,
        refresh_token: row.get(9)?,
        token_expires_at: row.get(10)?,
        is_current: row.get::<_, i64>(11)? != 0,
    })
}

/// 读当前选中的站点（没有任何站点时返回 `None`）。
pub fn load(conn: &Connection) -> Result<Option<Operator>, AppError> {
    // 没有任何行被标为 current 时（理论上不该发生）回落到最早那条，别让 UI 空着。
    conn.query_row(
        &format!(
            "SELECT {SELECT_COLS} FROM loongport_operator
             ORDER BY is_current DESC, id ASC LIMIT 1"
        ),
        [],
        row_to_operator,
    )
    .optional()
    .map_err(|e| AppError::Database(format!("读取运营商失败: {e}")))
}

/// 列出全部站点，当前选中的排在最前。
pub fn list(conn: &Connection) -> Result<Vec<Operator>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_COLS} FROM loongport_operator ORDER BY is_current DESC, id ASC"
        ))
        .map_err(|e| AppError::Database(format!("准备查询失败: {e}")))?;
    let rows = stmt
        .query_map([], row_to_operator)
        .map_err(|e| AppError::Database(format!("列出运营商失败: {e}")))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| AppError::Database(format!("读取运营商行失败: {e}")))
}

/// 按 id 读一行。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Operator>, AppError> {
    conn.query_row(
        &format!("SELECT {SELECT_COLS} FROM loongport_operator WHERE id = ?1"),
        params![id],
        row_to_operator,
    )
    .optional()
    .map_err(|e| AppError::Database(format!("读取运营商失败: {e}")))
}

/// 本机 device-id，没有就生成。
///
/// 全局一个：它在 Key 名字里表示「这台机器」，与站点无关。
fn ensure_device_id(conn: &Connection) -> Result<String, AppError> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT device_id FROM loongport_operator WHERE device_id != '' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("读取 device_id 失败: {e}")))?;
    Ok(existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()))
}

/// 添加或更新一个站点，并把它设为当前选中。返回那一行的 id。
///
/// 同一个站点若已有**未登录**的行（`account_id IS NULL`），复用它而不是再插一条 ——
/// 用户连点两次「添加」不该得到两行。已登录的行不动（那是别的账号，或同一账号的既有配置）。
pub fn save_site(
    conn: &Connection,
    site_origin: &str,
    site_name: &str,
    api_base_url: &str,
) -> Result<i64, AppError> {
    let device_id = ensure_device_id(conn)?;
    let now = now_unix();

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM loongport_operator
             WHERE site_origin = ?1 AND account_id IS NULL LIMIT 1",
            params![site_origin],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("查询已有站点失败: {e}")))?;

    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE loongport_operator
                 SET site_name = ?1, api_base_url = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![site_name, api_base_url, now, id],
            )
            .map_err(|e| AppError::Database(format!("更新站点失败: {e}")))?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO loongport_operator
                    (site_origin, site_name, api_base_url, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![site_origin, site_name, api_base_url, &device_id, now],
            )
            .map_err(|e| AppError::Database(format!("保存站点失败: {e}")))?;
            conn.last_insert_rowid()
        }
    };

    set_current(conn, id)?;
    Ok(id)
}

/// 把某一行设为当前选中（其余置 0）。
pub fn set_current(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE loongport_operator SET is_current = CASE WHEN id = ?1 THEN 1 ELSE 0 END",
        params![id],
    )
    .map_err(|e| AppError::Database(format!("切换当前站点失败: {e}")))?;
    Ok(())
}

/// 登录成功后拿到的账号身份。
///
/// 打成一个结构体而不是三个平铺参数：`label` 与 `login_identifier` 都是 `&str`、意思却相反
/// （一个给人看、一个给机器填表单），平铺时相邻的两个同类型参数**调换了编译器也不会报**，
/// 而后果是把昵称填进登录框。带字段名就调不错了。
#[derive(Debug, Clone, Copy)]
pub struct AccountIdentity<'a> {
    /// 服务端的用户 id。**去重认这个**，改邮箱改昵称都不变。
    pub id: i64,
    /// 给人看的名字（昵称优先，回落邮箱）。
    pub label: &'a str,
    /// 重登时预填进登录框的值。见 [`Operator::login_identifier`]。
    pub login_identifier: &'a str,
}

/// 写入登录凭据与账号身份，并在发现重复时合并。
///
/// 返回**最终生效的那一行的 id** —— 可能不是传进来的 `id`：如果这个站上已经有同一个账号的
/// 行（用户重新添加了已配过的站），凭据写进那一行、把刚建的这条删掉。
pub fn save_credentials(
    conn: &Connection,
    id: i64,
    account: AccountIdentity<'_>,
    auth_token: &str,
    refresh_token: Option<&str>,
    token_expires_at: Option<i64>,
) -> Result<i64, AppError> {
    let AccountIdentity {
        id: account_id,
        label: account_label,
        login_identifier,
    } = account;
    let site_origin: String = conn
        .query_row(
            "SELECT site_origin FROM loongport_operator WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("读取站点失败: {e}")))?
        .ok_or_else(|| AppError::Config("保存凭据失败: 站点记录不存在".into()))?;

    // 这个站上是否已有同一个账号的行（排除自己）。
    let duplicate: Option<i64> = conn
        .query_row(
            "SELECT id FROM loongport_operator
             WHERE site_origin = ?1 AND account_id = ?2 AND id != ?3 LIMIT 1",
            params![&site_origin, account_id, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("查询重复账号失败: {e}")))?;

    let target = match duplicate {
        // 已经配过这个「站 × 账号」：凭据写回那一行，删掉这次新建的。
        Some(existing) => {
            conn.execute("DELETE FROM loongport_operator WHERE id = ?1", params![id])
                .map_err(|e| AppError::Database(format!("清理重复站点失败: {e}")))?;
            log::info!("站点 {site_origin} 的账号 {account_id} 已存在，合并到已有记录");
            existing
        }
        None => id,
    };

    conn.execute(
        "UPDATE loongport_operator
         SET account_id = ?1, account_label = ?2, login_identifier = ?3, auth_token = ?4,
             refresh_token = ?5, token_expires_at = ?6, updated_at = ?7
         WHERE id = ?8",
        params![
            account_id,
            account_label,
            login_identifier,
            auth_token,
            refresh_token,
            token_expires_at,
            now_unix(),
            target
        ],
    )
    .map_err(|e| AppError::Database(format!("保存凭据失败: {e}")))?;

    set_current(conn, target)?;
    Ok(target)
}

/// 只更新 token（续期用），不碰账号身份、不做去重。
///
/// 与 [`save_credentials`] 分开是因为语义不同：续期是「同一个账号换一把新 token」，账号没变
/// ⇒ 没有重复可言。走那条会白查一次重复，还得传一遍已知的 account_id。
pub fn update_tokens(
    conn: &Connection,
    id: i64,
    auth_token: &str,
    refresh_token: Option<&str>,
    token_expires_at: Option<i64>,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE loongport_operator
             SET auth_token = ?1, refresh_token = ?2, token_expires_at = ?3, updated_at = ?4
             WHERE id = ?5",
            params![auth_token, refresh_token, token_expires_at, now_unix(), id],
        )
        .map_err(|e| AppError::Database(format!("更新 token 失败: {e}")))?;
    if changed == 0 {
        return Err(AppError::Config("更新 token 失败: 站点记录不存在".into()));
    }
    Ok(())
}

/// 刷新账号的展示名与登录标识（用户在运营商那边改了昵称 / 邮箱之后）。
///
/// ## 为什么单独一个函数，而不是让 `update_tokens` 一起刷
///
/// **续期响应里没有账号信息** —— `/api/v1/auth/refresh` 只回 `access_token` /
/// `refresh_token` / `expires_at`（实测，见 [`crate::operator::api::refresh_token`]）。
/// 想刷标签就得额外打一次 `/user/profile`，那是独立的一次网络请求，不该塞进
/// 「只写库、不联网」的 `update_tokens` 里。
///
/// ## 不碰 `account_id`
///
/// 改邮箱改昵称时服务端主键不变，所以这里只更新展示与预填用的两个字段。**账号真的换了**
/// 是另一回事，走 [`save_credentials`]（它会查重、可能合并行）。
pub fn refresh_account_identity(
    conn: &Connection,
    id: i64,
    account_label: &str,
    login_identifier: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE loongport_operator
             SET account_label = ?1, login_identifier = ?2, updated_at = ?3
             WHERE id = ?4",
            params![account_label, login_identifier, now_unix(), id],
        )
        .map_err(|e| AppError::Database(format!("刷新账号信息失败: {e}")))?;
    if changed == 0 {
        return Err(AppError::Config("刷新账号信息失败: 站点记录不存在".into()));
    }
    Ok(())
}

/// 清掉某一行的凭据但保留站点与 device_id（登出 / 凭据失效后重登用）。
///
/// **`account_id` 也清掉**：下次登录可能换成别的账号，留着旧的会让去重判断把新账号误认成它。
/// **device_id 必须留着** —— 它进了服务端的 Key 名字，清掉会让重登后认领不到自己已建的 Key，
/// 于是给用户账号里堆一批重复 sk。
/// **`login_identifier` 也必须留着** —— 它存在的全部理由就是「重登时预填」，
/// 而这个函数正是重登前的那一步；清掉它等于让用户重新输一遍邮箱。
pub fn clear_credentials(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE loongport_operator
         SET auth_token = '', refresh_token = NULL, token_expires_at = NULL,
             account_id = NULL, account_label = '', updated_at = ?1
         WHERE id = ?2",
        params![now_unix(), id],
    )
    .map_err(|e| AppError::Database(format!("清除凭据失败: {e}")))?;
    Ok(())
}

/// 删掉一个站点。若删的是当前选中项，把剩下最早那条设为当前。
pub fn remove(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM loongport_operator WHERE id = ?1", params![id])
        .map_err(|e| AppError::Database(format!("删除站点失败: {e}")))?;

    // 没有 current 了就补一个 —— 否则 load() 虽有回落，但 is_current 语义会一直是脏的。
    let has_current: bool = conn
        .query_row(
            "SELECT 1 FROM loongport_operator WHERE is_current = 1 LIMIT 1",
            [],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| AppError::Database(format!("检查当前站点失败: {e}")))?
        .unwrap_or(false);

    if !has_current {
        if let Some(next) = conn
            .query_row(
                "SELECT id FROM loongport_operator ORDER BY id ASC LIMIT 1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| AppError::Database(format!("查询剩余站点失败: {e}")))?
        {
            set_current(conn, next)?;
        }
    }
    Ok(())
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_table(&conn).unwrap();
        conn
    }

    /// 省点样板。多数测试不关心 label 与登录标识的区别，传同一个值即可；
    /// 关心那个区别的（`login_identifier_is_stored_separately_from_the_display_label`）
    /// 显式传两个不同的值。
    fn ident<'a>(id: i64, label: &'a str, login_identifier: &'a str) -> AccountIdentity<'a> {
        AccountIdentity {
            id,
            label,
            login_identifier,
        }
    }

    #[test]
    fn load_returns_none_before_any_site_saved() {
        assert!(load(&mem()).unwrap().is_none());
    }

    #[test]
    fn save_site_generates_device_id_once_and_shares_it() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let first = get(&conn, a).unwrap().unwrap().device_id;
        assert!(!first.is_empty());

        // 第二个站共用同一个 device_id —— 它表示「这台机器」，与站点无关。
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        assert_eq!(get(&conn, b).unwrap().unwrap().device_id, first);
    }

    #[test]
    fn adding_the_same_site_twice_before_login_reuses_one_row() {
        // 用户连点两次「添加」不该得到两行。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://a.dev", "A 改名了", "https://a.dev/v1").unwrap();
        assert_eq!(a, b, "未登录的同站行应被复用");
        assert_eq!(list(&conn).unwrap().len(), 1);
        // 顺带更新了展示名。
        assert_eq!(get(&conn, a).unwrap().unwrap().site_name, "A 改名了");
    }

    #[test]
    fn same_site_different_accounts_coexist() {
        let conn = mem();
        let first = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(
            &conn,
            first,
            ident(100, "me@x.com", "me@x.com"),
            "tok1",
            None,
            None,
        )
        .unwrap();

        // 再添加同一个站、登录另一个账号 —— 两个都该留着。
        let second = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        assert_ne!(first, second, "已登录的行不该被复用");
        let final_id = save_credentials(
            &conn,
            second,
            ident(200, "alt@x.com", "alt@x.com"),
            "tok2",
            None,
            None,
        )
        .unwrap();
        assert_eq!(final_id, second);

        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn re_adding_a_site_with_the_same_account_merges_instead_of_duplicating() {
        // 这条是用户明确要的去重：同「域名 + 账号」只留一份。
        let conn = mem();
        let first = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(
            &conn,
            first,
            ident(100, "me@x.com", "me@x.com"),
            "old-token",
            None,
            None,
        )
        .unwrap();

        // 用户又添加了一次同一个站，并用同一个账号登录。
        let second = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let final_id = save_credentials(
            &conn,
            second,
            ident(100, "me@x.com", "me@x.com"),
            "new-token",
            None,
            None,
        )
        .unwrap();

        // 合并到原来那行，新建的那条被删掉。
        assert_eq!(final_id, first, "应合并回已有记录");
        assert_eq!(list(&conn).unwrap().len(), 1);
        // 凭据用的是新的那份。
        let op = get(&conn, first).unwrap().unwrap();
        assert_eq!(op.auth_token, "new-token");
        assert!(op.is_current, "合并后该行应是当前选中");
    }

    #[test]
    fn set_current_keeps_exactly_one_row_selected() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();

        // save_site 会把新站设为当前。
        assert_eq!(load(&conn).unwrap().unwrap().id, b);
        set_current(&conn, a).unwrap();
        assert_eq!(load(&conn).unwrap().unwrap().id, a);

        let selected = list(&conn).unwrap().iter().filter(|o| o.is_current).count();
        assert_eq!(selected, 1, "同时只能有一行被选中");
    }

    #[test]
    fn clear_credentials_drops_account_identity_too() {
        // account_id 不清的话，下次换账号登录会被去重逻辑误认成同一个账号。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(
            &conn,
            a,
            ident(100, "me@x.com", "me@x.com"),
            "tok",
            Some("ref"),
            Some(123),
        )
        .unwrap();
        let device = get(&conn, a).unwrap().unwrap().device_id;

        clear_credentials(&conn, a).unwrap();
        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.auth_token, "");
        assert!(op.account_id.is_none());
        assert_eq!(op.account_label, "");
        // 站点与 device_id 必须活着 —— 后者进了服务端的 Key 名字。
        assert_eq!(op.site_origin, "https://a.dev");
        assert_eq!(op.device_id, device);
    }

    #[test]
    fn removing_the_current_site_promotes_another() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        assert_eq!(load(&conn).unwrap().unwrap().id, b, "b 是当前");

        remove(&conn, b).unwrap();
        let now = load(&conn).unwrap().unwrap();
        assert_eq!(now.id, a);
        assert!(now.is_current, "剩下的那条要被提为当前，不能留个脏状态");
    }

    #[test]
    fn removing_the_last_site_leaves_nothing() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        remove(&conn, a).unwrap();
        assert!(load(&conn).unwrap().is_none());
        assert!(list(&conn).unwrap().is_empty());
    }

    #[test]
    fn save_credentials_on_a_missing_row_is_a_visible_error() {
        // 静默成功会让「登录成功但什么都没存下」变成查不出来的问题。
        let err = save_credentials(
            &mem(),
            999,
            ident(1, "x@x.com", "x@x.com"),
            "tok",
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("站点记录不存在"));
    }

    #[test]
    fn token_validity_leaves_a_margin_and_tolerates_missing_expiry() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(
            &conn,
            a,
            ident(1, "x@x.com", "x@x.com"),
            "tok",
            None,
            Some(1000),
        )
        .unwrap();
        let base = get(&conn, a).unwrap().unwrap();

        assert!(base.token_looks_valid(0));
        // 60 秒余量内算已过期：卡边界发请求只会拿到 401，白跑一趟。
        assert!(!base.token_looks_valid(950));
        assert!(!base.token_looks_valid(1000));

        // 服务端降级态（没给 expiry）不能判成「未就位」去轮询等 —— 那会永远等不到。
        let no_expiry = Operator {
            token_expires_at: None,
            ..base.clone()
        };
        assert!(no_expiry.token_looks_valid(i64::MAX - 100));

        // 没有 token 就是没登录，与过期是两件事。
        let empty = Operator {
            auth_token: String::new(),
            ..base
        };
        assert!(!empty.token_looks_valid(0));
    }

    #[test]
    fn display_label_distinguishes_accounts_on_the_same_site() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "BestApi", "https://a.dev/v1").unwrap();
        // 还没登录：只有站名。
        assert_eq!(get(&conn, a).unwrap().unwrap().display_label(), "BestApi");

        save_credentials(
            &conn,
            a,
            ident(1, "me@x.com", "me@x.com"),
            "tok",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            get(&conn, a).unwrap().unwrap().display_label(),
            "BestApi · me@x.com"
        );
    }

    #[test]
    fn v17_single_row_table_migrates_into_the_multi_row_shape() {
        let conn = Connection::open_in_memory().unwrap();
        // 复刻 v17 的表（含那个 CHECK 约束，正是它逼出这次建新表搬数据）。
        conn.execute(
            "CREATE TABLE loongport_operator (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                device_id TEXT NOT NULL,
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO loongport_operator
                (id, site_origin, site_name, api_base_url, device_id, auth_token, updated_at)
             VALUES (1, 'https://old.dev', 'Old', 'https://old.dev/v1', 'dev-uuid', 'old-tok', 5)",
            [],
        )
        .unwrap();

        migrate_v17_to_v18(&conn).unwrap();

        // 那一行必须活着、且成为当前选中项 —— 用户升级后不该被要求重新配站与重新登录。
        let op = load(&conn).unwrap().expect("迁移后应还有那一行");
        assert_eq!(op.site_origin, "https://old.dev");
        assert_eq!(
            op.device_id, "dev-uuid",
            "device_id 丢了会让已建的 Key 认领不回来"
        );
        assert_eq!(op.auth_token, "old-tok", "凭据丢了用户就得重新登录");
        assert!(op.is_current);
        assert!(op.account_id.is_none(), "老数据没有账号身份，登录后才回填");

        // 新表能装第二行了（老表的 CHECK 会拦住）。
        save_site(&conn, "https://new.dev", "New", "https://new.dev/v1").unwrap();
        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn create_table_then_migrate_is_the_real_startup_order() {
        // **这条复刻用户实际撞到的崩溃**：`Database::init` 先跑 `create_tables_on_conn`
        // （里面调 create_table），再跑迁移。升级的库上 `CREATE TABLE IF NOT EXISTS` 跳过，
        // 那时表还是 v17 形态没有 account_id 列，而索引引用了它 ⇒
        // `no such column: account_id`，整个 app 起不来。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE loongport_operator (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                device_id TEXT NOT NULL,
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO loongport_operator
                (id, site_origin, site_name, api_base_url, device_id, auth_token)
             VALUES (1, 'https://x.dev', 'X', 'https://x.dev/v1', 'dev-1', 'tok')",
            [],
        )
        .unwrap();

        // 启动顺序第一步：建表（旧库上它必须**不炸**）。
        create_table(&conn).expect("旧表上 create_table 不该失败");
        // 第二步：迁移。
        migrate_v17_to_v18(&conn).expect("迁移应成功");

        // 数据在、且索引已补上（补不上的话重复账号就拦不住）。
        let op = load(&conn).unwrap().expect("那一行应还在");
        assert_eq!(op.auth_token, "tok");
        let has_index: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='index'
                 AND name='idx_loongport_operator_site_account'",
                [],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(has_index, "迁移后必须补上去重索引");
    }

    #[test]
    fn migration_is_idempotent() {
        // 迁移可能因为上一次中断而重跑，跑两遍不能把数据搞坏。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, a, ident(1, "x@x.com", "x@x.com"), "tok", None, None).unwrap();

        migrate_v17_to_v18(&conn).unwrap();
        migrate_v17_to_v18(&conn).unwrap();

        assert_eq!(list(&conn).unwrap().len(), 1);
        assert_eq!(get(&conn, a).unwrap().unwrap().auth_token, "tok");
    }

    /// 建一张 v18 形态的表（没有 `login_identifier` 列），用于验 v18→v19。
    fn v18_table() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE loongport_operator (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                site_origin TEXT NOT NULL,
                site_name TEXT NOT NULL DEFAULT '',
                api_base_url TEXT NOT NULL DEFAULT '',
                account_id INTEGER,
                account_label TEXT NOT NULL DEFAULT '',
                device_id TEXT NOT NULL,
                auth_token TEXT NOT NULL DEFAULT '',
                refresh_token TEXT,
                token_expires_at INTEGER,
                is_current INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn v19_migration_keeps_existing_logins_working() {
        // 升级不该把人踢下线：补的那一列只用于「重登时预填」，不参与鉴权。
        let conn = v18_table();
        conn.execute(
            "INSERT INTO loongport_operator
                (site_origin, site_name, api_base_url, account_id, account_label,
                 device_id, auth_token, is_current, updated_at)
             VALUES ('https://a.dev', 'A', 'https://a.dev/v1', 7, '张三',
                     'dev-uuid', 'tok', 1, 5)",
            [],
        )
        .unwrap();

        migrate_v18_to_v19(&conn).unwrap();

        let op = load(&conn).unwrap().expect("迁移后那一行该还在");
        assert_eq!(op.auth_token, "tok", "凭据丢了用户就得重新登录");
        assert_eq!(op.account_id, Some(7));
        assert_eq!(op.account_label, "张三");
        assert_eq!(
            op.device_id, "dev-uuid",
            "device_id 丢了会让已建的 Key 认领不回来"
        );
        // 旧库没有这个值，只能等下次登录回填 —— 那时不预填而不是预填一个错的。
        assert_eq!(op.login_identifier, "");
    }

    #[test]
    fn v19_migration_is_idempotent() {
        let conn = v18_table();
        migrate_v18_to_v19(&conn).unwrap();
        migrate_v18_to_v19(&conn).expect("重跑不该报 duplicate column");
        assert!(is_v19_shape(&conn));
    }

    #[test]
    fn login_identifier_is_stored_separately_from_the_display_label() {
        // 这条是这个字段存在的理由：设了昵称的用户，label 是昵称而不是邮箱 ——
        // 拿 label 去预填登录框就填错了（sub2api 那个框要邮箱格式）。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, a, ident(7, "张三", "me@x.com"), "tok", None, None).unwrap();

        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.account_label, "张三", "给人看的是昵称");
        assert_eq!(op.login_identifier, "me@x.com", "填表单用的是登录标识");
    }

    #[test]
    fn clearing_credentials_keeps_the_login_identifier() {
        // clear_credentials 正是「重登前的那一步」，把预填值一起清掉等于让用户重新输邮箱。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, a, ident(7, "张三", "me@x.com"), "tok", None, None).unwrap();

        clear_credentials(&conn, a).unwrap();

        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.auth_token, "", "凭据该清掉");
        assert_eq!(op.account_id, None, "account_id 该清掉（下次可能换账号）");
        assert_eq!(
            op.login_identifier, "me@x.com",
            "登录标识必须留着 —— 它就是给重登预填用的"
        );
    }

    #[test]
    fn refreshing_identity_updates_label_and_identifier_but_not_account_id() {
        // 用户在运营商那边改了昵称与邮箱：服务端主键不变 ⇒ 仍是同一个账号，
        // 只有展示与预填要跟上。续期路径靠这个函数刷，否则站点选择器一直挂旧标签。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, a, ident(7, "老名字", "old@x.com"), "tok", None, None).unwrap();

        refresh_account_identity(&conn, a, "新名字", "new@x.com").unwrap();

        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.account_label, "新名字");
        assert_eq!(op.login_identifier, "new@x.com");
        assert_eq!(op.account_id, Some(7), "改邮箱不是换账号，主键不该动");
        assert_eq!(op.auth_token, "tok", "刷身份不该碰凭据");
    }

    #[test]
    fn refreshing_identity_on_a_missing_row_is_an_error() {
        let err = refresh_account_identity(&mem(), 999, "n", "e@x.com").unwrap_err();
        assert!(err.to_string().contains("站点记录不存在"), "{err}");
    }
}
