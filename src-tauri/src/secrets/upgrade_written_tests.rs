//! App-local ownership evidence must never authorize a different native writer.
use super::*;
use crate::app_config::AppType;
use crate::live::patch::LivePatch;
use serde_json::json;

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn native_completion_rejects_foreign_written_without_blocking_future_peer() {
    for app in [AppType::Claude, AppType::Gemini, AppType::GrokBuild] {
        for written in [
            None,
            Some(json!({})),
            Some(json!({"tables":["retired"]})),
            Some(json!({"codex":{"version":1}})),
        ] {
            let accepted = written.is_none()
                || (app == AppType::GrokBuild && written.as_ref().unwrap().get("codex").is_none());
            let f = Fixture::new();
            publish_resume_fixture(&f);
            let db = Database::from_connection(
                rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
                session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
            );
            let config = match app {
                AppType::Claude => {
                    json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key","ANTHROPIC_BASE_URL":"https://provider.example.invalid"}})
                }
                AppType::Gemini => {
                    json!({"env":{"GEMINI_API_KEY":"synthetic-key","GOOGLE_GEMINI_BASE_URL":"https://provider.example.invalid"}})
                }
                AppType::GrokBuild => {
                    json!({"config":"[models]\ndefault='synthetic'\n[model.synthetic]\nmodel='synthetic'\nname='Synthetic'\napi_backend='responses'\ncontext_window=200000\nbase_url='https://provider.example.invalid'\napi_key='synthetic-key'\n"})
                }
                _ => unreachable!(),
            };
            let row = crate::provider::Provider::with_id(
                "synthetic".into(),
                "synthetic".into(),
                config.clone(),
                None,
            );
            db.save_provider(app.as_str(), &row).unwrap();
            db.set_current_provider(app.as_str(), &row.id).unwrap();
            drop(db);
            let mut settings = crate::settings::AppSettings::default();
            match app {
                AppType::Claude => settings.current_provider_claude = Some(row.id.clone()),
                AppType::Gemini => settings.current_provider_gemini = Some(row.id.clone()),
                AppType::GrokBuild => settings.current_provider_grokbuild = Some(row.id.clone()),
                _ => unreachable!(),
            }
            f.write_settings(&settings);
            let mut entry = json!({"mode":"direct"});
            if let Some(written) = &written {
                entry["written"] = written.clone();
            }
            let mut live = json!({"version":1,"apps":{"codex":{"mode":"future-mode","written":{"future":true}}}});
            live["apps"][app.as_str()] = entry;
            f.write_raw_mode(&serde_json::to_vec(&live).unwrap());
            match app {
                AppType::Claude => {
                    let path = crate::config::get_claude_settings_path();
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
                }
                AppType::Gemini => {
                    let projection =
                        crate::services::provider::gemini_direct::projection(&row).unwrap();
                    let env = crate::gemini_config::get_gemini_env_path();
                    let settings = crate::gemini_config::get_gemini_settings_path();
                    std::fs::create_dir_all(env.parent().unwrap()).unwrap();
                    std::fs::write(
                        &env,
                        projection
                            .env_patch()
                            .apply(&env, Some(b"USER_OPTION=keep\n"))
                            .unwrap(),
                    )
                    .unwrap();
                    std::fs::write(
                        &settings,
                        projection
                            .settings_patch()
                            .apply(&settings, Some(br#"{"ui":{"theme":"keep"}}"#))
                            .unwrap(),
                    )
                    .unwrap();
                }
                AppType::GrokBuild => {
                    crate::services::provider::grok_direct::projection(&row)
                        .expect("Grok ownership fixture must be a valid native provider");
                    let path = crate::grok_config::get_grok_config_path();
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, config["config"].as_str().unwrap()).unwrap();
                }
                _ => unreachable!(),
            }
            let inspected = inspect(&f.root, &f.device).unwrap();
            let review =
                AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                    .unwrap();
            let token = review.view(&inspected).unwrap().review_token.unwrap();
            let before = snapshot(f.home.path());
            let view = review.review_app(&inspected, &token, &app).unwrap();
            assert_eq!(
                view.can_complete_app,
                accepted,
                "app={}, written={written:?}",
                app.as_str()
            );
            assert!(!view.can_recover_operation && !view.can_start_upgrade);
            assert_eq!(snapshot(f.home.path()), before);
            let session = review.runtime_session(&inspected, &token).unwrap();
            crate::settings::unlock_settings(session.clone()).unwrap();
            let state = crate::store::AppState::new(std::sync::Arc::new(
                Database::init_with_secrets(session).unwrap(),
            ))
            .unwrap();
            let before = snapshot(f.home.path());
            let result = crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &app);
            assert_eq!(
                result.is_ok(),
                accepted,
                "native app={}, written={written:?}",
                app.as_str()
            );
            drop(result);
            assert!(crate::mode::operation::AppWrite::begin_mode(
                &state.proxy_service,
                &AppType::Codex
            )
            .is_err());
            assert_eq!(snapshot(f.home.path()), before);
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn published_pointer_rejects_foreign_written_and_retains_original_journal() {
    for target in [false, true] {
        for written in [json!({}), json!({"tables":["synthetic-grok"]})] {
            let f = Fixture::new();
            published_pointer_fixture(&f);
            let mut entry = json!({"mode":"direct","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"},"published":true}});
            if target {
                entry["pending"]["target"]["written"] = written;
            } else {
                entry["written"] = written;
            }
            f.write_raw_mode(
                &serde_json::to_vec(&json!({"version":1,"apps":{"codex":entry}})).unwrap(),
            );
            let coordinator = crate::secrets::startup::StartupCoordinator::new(
                f.root.clone(),
                inspect(&f.root, &f.device).unwrap(),
            );
            let token = coordinator
                .authenticate_upgrade(None, &f.store)
                .unwrap()
                .review_token
                .unwrap();
            let before = snapshot(f.home.path());
            let view = coordinator
                .review_upgrade_app(&token, &AppType::Codex)
                .unwrap();
            assert!(
                !view.can_recover_operation,
                "foreign Written in target={target}"
            );
            assert!(coordinator
                .recover_upgrade_app(&token, &AppType::Codex, &view.revision)
                .is_err());
            assert_eq!(snapshot(f.home.path()), before);
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn operation_written_targets_are_bound_to_the_app_before_staging_or_recovery() {
    use crate::live::{
        engine::{lock_app, LiveFile},
        patch::{json::JsonPatch, KeyPath},
    };
    use crate::mode::{operation, state};
    for app in [
        AppType::Claude,
        AppType::Gemini,
        AppType::Codex,
        AppType::GrokBuild,
    ] {
        for written in [
            None,
            Some(json!({})),
            Some(json!({"tables":["retired"]})),
            Some(json!({"codex":{"version":1}})),
            Some(json!({"tables":["retired"],"codex":{"version":1}})),
            Some(json!({"codex":{"version":2}})),
            Some(json!({"future":true})),
        ] {
            let accepted = written.is_none()
                || match app {
                    AppType::GrokBuild => written
                        .as_ref()
                        .is_some_and(|w| w.get("codex").is_none() && w.get("future").is_none()),
                    AppType::Codex => written.as_ref().is_some_and(|w| {
                        w.get("tables").is_none() && w.pointer("/codex/version") == Some(&json!(1))
                    }),
                    _ => false,
                };
            let f = Fixture::new();
            let path = f.home.path().join("synthetic-client.json");
            std::fs::write(&path, br#"{"key":"original","user":"keep"}"#).unwrap();
            let mut target = state::PendingTarget::pointer(Some("synthetic".into()));
            target.written = written
                .as_ref()
                .map(|w| serde_json::from_value(w.clone()).unwrap());
            let key = std::sync::RwLock::new(f.vault.clone());
            let vault = key.read().unwrap();
            let guard = lock_app(app.as_str());
            let patch = JsonPatch {
                set: vec![(KeyPath::new(&["key"]), json!("planned"))],
                ..Default::default()
            };
            let called = std::cell::Cell::new(false);
            let before = snapshot(f.home.path());
            let result = operation::run(
                &f.device,
                &vault,
                &guard,
                state::op::SWITCH,
                &[operation::FileChange {
                    file: LiveFile::shared(&path),
                    patch: &patch,
                }],
                target,
                &|_| {
                    called.set(true);
                    Ok(())
                },
            );
            assert_eq!(
                result.is_ok(),
                accepted,
                "app={}, target={written:?}",
                app.as_str()
            );
            assert_eq!(called.get(), accepted);
            if !accepted {
                assert_eq!(snapshot(f.home.path()), before);
                let pending = json!({"op":"switch","files":[],"target":{"pointer":"synthetic","written":written},"published":false});
                f.write_raw_mode(&serde_json::to_vec(&json!({"version":1,"apps":{(app.as_str()):{"mode":"direct","pending":pending}}})).unwrap());
                let before = snapshot(f.home.path());
                assert!(operation::recover(
                    &f.device,
                    &vault,
                    &guard,
                    &[LiveFile::shared(&path)],
                    &|_| {
                        called.set(true);
                        Ok(())
                    }
                )
                .is_err());
                assert!(!called.get());
                assert_eq!(snapshot(f.home.path()), before);
            }
        }
    }
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify() {
    operation_written_targets_are_bound_to_the_app_before_staging_or_recovery();
    println!("PASS 28 app/Written targets and rejected recovery snapshots");
    native_completion_rejects_foreign_written_without_blocking_future_peer();
    println!("PASS 12 native app/Written cases with read-only and native admission checks");
    published_pointer_rejects_foreign_written_and_retains_original_journal();
    println!("PASS four foreign Written journal cases without recovery writes");
}
