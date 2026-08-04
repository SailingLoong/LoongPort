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
//! | `auth_token` / `refresh_token` / `token_expires_at` | 登录凭据 |
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
//! ## 曾经有个 `device_id` 列，2026-08-04 删了
//!
//! 它进服务端的 Key 名字（`LoongPort/<device_id>/…`）表示「这台机器」。那个命名
//! 改成了按账号（见 [`super::provision`] 的「为什么按账号而不按机器」——
//! 按机器会让每接一台新机器就在用户账号里多建一整套 Key，实测堆到 11 把只有 3 把在用）。
//!
//! 改完它就没有任何读取方了，所以列与字段一并删掉：留一个死字段要配
//! `allow(dead_code)`，而那会掩盖「它已经没用了」这个事实。
//!
//! **能直接删是因为当前还在测试阶段**（维护者确认没有要保护的已装机数据，
//! 两台机器的库都删掉重建了）。若将来真需要「本机标识」（设备级限流之类），
//! 那时加一列 + 一次迁移即可 —— 但**别放进本表**：`loongport_operator`
//! 参与 WebDAV/S3 同步（不在 `database/backup.rs` 的 `SYNC_SKIP_TABLES` 里），
//! 同步会把 A 机器的值搬到 B 机器上，一个「per-machine 身份」装进同步表里天生就是错的。
//!
//! ## 也曾经有个 `is_current` 列，2026-08-04 删了
//!
//! 它表示「当前选中的那一行」（同时只有一行为 1），服务的是两件事：
//! 已删的站点切换器（要标出选中的是哪个），以及无参命令的「回落到当前站」。
//!
//! **那个概念本身是错的语义**，不只是没人用了：界面是**多行并列**的，每一行各有自己的
//! 登录态、余额、档位 —— 「当前」在这里指不出任何东西。它带来过一个真实 bug
//! （按 `is_current DESC` 排 ⇒ 点某一行的登录就让它跳到第一位），还差点带来更糟的一个
//! （review 抓出：靠它定位会让「给 A 获取密钥」静默作用到 B）。
//!
//! 所以这次是**把概念连根拔掉**，不是清死代码：`load()` / `set_current()` 一并删，
//! 四条命令（login / provision / balance / purchase）的 `operator_id` 从 `Option<i64>`
//! 收成必填，加站那条路改为拿 `ProbeResult::operator_id` 显式往下传，
//! `check_session` 从「探当前站」改成**逐行探活**。`remove()` 里那段
//! 「删了当前站要提另一条」的跨行不变量维护也随之消失。
//!
//! 同样是测试阶段直接删列、不加迁移步骤（与 `device_id` 同一处境、同一做法）。

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
    /// 空串 = 还没登录过（那时不预填，让用户自己输）。
    pub login_identifier: String,
    pub auth_token: String,
    pub refresh_token: Option<String>,
    /// Unix 秒。`None` 表示服务端没给（降级态：可用但不可续期）。
    pub token_expires_at: Option<i64>,
    /// 用户手工拖动决定的行序，越小越靠前。
    ///
    /// ## 为什么需要一列专门存序
    ///
    /// 曾经 [`list`] 排的是 `ORDER BY is_current DESC, id ASC`（把「当前站」顶到第一），
    /// 于是用户点某一行的登录/获取密钥就会让行序跳动 —— 他明确指出过：
    /// **选一个档位不该重排运营商的顺序。** 那一版的根因是「用一个会变的状态当排序键」，
    /// 而 `is_current` 整个概念 2026-08-04 已删（多行并列的界面里没有「当前」可谈）。
    ///
    /// 新库默认按 id 顺序（等于添加顺序）；用户拖过之后就完全尊重他。
    ///
    /// **Rust 侧只写不读**：排序在 SQL 里做（`list` 的 `ORDER BY sort_index`），
    /// 前端拿到的顺序就是最终顺序、不需要这个数值。留在结构体里是因为
    /// `row_to_operator` 按 `SELECT_COLS` 的列序逐个 `row.get()` ——
    /// 它是最后一列，删掉字段就得同时改 `SELECT_COLS`，而两处不同步的后果
    /// 是运行期的 `Invalid column index`（编译器管不到，删 `device_id` 那次
    /// 实测踩过）。由 [`select_cols_match_the_row_reader`] 钉着。
    #[allow(dead_code)]
    pub sort_index: i64,
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

    /// **登录过、但凭据已经不能用了。**
    ///
    /// 与 `!token_looks_valid()` 不同：那个把「从没登录」与「登录过但过期」混成一件事，
    /// 而对用户是两种处境 —— 前者要输账号+密码，后者只需确认密码与人机验证。
    ///
    /// 判据是「有 `account_id`（登录过）+ token 不可用 + 没有 refresh_token」。
    /// **`refresh_token` 还在时不算过期**：下一次请求会自动续期、用户根本不必管，
    /// 报了他会白跑一次重登。
    ///
    /// 抽成方法而不是在各命令里就地写：`operator_status` 与 `operator_list_operators`
    /// 都要这个判据，两处各写一遍迟早分叉（一处改了另一处没改 ⇒ 同一个账号在
    /// 两个界面显示不同状态）。
    pub fn session_expired(&self, now_unix: i64) -> bool {
        !self.token_looks_valid(now_unix)
            && self.account_id.is_some()
            && self.refresh_token.is_none()
    }
}

