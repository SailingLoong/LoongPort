//! 运营商凭据与本机标识的持久化。
//!
//! 一张表 `loongport_operator`（单行，V2 只支持一家运营商在用）：
//!
//! | 列 | 含义 |
//! |---|---|
//! | `id` | 恒为 1（`CHECK` 约束保证单行） |
//! | `site_origin` | 面板 origin，如 `https://bestapi.store` |
//! | `site_name` | 展示名，来自探测结果 |
//! | `api_base_url` | 归一后的 codex `base_url`（带 `/v1`） |
//! | `device_id` | 本机 UUID v4，用于 Key 命名 |
//! | `auth_token` / `refresh_token` / `token_expires_at` | 登录凭据 |
//!
//! ## 为什么 device_id 和凭据同表
//!
//! V1 分了三张表（`loongport_device` / `loongport_credential` / `loongport_operator`），
//! 因为它要支持多运营商 + 多设备同步过滤。V2 单运营商、不做云同步，三张表的行都是 1:1，
//! 合成一张是消除 join 而不是牺牲边界。
//!
//! ## 凭据存在 SQLite 明文里，没进 keyring
//!
//! V1 用了 `keyring` crate（三平台原生后端）。V2 第一版不引它，理由是**同一个库里已经躺着
//! 明文 API Key**：cc-switch 的 `providers.settings_config` 存的就是明文 sk（上游行为），
//! 把 token 加密而 sk 不加密没有实际收益。这是**知情引入的技术债**，偿还条件见项目
//! `TODO.md`：要么两者一起进 keyring，要么都不进。

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

/// 一家运营商的完整状态（含凭据）。
#[derive(Debug, Clone)]
pub struct Operator {
    pub site_origin: String,
    pub site_name: String,
    /// codex 用的 API 基址，已归一到带 `/v1`。
    pub api_base_url: String,
    pub device_id: String,
    pub auth_token: String,
    pub refresh_token: Option<String>,
    /// Unix 秒。`None` 表示服务端没给（降级态：可用但不可续期）。
    pub token_expires_at: Option<i64>,
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
}

/// 建表（v16→v17 迁移与全新库都走它）。
pub fn create_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS loongport_operator (
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
    .map_err(|e| AppError::Database(format!("创建 loongport_operator 表失败: {e}")))?;
    Ok(())
}

