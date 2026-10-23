//! 数据库模块 - SQLite 数据持久化
//!
//! 此模块提供应用的核心数据存储功能，包括：
//! - 供应商配置管理
//! - MCP 服务器配置
//! - 提示词管理
//! - Skills 管理
//! - 通用设置存储
//!
//! ## 架构设计
//!
//! ```text
//! database/
//! ├── mod.rs        - Database 结构体 + 初始化
//! ├── schema.rs     - 表结构定义 + Schema 迁移
//! ├── backup.rs     - SQL 导入导出 + 快照备份
//! ├── migration.rs  - JSON → SQLite 数据迁移
//! └── dao/          - 数据访问对象
//!     ├── providers.rs
//!     ├── mcp.rs
//!     ├── prompts.rs
//!     ├── skills.rs
//!     └── settings.rs
//! ```

pub(crate) mod backup;
mod dao;
pub(crate) mod inspection;
/// LoongPort 自己的迁移，与上游 cc-switch 的完全分离（各记各的版本号）。
pub(crate) mod loongport_schema;
mod migration;
mod schema;
pub(crate) mod vault;

#[cfg(test)]
mod tests;

// DAO 类型导出供外部使用
pub use dao::order_profiles;
pub(crate) use dao::providers::delete_provider_row_on;
pub(crate) use dao::providers_seed::{
    is_official_seed_id, official_seed_id, CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
    CODEX_OFFICIAL_PROVIDER_ID, GROKBUILD_OFFICIAL_PROVIDER_ID,
};
pub(crate) use dao::proxy::{
    validate_cost_multiplier, validate_pricing_source, PRICING_SOURCE_REQUEST,
    PRICING_SOURCE_RESPONSE,
};
#[cfg(test)]
pub(crate) use dao::qoder_campaign_operations::{
    create_schema as create_qoder_campaign_operations_schema,
    read_rows as read_qoder_campaign_operation_rows,
};

// Narrow CN operation hooks for the original claim service. Keep the DAO private.
pub(crate) use dao::qoder_campaign_operations::{
    insert_cn_unknown_on as reserve_qoder_cn_operation_on,
    observe_cn_claimed_on as observe_qoder_cn_claimed_on,
    read_cn_on as read_qoder_cn_operations_on,
    reconcile_cn_result_on as reconcile_qoder_cn_operation_on,
    update_cn_result_on as finish_qoder_cn_operation_on, CnOperation, CnTarget,
};

#[cfg(test)]
pub(crate) use dao::qoder_http_auth::{
    create_column as create_qoder_http_auth_column,
    validate_column as validate_qoder_http_auth_column, COLUMN as QODER_HTTP_AUTH_COLUMN,
};

// Narrow schema hooks used by the existing credential inventory. Keep DAO private.
pub(crate) use dao::qoder_http_auth::{
    has_column as has_qoder_http_auth_column,
    INTRODUCED_VERSION as QODER_HTTP_AUTH_INTRODUCED_VERSION,
};
pub use dao::FailoverQueueItem;
pub use dao::Profile;

use crate::config::get_app_config_dir;
use crate::error::AppError;
use rusqlite::{hooks::Action, Connection};
use serde::Serialize;
use std::sync::Mutex;

// DAO 方法通过 impl Database 提供，无需额外导出

/// 当前 Schema 版本号
/// 每次修改表结构时递增，并在 schema.rs 中添加相应的迁移逻辑
/// **上游 cc-switch 的** schema 版本，写在 `PRAGMA user_version` 里。
///
/// ⚠️ **这个数归上游，LoongPort 自己的迁移不许往上加** —— 那是抢占上游号段，
/// 撞上之后是静默缺表而不是报错。我们那套走
/// [`loongport_schema`](self::loongport_schema)，各记各的版本。
///
/// 合并上游时这个值跟着上游走（v17 = 上游的 Pi 会话用量统计迁移）。
pub(crate) const SCHEMA_VERSION: i32 = 20;
/// Fixed source of the approved upgrade, independent of the runtime ceiling.
pub(crate) const UPSTREAM4_SOURCE_SCHEMA_VERSION: i32 = 17;
/// Fixed destination for the approved, explicitly staged upstream upgrade.
pub(crate) const UPSTREAM4_SCHEMA_VERSION: i32 = 20;

/// 安全地序列化 JSON，避免 unwrap panic
pub(crate) fn to_json_string<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|e| AppError::Config(format!("JSON serialization failed: {e}")))
}