/// 建表 + 索引。**全新库与 v16→v17 迁移都走它，两条路建出的形态完全一样。**
///
/// 索引不再需要「先判表形态」那道守卫：从前 `create_tables_on_conn` 跑在迁移之前，
/// 升级的库上 `CREATE TABLE IF NOT EXISTS` 会跳过，那时表还是 v17 的旧形态、
/// 没有 `account_id` 列，而索引引用了它 ⇒ `CREATE INDEX` 当场报
/// `no such column: account_id`，整个 app 起不来（实测踩过）。
///
/// 2026-08-04 把 LoongPort 自己那段迁移压紧重编之后，**旧形态不再存在** ——
/// 表只有一种形状，索引可以无条件建。见 `database/schema.rs` 那段说明。
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
            auth_token TEXT NOT NULL DEFAULT '',
            refresh_token TEXT,
            token_expires_at INTEGER,
            sort_index INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 loongport_operator 表失败: {e}")))?;

    // 去重键。SQLite 把 NULL 视为互不相等 ⇒ 多条未登录行不受约束，由 save_site 收口。
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_loongport_operator_site_account
         ON loongport_operator(site_origin, account_id)",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 loongport_operator 索引失败: {e}")))?;

    Ok(())
}

/// ⚠️ **这里的列序与 [`row_to_operator`] 的位置索引是一份契约，编译器管不到。**
///
/// 增删列时两处必须同步改，且 `row.get(n)` 的 `n` 会**整片后移** ——
/// 删 `device_id` 那次实测踩过：只改到一半，后三个字段仍读旧索引，
/// 于是 `creds` 模块 20+ 条测试同时红在 `Invalid column index: 12`。
///
/// 由 [`select_cols_match_the_row_reader`] 钉住两者的列数一致。
const SELECT_COLS: &str =
    "id, site_origin, site_name, api_base_url, account_id, account_label, login_identifier, \
     auth_token, refresh_token, token_expires_at, sort_index";

fn row_to_operator(row: &rusqlite::Row<'_>) -> rusqlite::Result<Operator> {
    Ok(Operator {
        id: row.get(0)?,
        site_origin: row.get(1)?,
        site_name: row.get(2)?,
        api_base_url: row.get(3)?,
        account_id: row.get(4)?,
        account_label: row.get(5)?,
        login_identifier: row.get(6)?,
        auth_token: row.get(7)?,
        refresh_token: row.get(8)?,
        token_expires_at: row.get(9)?,
        sort_index: row.get(10)?,
    })
}

