pub mod config;
#[cfg(feature = "gui")]
pub mod scheduler;

#[cfg(feature = "gui")]
use tauri::{Emitter, Manager};

pub use config::{APP_UPDATE_CHECKED_EVENT, MODELS_DEV_PRICING_UPDATED_EVENT};

#[cfg(feature = "gui")]
pub fn start(app: tauri::AppHandle) {
    start_qoder_claim(app.clone());
    start_zcode_claim(app.clone());
    start_minimax_automatic_claim(app.clone());
    start_trae_automatic_checkin(app.clone());
    start_workbuddy_automatic_claim(app.clone());
    start_directory_refresh(app.clone());
    start_models_dev_pricing_refresh(app.clone());
    start_relay_pricing_refresh(app.clone());
    start_crowd_metrics_flush(app.clone());
    start_plaza_visibility_seeding(app.clone());
    start_app_update_check(app);
}

#[cfg(feature = "gui")]
fn start_qoder_claim(app: tauri::AppHandle) {
    use crate::services::provider::qoder_daily::automatic;
    let db = app.state::<crate::AppState>().db.clone();
    scheduler::spawn_periodic(
        "qoder-automatic-claim",
        scheduler::TaskSchedule::new(
            automatic::STARTUP_DELAY,
            automatic::INTERVAL,
            automatic::INTERVAL,
        ),
        move || {
            let db = db.clone();
            async move {
                tauri::async_runtime::spawn_blocking(move || automatic::tick(&db))
                    .await
                    .map_err(|_| crate::AppError::Config("qoder.automatic_task_failed".into()))?
            }
        },
    );
}

#[cfg(feature = "gui")]
/// 广场开关的存量补播种：**一次性**（写-if-None），启动即跑。
///
/// 触发归属（AGENTS.md §1.4）：播种是数据层自己的初始化动作，挂在启动上；
/// 广场页与设置页只读 `plaza_visible` 的现成产出，不驱动它。首启弹窗那条
/// 播种在用户提交动作里（`relay::plaza::seed_from_first_site`），与本任务
/// 在同一设置写锁内裁决，显式首域与用户开关优先。
fn start_plaza_visibility_seeding(app: tauri::AppHandle) {
    let db = app.state::<crate::AppState>().db.clone();
    tauri::async_runtime::spawn(async move {
        // 读库是阻塞操作，单独进 blocking；播种本身要先刷一次远端配置（见
        // plaza::seed_for_existing_install 的文档），那是 await。
        let origins = match tauri::async_runtime::spawn_blocking(move || {
            let vault = db.secrets.read()?;
            let relays = db
                .conn
                .lock()
                .map_err(|e| crate::error::AppError::Database(format!("获取数据库连接失败: {e}")))
                .and_then(|conn| crate::relay::creds::list(&conn, &vault));
            relays.map(|relays| {
                relays
                    .iter()
                    .map(|relay| relay.site_origin.clone())
                    .collect::<Vec<_>>()
            })
        })
        .await
        {
            Ok(Ok(origins)) => origins,
            Ok(Err(e)) => {
                log::warn!("广场开关补播种：读站点列表失败，保持默认: {e}");
                return;
            }
            Err(e) => {
                log::warn!("广场开关补播种：读站点任务失败，保持默认: {e}");
                return;
            }
        };
        crate::relay::plaza::seed_for_existing_install(&origins).await;
    });
}

#[cfg(feature = "gui")]
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

#[cfg(feature = "gui")]
/// 站点实测共建：每 15 分钟 flush 已闭合的小时桶。
///
/// 门禁（开关 + 看过告知，见 `crowd::uploader::upload_allowed`）在 `flush_once`
/// 里读 —— 不满足时任务是纯空转，不需要在注册层再做一次判断（两处判断迟早分叉）。
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

#[cfg(feature = "gui")]
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

#[cfg(feature = "gui")]
fn start_directory_refresh(app: tauri::AppHandle) {
    let schedule = scheduler::TaskSchedule::new(
        config::DIRECTORY_STARTUP_DELAY,
        config::DIRECTORY_REFRESH_INTERVAL,
        config::DIRECTORY_RETRY_DELAY,
    );
    scheduler::spawn_periodic("directory", schedule, move || {
        let app = app.clone();
        async move {
            if let Some(config) = crate::relay::remote_config::refresh_and_cache()
                .await
                .or_else(crate::relay::remote_config::load_cached)
            {
                crate::relay::plaza::resolve_pending(&config)?;
            }
            // 实测快照（行级观测的 owner）：数据层自己的周期触发；
            // 失败返回 Err → 走 RETRY_DELAY 重试，读取路径只消费缓存。
            crate::crowd::snapshot::refresh_and_cache().await?;
            // 探针落盘 + 逐站日志：best-effort，不把它记成任务失败 ——
            // 探针的单站失败已经作为 NetworkBlocked 落进了结果里。
            crate::relay::directory::refresh_site_probes_for_directory().await;
            // transit 摘要（站方公开数据）与实测快照同一周期刷，
            // 与手动刷新按钮共用同一条「刷完广播」路径。
            crate::spawn_transit_refresh_and_emit(app.clone());
            Ok(())
        }
    });
}

#[cfg(feature = "gui")]
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

