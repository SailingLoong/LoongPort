pub mod config;
pub mod scheduler;

use tauri::{Emitter, Manager};

pub use config::{APP_UPDATE_CHECKED_EVENT, MODELS_DEV_PRICING_UPDATED_EVENT};

pub fn start(app: tauri::AppHandle) {
    start_veridrop_directory_refresh(app.clone());
    start_models_dev_pricing_refresh(app.clone());
    start_relay_pricing_refresh(app.clone());
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
            crate::refresh_stale_directories(app).await
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