/// 安全地获取 Mutex 锁，避免 unwrap panic
macro_rules! lock_conn {
    ($mutex:expr) => {
        $mutex
            .lock()
            .map_err(|e| AppError::Database(format!("Mutex lock failed: {}", e)))?
    };
}

// 导出宏供子模块使用
pub(crate) use lock_conn;

/// 数据库连接封装
///
/// 使用 Mutex 包装 Connection 以支持在多线程环境（如 Tauri State）中共享。
/// rusqlite::Connection 本身不是 Sync 的，因此需要这层包装。
pub struct Database {
    pub(crate) conn: ConnectionMutex,
    pub(crate) pricing_observations:
        std::sync::Arc<crate::services::model_pricing::PricingObservations>,
    pub(crate) secrets: std::sync::Arc<crate::secrets::session::SecretSession>,
}

/// Serializes SQLite access and denies business operations after an interrupted
/// credential transition, including DAOs that do not read encrypted columns.
pub(crate) struct ConnectionMutex {
    inner: Mutex<Connection>,
    secrets: std::sync::Arc<crate::SecretSession>,
}
impl ConnectionMutex {
    #[cfg(any(unix, test))]
    pub(crate) fn paperclip_try_lock(
        &self,
    ) -> Result<Option<std::sync::MutexGuard<'_, Connection>>, AppError> {
        let conn = match self.inner.try_lock() {
            Ok(conn) => conn,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(_) => return Err(AppError::Database("Database lock unavailable".into())),
        };
        self.secrets.ensure_available()?;
        Ok(Some(conn))
    }

    /// Passive diagnostics must not wait behind a business transaction. Keep
    /// the same session denial and returned guard as the original lock owner.
    #[cfg(any(feature = "gui", test))]
    pub(crate) fn try_lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        let conn = self
            .inner
            .try_lock()
            .map_err(|_| AppError::Database("Database lock unavailable".into()))?;
        self.secrets.ensure_available()?;
        Ok(conn)
    }
    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        let conn = self
            .inner
            .lock()
            .map_err(|_| AppError::Database("Database lock unavailable".into()))?;
        self.secrets.ensure_available()?;
        Ok(conn)
    }
}

fn register_db_change_hook(
    conn: &Connection,
    observations: std::sync::Arc<crate::services::model_pricing::PricingObservations>,
    notify_changes: bool,
) -> Result<(), AppError> {
    conn.update_hook(Some(
        move |action: Action, _database: &str, table: &str, _row_id: i64| match action {
            Action::SQLITE_INSERT | Action::SQLITE_UPDATE | Action::SQLITE_DELETE => {
                if table == "model_pricing" || table == "loongport_vault" {
                    // Includes direct SQL edits and edits later rolled back. A
                    // new authoritative fetch is required after any mutation.
                    let _ = observations.invalidate();
                }
                if notify_changes {
                    #[cfg(feature = "gui")]
                    crate::services::webdav_auto_sync::notify_db_changed(table);
                    #[cfg(feature = "gui")]
                    crate::services::s3_auto_sync::notify_db_changed(table);
                }
            }
            _ => {}
        },
    ))
    .map_err(|e| AppError::Database(e.to_string()))
}

/// Prefer WAL so read-only companion processes can read while the app writes.
///
/// SQLite may reject WAL on some filesystems (for example, a read-only or
/// otherwise limited mount). That is a compatibility condition, not a reason
/// to prevent the app from starting, so callers decide whether to warn or fail.
fn configure_wal_mode(conn: &Connection) -> Result<(), AppError> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))
        .map_err(|e| AppError::Database(e.to_string()))?;

    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(AppError::Database(format!(
            "SQLite did not enable WAL journal mode (reported: {mode})"
        )))
    }
}

impl Database {
    pub(crate) fn from_connection(
        conn: Connection,
        secrets: std::sync::Arc<crate::SecretSession>,
    ) -> Self {
        let pricing_observations =
            std::sync::Arc::new(crate::services::model_pricing::PricingObservations::default());
        match register_db_change_hook(&conn, pricing_observations.clone(), false) {
            Ok(()) => pricing_observations
                .hook_registered
                .store(true, std::sync::atomic::Ordering::Release),
            Err(error) => log::warn!("Pricing observation invalidation unavailable: {error}"),
        }
        Self {
            pricing_observations,
            conn: ConnectionMutex {
                inner: Mutex::new(conn),
                secrets: secrets.clone(),
            },
            secrets,
        }
    }

    /// SQLite Backup replaces pages without row update hooks. Restore/import
    /// owners must call this while holding the live connection, before apply.
    pub(crate) fn invalidate_pricing_observations(&self) -> Result<(), AppError> {
        self.pricing_observations.invalidate()
    }

