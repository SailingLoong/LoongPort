pub mod config;
pub mod scheduler;

use tauri::{Emitter, Manager};

pub use config::MODELS_DEV_PRICING_UPDATED_EVENT;

pub fn start(app: tauri::AppHandle) {
    start_veridrop_directory_refresh(app.clone());
    start_models_dev_pricing_refresh(app);
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
    use super::MODELS_DEV_PRICING_UPDATED_EVENT;

    #[test]
    fn models_dev_event_matches_frontend_constant() {
        let frontend = include_str!("../../../src/config/constants.ts");

        assert!(frontend.contains(MODELS_DEV_PRICING_UPDATED_EVENT));
    }
}