/// 列出全部站点，当前选中的排在最前。
pub fn list(conn: &Connection) -> Result<Vec<Operator>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            // ⚠️ 排序键必须是**用户拖出来的那个**，不能是任何会被别的操作改动的状态：
            // 曾经按 `is_current DESC` 排，于是点某一行的登录/获取密钥就让行序跳。
            // 用户明确指出过：选一个档位不该重排运营商的顺序。
            //
            // 按 `sort_index`（用户拖出来的顺序）；同值时按 id 稳定兜底 ——
            // 新库那一列默认 0，此时退化成添加顺序，与迁移前一致。
            "SELECT {SELECT_COLS} FROM loongport_operator ORDER BY sort_index ASC, id ASC"
        ))
        .map_err(|e| AppError::Database(format!("准备查询失败: {e}")))?;
    let rows = stmt
        .query_map([], row_to_operator)
        .map_err(|e| AppError::Database(format!("列出运营商失败: {e}")))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| AppError::Database(format!("读取运营商行失败: {e}")))
}

/// 按用户拖出来的顺序重写 `sort_index`。
///
/// `ids` 是拖动后的完整顺序（前端给的），下标即新的 `sort_index`。
/// **整批重写而不是只改动过的那几行**：前端本来就持有全序，逐行 diff 反而要在两边
/// 各维护一套「哪些变了」的逻辑，而这张表最多几行，整写的代价可以忽略。
///
/// 事务包起来 —— 中途失败留下一半新一半旧的顺序，界面上就是乱序。
pub fn reorder(conn: &Connection, ids: &[i64]) -> Result<(), AppError> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    for (idx, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE loongport_operator SET sort_index = ?1, updated_at = ?2 WHERE id = ?3",
            params![idx as i64, now_unix(), id],
        )
        .map_err(|e| AppError::Database(format!("更新排序失败: {e}")))?;
    }
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交排序失败: {e}")))?;
    Ok(())
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
                    (site_origin, site_name, api_base_url, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![site_origin, site_name, api_base_url, now],
            )
            .map_err(|e| AppError::Database(format!("保存站点失败: {e}")))?;
            conn.last_insert_rowid()
        }
    };

    Ok(id)
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
/// ## `account_id` 只在它**为空时**补上，不覆盖已有值
///
/// 改邮箱改昵称时服务端主键不变，所以正常情况下这里不需要写 `account_id`；
/// **账号真的换了**是另一回事，走 [`save_credentials`]（它会查重、可能合并行）。
///
/// ⭐ 但「为空时补上」是必须的，否则有一类行**永久修不好**（实测踩到）：
///
/// 一行若处在「有 `auth_token` 但 `account_id` 为空」的状态（早期版本或中途失败的
/// 登录留下的），它就再也补不齐了 —— [`Operator::token_looks_valid`] 在
/// `token_expires_at` 为 `NULL` 时返回 `true`（那是有意的乐观降级），于是
/// `usable_operator` 直接早退、**不走续期**，而续期后那次 profile 请求是唯一
/// 会拿到 `account.id` 的地方。闭环之后：用户点任何「刷新」都补不上。
///
/// 后果不只是少个字段：`account_id` 为空 ⇒ [`save_credentials`] 的去重查不到它
/// ⇒ 同一个账号重新登录会**新建一行**而不是合并，站点列表里堆重复。
///
/// ⚠️ **不覆盖非空值**：`account_id` 有唯一索引 `(site_origin, account_id)`，
/// 无条件写可能撞上同站另一行 ⇒ 那属于「换账号」，不是这个函数的职责。
pub fn refresh_account_identity(
    conn: &Connection,
    id: i64,
    account: AccountIdentity<'_>,
) -> Result<(), AppError> {
    let AccountIdentity {
        id: account_id,
        label: account_label,
        login_identifier,
    } = account;
    let changed = conn
        .execute(
            "UPDATE loongport_operator
             SET account_label = ?1, login_identifier = ?2, updated_at = ?3,
                 -- 只在为空时补：非空说明账号身份已知，换账号该走 save_credentials。
                 account_id = COALESCE(account_id, ?4)
             WHERE id = ?5",
            params![account_label, login_identifier, now_unix(), account_id, id],
        )
        .map_err(|e| AppError::Database(format!("刷新账号信息失败: {e}")))?;
    if changed == 0 {
        return Err(AppError::Config("刷新账号信息失败: 站点记录不存在".into()));
    }
    Ok(())
}