#[cfg(feature = "gui")]
fn start_zcode_claim(app: tauri::AppHandle) {
    let interval = crate::zcode_accounts::claim_runtime::INTERVAL;
    let db = app.state::<crate::AppState>().db.clone();
    scheduler::spawn_periodic(
        "zcode-claim",
        scheduler::TaskSchedule::new(interval, interval, interval),
        move || {
            let app = app.clone();
            let db = db.clone();
            async move {
                crate::zcode_accounts::claim_runtime::tick(app, db).await;
                Ok(())
            }
        },
    );
}

#[cfg(feature = "gui")]
fn start_trae_automatic_checkin(app: tauri::AppHandle) {
    use std::time::Duration;
    let db = app.state::<crate::AppState>().db.clone();
    scheduler::spawn_periodic(
        "trae-automatic-checkin",
        scheduler::TaskSchedule::new(
            Duration::from_secs(30),
            Duration::from_secs(15 * 60),
            Duration::from_secs(15 * 60),
        ),
        move || {
            let db = db.clone();
            async move {
                // The existing account owner enforces current per-account consent.
                let _ = crate::trae_accounts::checkin::automatic_tick(db.secrets.clone()).await;
                Ok(())
            }
        },
    );
}

#[cfg(feature = "gui")]
pub(crate) fn wake_automatic_claim(provider: scheduler::AutomaticProvider) {
    scheduler::automatic_registry().wake(provider);
}

#[cfg(feature = "gui")]
fn start_automatic_claim_task<F, Fut>(
    app: tauri::AppHandle,
    provider: scheduler::AutomaticProvider,
    task: F,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = scheduler::AutomaticOutcome> + Send + 'static,
{
    let Some(control) = scheduler::automatic_registry().register(provider) else {
        return;
    };
    let schedule = scheduler::TaskSchedule::new(
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(15 * 60),
        std::time::Duration::from_secs(15 * 60),
    );
    tauri::async_runtime::spawn(scheduler::run_wakeable(
        control,
        schedule,
        task,
        move |status| {
            if app
                .emit(crate::events::AUTOMATIC_CLAIM_STATUS_UPDATED, status)
                .is_err()
            {
                log::warn!("Could not publish automatic account check status");
            }
        },
    ));
}

#[cfg(feature = "gui")]
fn start_workbuddy_automatic_claim(app: tauri::AppHandle) {
    use scheduler::{AutomaticOutcome as Outcome, AutomaticProvider, AutomaticReason as Reason};
    let db = app.state::<crate::AppState>().db.clone();
    start_automatic_claim_task(app, AutomaticProvider::Workbuddy, move || {
        let db = db.clone();
        async move {
            if db.secret_session().read().is_err() {
                return Outcome::skipped(Reason::Locked);
            }
            let engine = crate::workbuddy::engine::Engine {
                session: db.secret_session(),
                transport: &crate::workbuddy::http::OfficialHttp,
                clock: &crate::workbuddy::clock::SystemClock,
            };
            let rows = match engine.list().await {
                Ok(rows) => rows,
                Err(_) => return Outcome::failed(),
            };
            let enabled: Vec<_> = rows.iter().filter(|row| row.auto_claim_enabled).collect();
            if enabled.is_empty() {
                return Outcome::skipped(Reason::Disabled);
            }
            if !enabled
                .iter()
                .any(|row| row.can_set_automatic && !row.auto_claim_blocked && row.can_claim)
            {
                return Outcome::skipped(if enabled.iter().any(|row| row.auto_claim_blocked) {
                    Reason::Pending
                } else {
                    Reason::NotEligible
                });
            }
            match engine.automatic_tick().await {
                Ok(0) => Outcome::completed(),
                Ok(_) | Err(_) => Outcome::failed(),
            }
        }
    });
}

#[cfg(feature = "gui")]
fn start_minimax_automatic_claim(app: tauri::AppHandle) {
    use scheduler::{AutomaticOutcome as Outcome, AutomaticProvider, AutomaticReason as Reason};
    let db = app.state::<crate::AppState>().db.clone();
    start_automatic_claim_task(app, AutomaticProvider::MinimaxCode, move || {
        let db = db.clone();
        async move {
            if db.secret_session().read().is_err() {
                return Outcome::skipped(Reason::Locked);
            }
            let engine = crate::minimax_code::checkin::Engine {
                session: db.secret_session(),
                transport: &crate::minimax_code::http::OfficialHttp,
            };
            let rows = match engine.list().await {
                Ok(rows) => rows,
                Err(_) => return Outcome::failed(),
            };
            let enabled: Vec<_> = rows.iter().filter(|row| row.auto_claim_enabled).collect();
            if enabled.is_empty() {
                return Outcome::skipped(Reason::Disabled);
            }
            if !enabled.iter().any(|row| row.authorized) {
                return Outcome::skipped(Reason::NotEligible);
            }
            // The original owner still queries unresolved attempts but never replays them.
            match engine.automatic_tick().await {
                Ok(0) => {}
                Ok(_) | Err(_) => return Outcome::failed(),
            }
            match engine.list().await {
                Ok(rows)
                    if rows.iter().any(|row| {
                        row.auto_claim_enabled
                            && row.authorized
                            && row.status == crate::minimax_code::catalog::Status::Unknown
                    }) =>
                {
                    Outcome::skipped(Reason::Pending)
                }
                Ok(_) => Outcome::completed(),
                Err(_) => Outcome::failed(),
            }
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