    /// Opaque credential context for adapters sharing this database's lifecycle.
    pub fn secret_session(&self) -> &crate::secrets::session::SecretSession {
        &self.secrets
    }

    /// 初始化数据库连接并创建表
    ///
    /// 数据库文件位于 `~/.cc-switch/cc-switch.db`
    pub fn init() -> Result<Self, AppError> {
        vault::preflight_runtime(&get_app_config_dir().join(crate::config::DB_FILE_NAME))?;
        let secrets = crate::secrets::session::SecretSession::open(
            &get_app_config_dir(),
            &crate::secrets::key_store::SystemKeyStore,
            None,
        )?;
        Self::init_with_secrets(secrets)
    }

    pub fn init_with_secrets(
        secrets: std::sync::Arc<crate::secrets::session::SecretSession>,
    ) -> Result<Self, AppError> {
        let db_path = secrets.root().join(crate::config::DB_FILE_NAME);
        let conn = {
            let context = secrets.read()?;
            vault::prepare(&db_path, &context)?
        };

        if let Err(e) = configure_wal_mode(&conn) {
            log::warn!("Failed to enable SQLite WAL journal mode, continuing with fallback: {e}");
        }

        // 启用外键约束
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        let db = Self::from_connection(conn, secrets);
        {
            let conn = lock_conn!(db.conn);
            register_db_change_hook(&conn, db.pricing_observations.clone(), true)?;
        }
        if let Err(e) = db.ensure_incremental_auto_vacuum() {
            log::warn!("Failed to ensure incremental auto-vacuum: {e}");
        }
        // auto-vacuum 迁移会对有表的库执行一次 VACUUM；Windows 上 VACUUM 以改名
        // 重建数据库文件，会丢掉收紧过的 DACL——这里补一次收紧（幂等）。
        if let Err(e) = crate::config::ensure_private_file(&db_path) {
            log::warn!("Failed to re-restrict database file after auto-vacuum: {e}");
        }
        db.ensure_model_pricing_seeded()?;
        if let Err(e) = crate::services::model_pricing::sync_local_model_pricing(&db) {
            log::warn!("Failed to sync local model pricing file: {e}");
        }

        // Startup cleanup: prune old logs and reclaim space
        if let Err(e) = db.cleanup_old_stream_check_logs(7) {
            log::warn!("Startup stream_check_logs cleanup failed: {e}");
        }
        if let Err(e) = db.rollup_and_prune(30) {
            log::warn!("Startup rollup_and_prune failed: {e}");
        }
        // Reclaim disk space after cleanup
        {
            let conn = lock_conn!(db.conn);
            if let Err(e) = Self::incremental_vacuum_on_conn(&conn) {
                log::warn!("Startup incremental vacuum failed: {e}");
            }
        }

        Ok(db)
    }

    /// 读取磁盘上数据库的 `user_version`；仅当它比应用支持的 [`SCHEMA_VERSION`]
    /// 更新时返回 `Some(version)`。
    ///
    /// 用于初始化失败后判断是否为「数据库版本过新（应用过旧，需升级应用）」的可恢复
    /// 场景——此时不应反复弹出无效的重试对话框，而应引导用户在应用内升级。
    pub fn stored_user_version_exceeds_supported(
        db_path: &std::path::Path,
    ) -> Result<Option<i32>, AppError> {
        let Some(inspected) = inspection::capture(db_path)? else {
            return Ok(None);
        };
        let version = Self::get_user_version(&inspected.image)?;
        inspection::verify_unchanged(db_path, &inspected.revision)?;
        Ok((version > SCHEMA_VERSION).then_some(version))
    }

    /// 创建内存数据库（用于测试）
    pub fn memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory().map_err(|e| AppError::Database(e.to_string()))?;

        // 启用外键约束
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;

        let db = Self::from_connection(conn, crate::secrets::session::SecretSession::ephemeral()?);
        {
            let conn = lock_conn!(db.conn);
            register_db_change_hook(&conn, db.pricing_observations.clone(), true)?;
        }
        db.create_tables()?;
        // 与 `init` / 导入路径一致地把 LoongPort 的版本号 stamp 上。
        //
        // 少了这一行，**全部单测都跑在「表在、版本停在 0」的库上** —— 那是生产里
        // 不存在的形态，等于测试保真度有个缺口：将来加 v1→v2 的迁移时，单测覆盖不到
        // 「已经是最新形态的表又被迁一次」这条真实路径。
        {
            let context = db.secrets.read()?;
            let conn = lock_conn!(db.conn);
            loongport_schema::apply(&conn)?;
            vault::stamp(&conn, &context)?;
        }
        db.ensure_model_pricing_seeded()?;