/// 清掉某一行的凭据但保留站点（登出 / 凭据失效后重登用）。
///
/// **`account_id` 也清掉**：下次登录可能换成别的账号，留着旧的会让去重判断把新账号误认成它。
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

/// 删掉一个站点。
///
/// 2026-08-04 之前这里还有一段「删的是当前选中项就把剩下最早那条提为当前」——
/// 随 `is_current` 一起删了。没有「当前站」这个概念之后，删一行就只是删一行，
/// 不必再维护任何跨行的不变量。
pub fn remove(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM loongport_operator WHERE id = ?1", params![id])
        .map_err(|e| AppError::Database(format!("删除站点失败: {e}")))?;
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
    /// 关心那个区别的（`login_identifier_is_stored_separately_from_the_account_label`）
    /// 显式传两个不同的值。
    fn ident<'a>(id: i64, label: &'a str, login_identifier: &'a str) -> AccountIdentity<'a> {
        AccountIdentity {
            id,
            label,
            login_identifier,
        }
    }

    /// **`SELECT_COLS` 的列数必须等于 `row_to_operator` 读的个数。**
    ///
    /// 那两处是一份契约，而**编译器管不到**：`row.get(n)` 的 `n` 是运行期索引，
    /// 少一列不会编译失败，只会在真的查库时报 `Invalid column index`。
    ///
    /// 删 `device_id` 那次实测踩过：`SELECT_COLS` 少了一列、`row_to_operator`
    /// 只改到一半（后三个字段仍读旧索引），于是 `creds` 模块 20+ 条测试
    /// 同时红在 `Invalid column index: 12`。虽然被测试拦住了，但报错点离根因很远
    /// （每条测试都在喊「读运营商失败」），查起来绕。这条直接指出「是列数对不上」。
    ///
    /// 用真的查一行来验而不是数字符串里的逗号：后者数得出 12 却证明不了
    /// `row.get(11)` 拿到的是 `sort_index` 而不是别的列。
    #[test]
    fn select_cols_match_the_row_reader() {
        let conn = mem();
        let id = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(
            &conn,
            id,
            ident(7, "标签", "me@x.com"),
            "tok",
            Some("ref"),
            Some(123),
        )
        .unwrap();

        // 每个字段都取回**它自己**的值 —— 索引错位时这些会互相串或直接报错。
        let op = get(&conn, id).unwrap().expect("那一行该在");
        assert_eq!(op.id, id);
        assert_eq!(op.site_origin, "https://a.dev");
        assert_eq!(op.site_name, "A");
        assert_eq!(op.api_base_url, "https://a.dev/v1");
        assert_eq!(op.account_id, Some(7));
        assert_eq!(op.account_label, "标签");
        assert_eq!(op.login_identifier, "me@x.com");
        assert_eq!(op.auth_token, "tok");
        assert_eq!(op.refresh_token.as_deref(), Some("ref"));
        assert_eq!(op.token_expires_at, Some(123));

        // 列数也直接对一遍：`SELECT *` 的实际列数必须与 `SELECT_COLS` 一致，
        // 否则说明表里有列没被读（那是「加了列忘了读」的另一半）。
        let n_selected = SELECT_COLS.split(',').count();
        let n_in_table: usize = conn
            .prepare("SELECT * FROM loongport_operator LIMIT 0")
            .unwrap()
            .column_count();
        assert_eq!(
            n_selected + 1,
            n_in_table,
            "SELECT_COLS 有 {n_selected} 列、表里有 {n_in_table} 列 —— \
             差值应恰好是 1（`updated_at` 不进结构体）。不等说明加/删列时漏了一处。"
        );
    }

    #[test]
    fn list_is_empty_before_any_site_saved() {
        assert!(list(&mem()).unwrap().is_empty());
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
        clear_credentials(&conn, a).unwrap();
        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.auth_token, "");
        assert!(op.account_id.is_none());
        assert_eq!(op.account_label, "");
        // 站点必须活着 —— 登出只清凭据，不是删站点。
        assert_eq!(op.site_origin, "https://a.dev");
    }

    #[test]
    fn removing_one_site_leaves_the_others_untouched() {
        // 2026-08-04 之前这条叫 `removing_the_current_site_promotes_another`，
        // 断言的是「删掉当前站要把剩下最早那条提为当前」。`is_current` 整个概念
        // 已删 ⇒ 现在要钉的是更简单的事实：删一行只影响那一行。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();

        remove(&conn, b).unwrap();

        let rows = list(&conn).unwrap();
        assert_eq!(rows.len(), 1, "只该少掉被删那一行");
        assert_eq!(rows[0].id, a);
        assert!(get(&conn, b).unwrap().is_none(), "b 该真的没了");
    }

    #[test]
    fn removing_the_last_site_leaves_nothing() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        remove(&conn, a).unwrap();
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
    fn session_expired_separates_never_logged_in_from_credentials_gone_stale() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();

        // 从没登录：token 无效但也**不是过期** —— 前端该摆「登录」而不是「登录已过期」。
        let fresh = get(&conn, a).unwrap().unwrap();
        assert!(!fresh.token_looks_valid(0));
        assert!(
            !fresh.session_expired(0),
            "没登录过不算过期，否则用户会被提示「重新登录」而他从没登录过"
        );

        // 登录过 + token 过期 + 没有 refresh_token ⇒ 真过期。
        save_credentials(
            &conn,
            a,
            ident(1, "x@x.com", "x@x.com"),
            "tok",
            None,
            Some(1000),
        )
        .unwrap();
        let stale = get(&conn, a).unwrap().unwrap();
        assert!(stale.session_expired(2000));
        // 同一条记录在 token 还有效时不算过期。
        assert!(!stale.session_expired(0));

        // 有 refresh_token 时**不报过期**：下次请求会自动续期，报了用户白跑一次重登。
        let renewable = Operator {
            refresh_token: Some("rt".into()),
            ..stale
        };
        assert!(
            !renewable.session_expired(2000),
            "refresh_token 还在就该静默续期，不该催用户重登"
        );
    }

    #[test]
    fn list_order_follows_sort_index_only() {
        // ⚠️ **这条是用户报的 bug 的回归测试**：原来排序是
        // `ORDER BY is_current DESC, id ASC`，而 `is_current` 会因为用户点某一行的
        // 登录/获取密钥而改变 ⇒ 那一行跳到第一位 ⇒ **选个档位就重排了运营商顺序**。
        //
        // 那一列 2026-08-04 已整个删掉（见模块文档），所以现在钉的是更强的性质：
        // **除了 `reorder`，没有任何操作能改变行序** —— 这里拿「登录」当代表，
        // 它是当初真的会让行跳位的那个操作（`save_credentials` 曾在末尾 `set_current`）。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        let c = save_site(&conn, "https://c.dev", "C", "https://c.dev/v1").unwrap();

        let order: Vec<i64> = list(&conn).unwrap().into_iter().map(|o| o.id).collect();
        assert_eq!(
            order,
            vec![a, b, c],
            "新库 sort_index 全为 0 ⇒ 退化成按 id（添加顺序）"
        );

        // 给最后那行登录 —— 行序仍然不能变（这正是用户报的那个 bug 的形态）。
        save_credentials(&conn, c, ident(1, "c@x.com", "c@x.com"), "tok", None, None).unwrap();
        let order: Vec<i64> = list(&conn).unwrap().into_iter().map(|o| o.id).collect();
        assert_eq!(order, vec![a, b, c], "登录某一行不该让它跳到最前");

        // 给第一行登录也一样。
        save_credentials(&conn, a, ident(2, "a@x.com", "a@x.com"), "tok", None, None).unwrap();
        let order: Vec<i64> = list(&conn).unwrap().into_iter().map(|o| o.id).collect();
        assert_eq!(order, vec![a, b, c], "任何登录都不该改变行序");
    }

    #[test]
    fn reorder_persists_user_order() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        let b = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        let c = save_site(&conn, "https://c.dev", "C", "https://c.dev/v1").unwrap();

        // 用户把 C 拖到最前。
        reorder(&conn, &[c, a, b]).unwrap();
        let order: Vec<i64> = list(&conn).unwrap().into_iter().map(|o| o.id).collect();
        assert_eq!(order, vec![c, a, b]);

        // 拖完之后登录其中一行，顺序仍不变 —— 两件事互不干扰。
        save_credentials(&conn, b, ident(1, "b@x.com", "b@x.com"), "tok", None, None).unwrap();
        let order: Vec<i64> = list(&conn).unwrap().into_iter().map(|o| o.id).collect();
        assert_eq!(order, vec![c, a, b], "登录不该动用户拖出来的顺序");
    }

    #[test]
    fn login_identifier_is_stored_separately_from_the_account_label() {
        // 这条是这个字段存在的理由：设了昵称的用户，`account_label` 是昵称而不是邮箱
        // —— 拿它去预填登录框就填错了（sub2api 那个框要邮箱格式）。
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
    fn refreshing_identity_updates_label_and_identifier_without_overwriting_account_id() {
        // 用户在运营商那边改了昵称与邮箱：服务端主键不变 ⇒ 仍是同一个账号，
        // 只有展示与预填要跟上。续期路径靠这个函数刷，否则站点选择器一直挂旧标签。
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, a, ident(7, "老名字", "old@x.com"), "tok", None, None).unwrap();

        // 传一个**不同的** account_id（99）：已有值非空 ⇒ 必须不被覆盖。
        refresh_account_identity(&conn, a, ident(99, "新名字", "new@x.com")).unwrap();

        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(op.account_label, "新名字");
        assert_eq!(op.login_identifier, "new@x.com");
        assert_eq!(
            op.account_id,
            Some(7),
            "改邮箱不是换账号，主键不该动 —— 也不该被传进来的新 id 覆盖\
             （`account_id` 有唯一索引，覆写可能撞同站另一行；换账号走 save_credentials）"
        );
        assert_eq!(op.auth_token, "tok", "刷身份不该碰凭据");
    }

    /// ⭐ 「有 token 但 `account_id` 为空」那类行**必须能被补齐**。
    ///
    /// 实测踩到的死局：这种行（早期版本或中途失败的登录留下）原本永久修不好 ——
    /// [`Operator::token_looks_valid`] 对 `token_expires_at = NULL` 返回 `true`
    /// ⇒ `usable_operator` 早退、不走续期，而续期后那次 profile 请求原本是唯一
    /// 拿得到 `account.id` 的地方。用户点任何「刷新」都补不上。
    ///
    /// 后果不止少个字段：`account_id` 为空 ⇒ [`save_credentials`] 的去重查不到它
    /// ⇒ 同一个账号重登会**新建一行**而不是合并，列表里堆重复。
    #[test]
    fn refreshing_identity_backfills_a_missing_account_id() {
        let conn = mem();
        let a = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        // 造出那种半成品行：有 token、没账号身份（`save_site` 不写 account_id，
        // 再单独塞一把 token 进去 —— 与实测到的脏数据同形）。
        update_tokens(&conn, a, "tok", Some("refresh"), None).unwrap();
        assert_eq!(
            get(&conn, a).unwrap().unwrap().account_id,
            None,
            "前提：这行确实没有 account_id"
        );

        refresh_account_identity(&conn, a, ident(42, "名字", "me@x.com")).unwrap();

        let op = get(&conn, a).unwrap().unwrap();
        assert_eq!(
            op.account_id,
            Some(42),
            "为空时必须补上 —— 否则这行永远参与不了去重，重登会堆重复行"
        );
        assert_eq!(op.account_label, "名字");
        assert_eq!(op.login_identifier, "me@x.com");
    }

    #[test]
    fn refreshing_identity_on_a_missing_row_is_an_error() {
        let err = refresh_account_identity(&mem(), 999, ident(1, "n", "e@x.com")).unwrap_err();
        assert!(err.to_string().contains("站点记录不存在"), "{err}");
    }
}
