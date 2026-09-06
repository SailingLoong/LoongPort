//! P4b：熔断跳闸事件 + P5：模型真伪异常事件（站点侧信号的本地事件源）。
//!
//! 两张表都是「proxy_request_logs 里切不出来」的事实：跳闸是熔断器的状态
//! 转换；模型异常是被动验证对响应的判定。所以在发生时各落一行事件，
//! 上传切桶时按小时计数并入桶（跳闸 → 小时桶 breaker_trips，模型异常 →
//! 模型子桶 anomalies）。
//!
//! 跳闸口径：**只记非致命跳闸**。致命跳闸（401/402/凭证级 403 一次即开）是
//! 上传者自己的凭证/余额问题，计入会把它变成站点的公开健康分 —— 与
//! `bucket::SITE_SIDE_ERROR_EXPR` 剔除同类失败是同一条纪律。
//!
//! 模型异常口径：**只记 Anomaly**（异源指纹/自述冒充级，高置信换芯证据）。
//! Suspicious 是弱信号（回写降级生态下 ModelMatch 通过无意义、signature
//! 缺失可能只是没开 thinking），公开面宁缺毋认，不进众测。
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
    let now = chrono::Utc::now().timestamp();
    let hour_epoch = now / 3600 * 3600;
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO crowd_breaker_events (hour_epoch, provider_id, app_type, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![hour_epoch, provider_id, app_type, now],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 落一条模型异常事件（被动验证判定为 Anomaly 时，由消费 worker 调用）。
/// `observed_at` 用响应观察时间而不是入队时间 —— batch 在 channel 里排队
/// 跨过整点时，事件仍归观察到它的那个小时。失败只回 Err 由调用方打日志。
pub fn record_model_anomaly(
    db: &Database,
    provider_id: &str,
    app_type: &str,
    model: &str,
    observed_at: i64,
) -> Result<(), AppError> {
    let hour_epoch = observed_at / 3600 * 3600;
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO crowd_model_anomaly_events (hour_epoch, provider_id, app_type, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![hour_epoch, provider_id, app_type, model, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 裁掉窗口外的旧事件（35 天 = Worker 接受上限 30 天 + 余量），两张事件表
/// 一起裁。挂在 flush 成功后调用 —— 上传节奏天然节流，不需要单独的清理任务。
pub fn prune_old_events(db: &Database, now_epoch: i64) -> Result<u64, AppError> {
    let cutoff = (now_epoch / 3600 * 3600) - 35 * 86400;
    let conn = crate::database::lock_conn!(db.conn);
    let mut n = conn
        .execute(
            "DELETE FROM crowd_breaker_events WHERE hour_epoch < ?1",
            params![cutoff],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    n += conn
        .execute(
            "DELETE FROM crowd_model_anomaly_events WHERE hour_epoch < ?1",
            params![cutoff],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归闸（P5 修根时立）：DDL 的 `created_at NOT NULL` 与 INSERT 列集
    /// 曾经不一致 —— 跳闸事件在真机上每次插入都失败、计数恒 0，桶测试直接
    /// INSERT 全列所以没抓到。两条事件写入必须真的落行。
    #[test]
    fn event_writes_actually_persist() {
        let db = Database::memory().expect("内存库");
        record_breaker_trip(&db, "acct-a", "codex").expect("跳闸事件落库");
        record_model_anomaly(&db, "acct-a", "codex", "test-model", 11 * 3600 + 30)
            .expect("模型异常事件落库");

        let conn = db.conn.lock().unwrap();
        let breaker_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM crowd_breaker_events", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(breaker_count, 1, "跳闸事件必须落行");
        let (hour, model): (i64, String) = conn
            .query_row(
                "SELECT hour_epoch, model FROM crowd_model_anomaly_events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(hour, 11 * 3600, "异常事件归观察到它的小时（非入队时间）");
        assert_eq!(model, "test-model");
    }
}
