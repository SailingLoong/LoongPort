//! P4b：熔断跳闸事件（站点侧信号的本地事件源）。
//!
//! `proxy_request_logs` 里切不出「跳闸」—— 它是熔断器的状态转换，不是一行
//! 请求日志。所以在跳闸发生时（`provider_router::record_result` 的非致命
//! 分支）落一行事件，上传切桶时按 (hour, provider, app) 计数并入小时桶。
//!
//! 口径：**只记非致命跳闸**。致命跳闸（401/402/凭证级 403 一次即开）是
//! 上传者自己的凭证/余额问题，计入会把它变成站点的公开健康分 —— 与
//! `bucket::SITE_SIDE_ERROR_EXPR` 剔除同类失败是同一条纪律。
//!
//! 表是纯追加的事件流：上传幂等靠「按小时重算计数」而不是删行（行本身
//! 不可变，重算结果稳定）；清理走 flush 时顺手裁掉 35 天前的旧行。

use rusqlite::params;

use crate::database::Database;
use crate::error::AppError;

/// 落一条跳闸事件。调用点在转发主链路里 —— 失败只回 Err 由调用方打日志，
/// 绝不向上传播影响请求本身。
pub fn record_breaker_trip(
    db: &Database,
    provider_id: &str,
    app_type: &str,
) -> Result<(), AppError> {
    let hour_epoch = chrono::Utc::now().timestamp() / 3600 * 3600;
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO crowd_breaker_events (hour_epoch, provider_id, app_type) VALUES (?1, ?2, ?3)",
        params![hour_epoch, provider_id, app_type],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 裁掉窗口外的旧事件（35 天 = Worker 接受上限 30 天 + 余量）。
/// 挂在 flush 成功后调用 —— 上传节奏天然节流，不需要单独的清理任务。
pub fn prune_old_events(db: &Database, now_epoch: i64) -> Result<u64, AppError> {
    let cutoff = (now_epoch / 3600 * 3600) - 35 * 86400;
    let conn = crate::database::lock_conn!(db.conn);
    let n = conn
        .execute(
            "DELETE FROM crowd_breaker_events WHERE hour_epoch < ?1",
            params![cutoff],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(n as u64)
}
