pub mod config;
pub mod scheduler;

use tauri::{Emitter, Manager};

pub use config::{APP_UPDATE_CHECKED_EVENT, MODELS_DEV_PRICING_UPDATED_EVENT};

pub fn start(app: tauri::AppHandle) {
    start_veridrop_directory_refresh(app.clone());
    start_models_dev_pricing_refresh(app.clone());
    start_relay_pricing_refresh(app.clone());
    start_crowd_metrics_flush(app.clone());
    start_app_update_check(app);
}

fn start_relay_pricing_refresh(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        config::RELAY_PRICING_STARTUP_DELAY,
        config::RELAY_PRICING_REFRESH_INTERVAL,
        config::RELAY_PRICING_RETRY_DELAY,
    );
    scheduler::spawn_periodic("relay-pricing", schedule, move || {
        crate::refresh_due_relay_pricing(app.clone())
    });
}

/// 站点实测共建：每 15 分钟 flush 已闭合的小时桶。
///
/// 门禁（`crowd_metrics_enabled`）在 `flush_once` 里读 —— 关着时任务是纯空转，
/// 不需要在注册层再做一次判断（两处判断迟早分叉）。
fn start_crowd_metrics_flush(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        config::CROWD_METRICS_STARTUP_DELAY,
        config::CROWD_METRICS_FLUSH_INTERVAL,
        config::CROWD_METRICS_RETRY_DELAY,
    );
    let db = app.state::<crate::AppState>().db.clone();
    scheduler::spawn_periodic("crowd-metrics-flush", schedule, move || {
        let db = db.clone();
        async move { crate::crowd::uploader::flush_once(&db).await }
    });
}

fn start_app_update_check(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        config::UPDATE_CHECK_STARTUP_DELAY,
        config::UPDATE_CHECK_INTERVAL,
        config::UPDATE_CHECK_INTERVAL,
    );

    scheduler::spawn_periodic("app-update", schedule, move || {
        let app = app.clone();
        async move {
            let result = crate::services::app_update::check(&app).await?;
            app.emit(APP_UPDATE_CHECKED_EVENT, &result)
                .map_err(|error| {
                    crate::AppError::Message(format!(
                        "failed to emit {APP_UPDATE_CHECKED_EVENT}: {error}"
                    ))
                })?;
            Ok(())
        }
    });
}

fn start_veridrop_directory_refresh(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        config::VERIDROP_STARTUP_DELAY,
        config::VERIDROP_REFRESH_INTERVAL,
        config::VERIDROP_RETRY_DELAY,
    );
    scheduler::spawn_periodic("veridrop-directory", schedule, move || {
        let app = app.clone();
        async move {
            crate::relay::remote_config::refresh_and_cache().await;
            crate::refresh_stale_directories(app.clone()).await?;
            // 漏斗收尾（探针 + 三层日志）：best-effort，不把它记成任务失败 ——
            // 探针的单站失败已经作为 NetworkBlocked 落进了结果里。
            crate::relay::leaderboard::refresh_site_probes_for_directory().await;
            // transit 摘要与榜单同一周期刷（都是 6 小时口径的站方数据），
            // 与手动刷新按钮共用同一条「刷完广播」路径。
            crate::spawn_transit_refresh_and_emit(app.clone());
            Ok(())
        }
    });
}

fn start_models_dev_pricing_refresh(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        std::time::Duration::ZERO,
        config::MODELS_DEV_REFRESH_INTERVAL,
        config::MODELS_DEV_REFRESH_INTERVAL,
    );
    let db = app.state::<crate::AppState>().db.clone();

    scheduler::spawn_periodic("models-dev-pricing", schedule, move || {
        let app = app.clone();
        let db = db.clone();
        async move {
            let result = crate::services::models_dev::sync_pricing(db, false).await?;
            if !result.skipped {
                app.emit(MODELS_DEV_PRICING_UPDATED_EVENT, &result)
                    .map_err(|error| {
                        crate::AppError::Message(format!(
                            "failed to emit {MODELS_DEV_PRICING_UPDATED_EVENT}: {error}"
                        ))
                    })?;
            }
            Ok(())
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{config, APP_UPDATE_CHECKED_EVENT, MODELS_DEV_PRICING_UPDATED_EVENT};

    #[test]
    fn app_update_event_matches_frontend_constant() {
        let frontend = include_str!("../../../src/config/constants.ts");

        assert!(frontend.contains(APP_UPDATE_CHECKED_EVENT));
    }

    #[test]
    fn models_dev_event_matches_frontend_constant() {
        let frontend = include_str!("../../../src/config/constants.ts");

        assert!(frontend.contains(MODELS_DEV_PRICING_UPDATED_EVENT));
    }

    #[test]
    fn relay_pricing_uses_its_own_six_hour_interval() {
        assert_eq!(
            config::RELAY_PRICING_REFRESH_INTERVAL,
            std::time::Duration::from_secs(6 * 60 * 60)
        );
    }
}
