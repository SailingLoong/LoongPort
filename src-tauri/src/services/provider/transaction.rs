//! Reversible writes for exclusive provider configuration, shared by edit and selection.
use crate::{app_config::AppType, error::AppError, provider::Provider, store::AppState};

/// Caller holds the application switch lock throughout capture, operation and rollback.
pub(crate) fn with_provider_config_transaction<T>(
    state: &AppState,
    app: &AppType,
    provider: &Provider,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let previous_model = crate::proxy::auto_strategy::get_model_pref(&state.db, app.as_str());
    let previous_local = crate::settings::get_current_provider(app);
    let previous_current = state.db.get_current_provider(app.as_str())?;
    let outgoing = previous_local
        .as_ref()
        .or(previous_current.as_ref())
        .filter(|id| *id != &provider.id)
        .map(|id| state.db.get_provider_by_id(id, app.as_str()))
        .transpose()?
        .flatten();
    let snippet = state.db.get_config_snippet(app.as_str())?;
    let backup = futures::executor::block_on(state.db.get_live_backup(app.as_str()))?;
    let live = ProviderLiveSnapshot::capture(app)?;
    let result = operation();
    match result {
        Ok(result) => Ok(result),
        Err(error) => {
            let mut failures = Vec::new();
            let mut restore = |result: Result<(), AppError>| {
                if let Err(error) = result {
                    failures.push(error.to_string());
                }
            };
            restore(state.db.save_provider(app.as_str(), provider));
            if let Some(outgoing) = outgoing {
                restore(state.db.update_provider_settings_config(
                    app.as_str(),
                    &outgoing.id,
                    &outgoing.settings_config,
                ));
            }
            restore(crate::proxy::auto_strategy::set_model_pref(
                &state.db,
                app.as_str(),
                previous_model.as_deref(),
            ));
            restore(crate::settings::set_current_provider(
                app,
                previous_local.as_deref(),
            ));
            restore((|| {
                let conn = crate::database::lock_conn!(state.db.conn);
                conn.execute("UPDATE providers SET is_current = CASE WHEN id = ?2 THEN 1 ELSE 0 END WHERE app_type = ?1",
                    rusqlite::params![app.as_str(), previous_current])?;
                Ok(())
            })());
            restore(futures::executor::block_on(async {
                match backup {
                    Some(backup) => {
                        state
                            .db
                            .save_live_backup(app.as_str(), &backup.original_config)
                            .await
                    }
                    None => state.db.delete_live_backup(app.as_str()).await,
                }
            }));
            restore(state.db.set_config_snippet(app.as_str(), snippet));
            restore(live.restore());
            futures::executor::block_on(
                state
                    .proxy_service
                    .refresh_active_target_from_current_provider(app),
            );
            if failures.is_empty() {
                Err(error)
            } else {
                Err(AppError::Config(format!(
                    "{error}; rollback failed: {}",
                    failures.join("; ")
                )))
            }
        }
    }
}

/// Exact native configuration rollback for the applications with exclusive routing.
/// Codex retains its auth-generation-aware rollback contract.
enum ProviderLiveSnapshot {
    Codex(crate::codex_config::CodexLiveStateSnapshot),
    Files(Vec<(std::path::PathBuf, Option<Vec<u8>>)>),
}

impl ProviderLiveSnapshot {
    fn capture(app: &AppType) -> Result<Self, AppError> {
        let paths = match app {
            AppType::Codex => {
                return crate::codex_config::CodexLiveStateSnapshot::capture().map(Self::Codex)
            }
            AppType::Claude => vec![crate::config::get_claude_settings_path()],
            AppType::Gemini => vec![
                crate::gemini_config::get_gemini_env_path(),
                crate::gemini_config::get_gemini_settings_path(),
            ],
            AppType::GrokBuild => vec![crate::grok_config::get_grok_config_path()],
            _ => {
                return Err(AppError::Config(
                    "Application does not support routing selection".into(),
                ))
            }
        };
        let files = paths
            .into_iter()
            .map(|path| {
                let contents = match std::fs::read(&path) {
                    Ok(contents) => Some(contents),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(AppError::io(&path, error)),
                };
                Ok((path, contents))
            })
            .collect::<Result<_, AppError>>()?;
        Ok(Self::Files(files))
    }