        Ok(db)
    }

    pub(crate) fn get_auto_vacuum_mode(conn: &Connection) -> Result<i32, AppError> {
        conn.query_row("PRAGMA auto_vacuum;", [], |row| row.get(0))
            .map_err(|e| AppError::Database(format!("读取 auto_vacuum 失败: {e}")))
    }

    /// 回收全部空闲页。`PRAGMA incremental_vacuum` 每释放一页产出一行结果，
    /// 必须把结果读完才会回收完；`execute_batch` 只 step 一次，只能回收 1 页。
    pub(crate) fn incremental_vacuum_on_conn(conn: &Connection) -> Result<(), AppError> {
        let mut stmt = conn
            .prepare("PRAGMA incremental_vacuum;")
            .map_err(|e| AppError::Database(format!("执行 incremental_vacuum 失败: {e}")))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| AppError::Database(format!("执行 incremental_vacuum 失败: {e}")))?;
        while rows
            .next()
            .map_err(|e| AppError::Database(format!("执行 incremental_vacuum 失败: {e}")))?
            .is_some()
        {}
        Ok(())
    }

    fn has_user_tables(conn: &Connection) -> Result<bool, AppError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("读取表数量失败: {e}")))?;
        Ok(count > 0)
    }

    pub(crate) fn ensure_incremental_auto_vacuum_on_conn(
        conn: &Connection,
    ) -> Result<bool, AppError> {
        let mode = Self::get_auto_vacuum_mode(conn)?;
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = Self::has_user_tables(conn)?;
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(format!("设置 auto_vacuum 失败: {e}")))?;

        if !has_tables {
            return Ok(false);
        }

        conn.execute("VACUUM;", [])
            .map_err(|e| AppError::Database(format!("执行 VACUUM 失败: {e}")))?;
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(format!("恢复 foreign_keys 失败: {e}")))?;
        Ok(true)
    }

    pub(crate) fn ensure_incremental_auto_vacuum(&self) -> Result<bool, AppError> {
        let mode = {
            let conn = lock_conn!(self.conn);
            Self::get_auto_vacuum_mode(&conn)?
        };
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = {
            let conn = lock_conn!(self.conn);
            Self::has_user_tables(&conn)?
        };
        if has_tables {
            log::info!(
                "Detected auto_vacuum={mode}, rebuilding database to enable incremental vacuum"
            );
            self.backup_database_file()?;
        }

        let rebuilt = {
            let conn = lock_conn!(self.conn);
            Self::ensure_incremental_auto_vacuum_on_conn(&conn)?
        };

        if rebuilt {
            log::info!("Incremental auto-vacuum enabled after database rebuild");
        } else {
            log::info!("Incremental auto-vacuum configured for new database");
        }

        Ok(rebuilt)
    }

    /// 检查 MCP 服务器表是否为空
    pub fn is_mcp_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }

    /// 检查提示词表是否为空
    pub fn is_prompts_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }
}

#[cfg(test)]
mod diagnostic_connection_tests {
    use super::*;
    fn fixture() -> ConnectionMutex {
        ConnectionMutex {
            inner: Mutex::new(Connection::open_in_memory().unwrap()),
            secrets: crate::SecretSession::ephemeral().unwrap(),
        }
    }
    #[test]
    fn diagnostic_try_lock_uses_real_available_connection() {
        let owner = fixture();
        assert_eq!(
            owner
                .try_lock()
                .unwrap()
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn diagnostic_try_lock_does_not_wait_for_busy_connection() {
        let owner = fixture();
        let held = owner.lock().unwrap();
        assert!(owner.try_lock().is_err());
        drop(held);
        assert!(owner.try_lock().is_ok());
    }
    #[test]
    fn diagnostic_try_lock_preserves_blocked_session_denial() {
        let owner = fixture();
        owner.secrets.set_blocked(true);
        assert!(owner.lock().is_err());
        assert!(owner.try_lock().is_err());
        owner.secrets.set_blocked(false);
        assert!(owner.try_lock().is_ok());
    }
    #[test]
    fn diagnostic_try_lock_fails_closed_on_poison() {
        let owner = fixture();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = owner.inner.lock().unwrap();
            panic!("synthetic lock poisoning");
        }));
        assert!(owner.try_lock().is_err());
    }
}
