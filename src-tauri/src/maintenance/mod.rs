pub mod config;
pub mod scheduler;

pub fn start(app: tauri::AppHandle) {
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