    fn restore(&self) -> Result<(), AppError> {
        match self {
            Self::Codex(snapshot) => snapshot.restore_preserving_newer_same_account_auth(),
            Self::Files(files) => {
                let failures: Vec<String> = files
                    .iter()
                    .filter_map(|(path, contents)| {
                        match contents {
                            Some(contents) => crate::config::atomic_write_private(path, contents),
                            None => crate::config::delete_file(path),
                        }
                        .err()
                        .map(|error| error.to_string())
                    })
                    .collect();
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(AppError::Config(failures.join("; ")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ProviderService;
    use std::{ffi::OsString, sync::Arc, time::Duration};

    struct TempHome {
        dir: tempfile::TempDir,
        previous: Option<OsString>,
    }

    impl TempHome {
        fn new() -> Self {
            let home = Self {
                dir: tempfile::tempdir().expect("create isolated home"),
                previous: std::env::var_os("CC_SWITCH_TEST_HOME"),
            };
            std::env::set_var("CC_SWITCH_TEST_HOME", home.dir.path());
            crate::settings::reload_settings().expect("reload isolated settings");
            home
        }

        fn state(&self) -> Arc<AppState> {
            let db = Arc::new(
                crate::secrets::testing::initialize_database().expect("initialize isolated vault"),
            );
            crate::settings::mutate_settings(|settings| {
                settings.claude_config_dir = Some(
                    self.dir
                        .path()
                        .join("claude")
                        .to_string_lossy()
                        .into_owned(),
                );
            })
            .expect("isolate Claude native and plugin paths");
            assert!(crate::config::get_claude_settings_path().starts_with(self.dir.path()));
            assert!(crate::claude_plugin::claude_config_path()
                .unwrap()
                .starts_with(self.dir.path()));
            let state = Arc::new(AppState::new(db).expect("create application state"));
            for (id, model) in [("a", "model-a"), ("b", "model-b")] {
                let provider = Provider::with_id(
                    id.into(),
                    id.into(),
                    serde_json::json!({
                        "env": {
                            "ANTHROPIC_BASE_URL": "https://relay.example",
                            "ANTHROPIC_AUTH_TOKEN": "example-key",
                            "ANTHROPIC_MODEL": model
                        }
                    }),
                    None,
                );
                state.db.save_provider("claude", &provider).unwrap();
            }
            state.db.set_current_provider("claude", "a").unwrap();
            crate::settings::set_current_provider(&AppType::Claude, Some("a")).unwrap();
            state
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
            let _ = crate::settings::reload_settings();
        }
    }

    #[test]
    #[serial_test::serial]
    fn provider_delete_waits_for_selection_and_rechecks_current() {
        let home = TempHome::new();
        let state = home.state();
        let guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("claude"));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let deleting_state = state.clone();
        let deleting = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = ProviderService::delete(&deleting_state, AppType::Claude, "b");
            done_tx.send(result).unwrap();
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("deletion started");
        let before_unlock = done_rx.recv_timeout(Duration::from_millis(100));
        let waited = matches!(
            before_unlock,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        if waited {
            // A selection holding this lock publishes B before deletion may inspect current.
            state.db.set_current_provider("claude", "b").unwrap();
            crate::settings::set_current_provider(&AppType::Claude, Some("b")).unwrap();
        }
        drop(guard);
        let result = match before_unlock {
            Ok(result) => result,
            Err(_) => done_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("deletion completed after unlock"),
        };
        deleting.join().expect("deletion thread completed");
        assert!(waited, "delete bypassed the application's selection lock");
        assert!(
            result.is_err(),
            "delete must recheck the newly selected current provider"
        );
        assert!(state
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .is_some());
        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some("b")
        );
    }

    #[test]
    #[serial_test::serial]
    fn noncurrent_edit_does_not_read_or_restore_unrelated_live_files() {
        let home = TempHome::new();
        let state = home.state();
        let live_path = crate::config::get_claude_settings_path();
        // A directory at the file path causes a real read error on every supported OS.
        std::fs::create_dir_all(&live_path).unwrap();
        let sentinel = live_path.join("preserve.txt");
        std::fs::write(&sentinel, b"current application data").unwrap();
        let mut edited = state.db.get_provider_by_id("b", "claude").unwrap().unwrap();
        edited.name = "Updated configuration".into();
        edited.settings_config["env"]["ANTHROPIC_MODEL"] = serde_json::json!("model-b-updated");
        ProviderService::update(&state, AppType::Claude, None, edited.clone()).unwrap();
        let saved = state.db.get_provider_by_id("b", "claude").unwrap().unwrap();
        assert_eq!(saved.name, "Updated configuration");
        assert_eq!(
            saved.settings_config["env"]["ANTHROPIC_MODEL"],
            "model-b-updated"
        );
        edited.settings_config = serde_json::json!([]);
        assert!(ProviderService::update(&state, AppType::Claude, None, edited).is_err());
        assert_eq!(
            state
                .db
                .get_provider_by_id("b", "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            saved.settings_config
        );
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"current application data"
        );
        assert!(live_path.is_dir());
        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some("a")
        );
    }

    #[test]
    #[serial_test::serial]
    fn blocked_selection_preserves_current_model_and_native_configuration() {
        let home = TempHome::new();
        let state = home.state();
        let live_path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        let live =
            b"{\n  \"env\": {\"ANTHROPIC_MODEL\": \"model-a\"},\n  \"customPreference\": true\n}\n";
        std::fs::write(&live_path, live).unwrap();
        crate::proxy::auto_strategy::set_model_pref(&state.db, "claude", Some("model-a")).unwrap();
        crate::proxy::application_routing::set_tier_blocked(&state.db, "claude", "b", true)
            .unwrap();
        let target_before = state.db.get_provider_by_id("b", "claude").unwrap().unwrap();

        assert!(ProviderService::switch(&state, AppType::Claude, "b").is_err());

        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some("a")
        );
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Claude).as_deref(),
            Some("a")
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref(&state.db, "claude").as_deref(),
            Some("model-a")
        );
        assert_eq!(
            state
                .db
                .get_provider_by_id("b", "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            target_before.settings_config
        );
        assert_eq!(std::fs::read(&live_path).unwrap(), live);
        assert!(
            crate::proxy::application_routing::blocked_tier_ids(&state.db, "claude").contains("b")
        );
    }
    #[cfg(feature = "gui")]
    #[tokio::test]
    #[serial_test::serial]
    async fn failed_first_selection_clears_the_active_target_mirror() {
        let home = TempHome::new();
        let state = home.state();
        {
            let conn = state.db.conn.lock().unwrap();
            conn.execute(
                "UPDATE providers SET is_current = 0 WHERE app_type = 'claude'",
                [],
            )
            .unwrap();
        }
        crate::settings::set_current_provider(&AppType::Claude, None).unwrap();
        let original = state.db.get_provider_by_id("a", "claude").unwrap().unwrap();
        let live = serde_json::to_vec_pretty(&original.settings_config).unwrap();
        let live_path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        std::fs::write(&live_path, &live).unwrap();
        state
            .db
            .save_live_backup(
                "claude",
                &serde_json::to_string(&original.settings_config).unwrap(),
            )
            .await
            .unwrap();
        let mut config = state.db.get_global_proxy_config().await.unwrap();
        config.listen_port = 0;
        state.db.update_global_proxy_config(config).await.unwrap();
        state.proxy_service.start().await.unwrap();
        let mirror_during_commit = std::cell::Cell::new(false);
        let error = crate::services::application_selection::select_with_commit(
            &state,
            &AppType::Claude,
            &crate::services::application_selection::TierSelection {
                provider_id: "b".into(),
                model: None,
            },
            || {
                let status = futures::executor::block_on(state.proxy_service.get_status())
                    .map_err(AppError::Config)?;
                mirror_during_commit.set(
                    status
                        .active_targets
                        .iter()
                        .any(|target| target.app_type == "claude" && target.provider_id == "b"),
                );
                Err(AppError::Config("order commit rejected".into()))
            },
        );
        let status = state.proxy_service.get_status().await.unwrap();
        state.proxy_service.stop().await.unwrap();
        assert!(
            mirror_during_commit.get(),
            "selection must reach the active mirror before the injected failure"
        );
        assert!(error
            .unwrap_err()
            .to_string()
            .contains("order commit rejected"));
        assert!(state.db.get_current_provider("claude").unwrap().is_none());
        assert!(crate::settings::get_current_provider(&AppType::Claude).is_none());
        assert!(
            status
                .active_targets
                .iter()
                .all(|target| target.app_type != "claude"),
            "failed selection must not remain displayed as in use"
        );
        assert_eq!(std::fs::read(live_path).unwrap(), live);
    }
}