/// 读当前运营商（未配置返回 `None`）。
pub fn load(conn: &Connection) -> Result<Option<Operator>, AppError> {
    conn.query_row(
        "SELECT site_origin, site_name, api_base_url, device_id,
                auth_token, refresh_token, token_expires_at
         FROM loongport_operator WHERE id = 1",
        [],
        |row| {
            Ok(Operator {
                site_origin: row.get(0)?,
                site_name: row.get(1)?,
                api_base_url: row.get(2)?,
                device_id: row.get(3)?,
                auth_token: row.get(4)?,
                refresh_token: row.get(5)?,
                token_expires_at: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Database(format!("读取运营商失败: {e}")))
}

/// 写入运营商站点信息（不碰凭据）。首次调用时生成 `device_id`。
///
/// 用 `INSERT ... ON CONFLICT` 而不是先查再插：并发下先查再插会插两行（虽然 V2 是单线程
/// 调用，但 `CHECK (id = 1)` 之外再多一层保证不花钱）。
pub fn save_site(
    conn: &Connection,
    site_origin: &str,
    site_name: &str,
    api_base_url: &str,
) -> Result<String, AppError> {
    let device_id = match load(conn)? {
        // device_id 一旦生成就不再变：它进了服务端的 Key 名字，换掉就认领不回自己的 Key。
        Some(op) if !op.device_id.is_empty() => op.device_id,
        _ => uuid::Uuid::new_v4().to_string(),
    };
    let now = now_unix();

    conn.execute(
        "INSERT INTO loongport_operator
            (id, site_origin, site_name, api_base_url, device_id, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            site_origin = excluded.site_origin,
            site_name = excluded.site_name,
            api_base_url = excluded.api_base_url,
            updated_at = excluded.updated_at",
        params![site_origin, site_name, api_base_url, &device_id, now],
    )
    .map_err(|e| AppError::Database(format!("保存运营商失败: {e}")))?;

    Ok(device_id)
}

/// 写入登录凭据。站点必须已存在（登录总是发生在选站之后）。
pub fn save_credentials(
    conn: &Connection,
    auth_token: &str,
    refresh_token: Option<&str>,
    token_expires_at: Option<i64>,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE loongport_operator
             SET auth_token = ?1, refresh_token = ?2, token_expires_at = ?3, updated_at = ?4
             WHERE id = 1",
            params![auth_token, refresh_token, token_expires_at, now_unix()],
        )
        .map_err(|e| AppError::Database(format!("保存凭据失败: {e}")))?;

    if changed == 0 {
        return Err(AppError::Config(
            "保存凭据失败: 还没有选择运营商站点".into(),
        ));
    }
    Ok(())
}

/// 清掉凭据但保留站点与 device_id（登出 / 凭据失效后重登用）。
///
/// **device_id 必须留着**：它进了服务端的 Key 名字，清掉会让重登后认领不到自己已建的 Key，
/// 于是给用户账号里堆一批同分组的重复 sk。
pub fn clear_credentials(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "UPDATE loongport_operator
         SET auth_token = '', refresh_token = NULL, token_expires_at = NULL, updated_at = ?1
         WHERE id = 1",
        params![now_unix()],
    )
    .map_err(|e| AppError::Database(format!("清除凭据失败: {e}")))?;
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

    #[test]
    fn load_returns_none_before_any_site_saved() {
        assert!(load(&mem()).unwrap().is_none());
    }

    #[test]
    fn save_site_generates_device_id_once_and_keeps_it() {
        let conn = mem();
        let first = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        assert!(!first.is_empty());

        // 换站也不换 device_id —— 它进了服务端的 Key 名字，换掉就认领不回已建的 Key。
        let second = save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        assert_eq!(first, second);

        let op = load(&conn).unwrap().unwrap();
        assert_eq!(op.site_origin, "https://b.dev");
        assert_eq!(op.device_id, first);
    }

    #[test]
    fn save_site_stays_single_row() {
        let conn = mem();
        save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_site(&conn, "https://b.dev", "B", "https://b.dev/v1").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM loongport_operator", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn credentials_roundtrip() {
        let conn = mem();
        save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, "tok", Some("ref"), Some(1_800_000_000)).unwrap();

        let op = load(&conn).unwrap().unwrap();
        assert_eq!(op.auth_token, "tok");
        assert_eq!(op.refresh_token.as_deref(), Some("ref"));
        assert_eq!(op.token_expires_at, Some(1_800_000_000));
    }

    #[test]
    fn save_credentials_without_site_is_a_visible_error() {
        // 静默成功会让「登录成功但什么都没存下」变成一个查不出来的问题。
        let err = save_credentials(&mem(), "tok", None, None).unwrap_err();
        assert!(err.to_string().contains("还没有选择运营商站点"));
    }

    #[test]
    fn clear_credentials_preserves_site_and_device_id() {
        let conn = mem();
        let device = save_site(&conn, "https://a.dev", "A", "https://a.dev/v1").unwrap();
        save_credentials(&conn, "tok", Some("ref"), Some(123)).unwrap();
        clear_credentials(&conn).unwrap();

        let op = load(&conn).unwrap().unwrap();
        assert_eq!(op.auth_token, "");
        assert!(op.refresh_token.is_none());
        assert!(op.token_expires_at.is_none());
        // 这两条是本测试的重点，不是顺带断言。
        assert_eq!(op.site_origin, "https://a.dev");
        assert_eq!(op.device_id, device);
    }

    #[test]
    fn token_validity_leaves_a_margin_and_tolerates_missing_expiry() {
        let base = Operator {
            site_origin: "https://a.dev".into(),
            site_name: "A".into(),
            api_base_url: "https://a.dev/v1".into(),
            device_id: "d".into(),
            auth_token: "tok".into(),
            refresh_token: None,
            token_expires_at: Some(1000),
        };
        assert!(base.token_looks_valid(0));
        // 60 秒余量内算已过期：卡边界发请求只会拿到 401，白跑一趟。
        assert!(!base.token_looks_valid(950));
        assert!(!base.token_looks_valid(1000));

        // 服务端降级态（没给 expiry）不能判成「未就位」去轮询等——那会永远等不到。
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
}
