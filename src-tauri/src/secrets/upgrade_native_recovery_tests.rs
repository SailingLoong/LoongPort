//! Original native writes interrupted while the upgrade checkpoint is retained.
use super::*;
use crate::{app_config::AppType, mode::operation, provider::Provider};
use serde_json::json;

fn native_runtime(
    f: &Fixture,
    app: &AppType,
) -> (
    UpgradeInspection,
    AuthenticatedUpgrade,
    String,
    crate::store::AppState,
) {
    use crate::live::patch::LivePatch;
    publish_resume_fixture(f);
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    for id in ["a", "b"] {
        let config = match app {
            AppType::Claude => json!({"env": {
                "ANTHROPIC_AUTH_TOKEN": format!("synthetic-{id}"),
                "ANTHROPIC_BASE_URL": format!("https://{id}.example.invalid"),
                "ANTHROPIC_MODEL": format!("synthetic-{id}")
            }}),
            AppType::Gemini => {
                json!({"env": {"GEMINI_API_KEY": format!("synthetic-{id}"), "GOOGLE_GEMINI_BASE_URL": format!("https://{id}.example.invalid")}, "config":{"model":{"name":format!("synthetic-{id}")}}})
            }
            AppType::Codex => {
                json!({"auth":{"OPENAI_API_KEY":format!("synthetic-{id}")}, "config":format!("model = 'synthetic-{id}'\nmodel_provider = 'original'\n[model_providers.original]\nname = 'Synthetic'\nbase_url = 'https://{id}.example.invalid/v1'\nwire_api = 'responses'\n")})
            }
            AppType::GrokBuild => {
                json!({"config":format!("[models]\ndefault = '{id}'\n[model.{id}]\nmodel = 'synthetic-{id}'\nname = 'Synthetic {id}'\nbase_url = 'https://{id}.example.invalid/v1'\napi_key = 'synthetic-key-{id}'\napi_backend = 'responses'\ncontext_window = 200000\n")})
            }
            _ => unreachable!(),
        };
        db.save_provider(
            app.as_str(),
            &Provider::with_id(id.into(), "Synthetic".into(), config, None),
        )
        .unwrap();
    }
    db.set_current_provider(app.as_str(), "a").unwrap();
    let row = db.get_provider_by_id("a", app.as_str()).unwrap().unwrap();
    let mut settings = crate::settings::AppSettings::default();
    match app {
        AppType::Claude => settings.current_provider_claude = Some("a".into()),
        AppType::Codex => settings.current_provider_codex = Some("a".into()),
        AppType::Gemini => settings.current_provider_gemini = Some("a".into()),
        AppType::GrokBuild => settings.current_provider_grokbuild = Some("a".into()),
        _ => unreachable!(),
    }
    f.write_settings(&settings);
    for file in crate::mode::controller::files(app).unwrap() {
        std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
    }
    match app {
        AppType::Claude => {
            let mut config = row.settings_config.clone();
            config["unowned"] = json!({"keep":true});
            std::fs::write(
                crate::config::get_claude_settings_path(),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }
        AppType::Gemini => {
            let projection = crate::services::provider::gemini_direct::projection(&row).unwrap();
            let env = crate::gemini_config::get_gemini_env_path();
            let config = crate::gemini_config::get_gemini_settings_path();
            std::fs::write(
                &env,
                projection
                    .env_patch()
                    .apply(&env, Some(b"# synthetic-user\nUSER_OPTION=keep\n"))
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(
                &config,
                projection
                    .settings_patch()
                    .apply(&config, Some(br#"{"ui":{"theme":"keep"}}"#))
                    .unwrap(),
            )
            .unwrap();
        }
        AppType::Codex => {
            let text = row.settings_config["config"]
                .as_str()
                .unwrap()
                .replace("original", "custom");
            std::fs::write(crate::codex_config::get_codex_config_path(), format!("# synthetic-user\n{text}experimental_bearer_token = 'synthetic-a'\n[ui]\ntheme = 'keep'\n")).unwrap();
            std::fs::write(crate::codex_config::get_codex_auth_path(), br#"{"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-native","access_token":"synthetic-access","refresh_token":"synthetic-refresh"}}"#).unwrap();
        }
        AppType::GrokBuild => {
            std::fs::write(crate::grok_config::get_grok_config_path(), format!("# synthetic-user\n{}\n[ui]\ntheme = 'keep'\n[model.mine]\nmodel = 'synthetic-user'\napi_key = 'synthetic-user-key'\n", row.settings_config["config"].as_str().unwrap())).unwrap();
        }
        _ => unreachable!(),
    }
    drop(db);
    let peer = if *app == AppType::Codex {
        "claude"
    } else {
        "codex"
    };
    f.write_raw_mode(format!(r#"{{"version":1,"apps":{{"{}":{{"mode":"direct","attached":false}},"{peer}":{{"mode":"future-mode","opaque":900719925474099312345}}}}}}"#, app.as_str()).as_bytes());
    let inspected = inspect(&f.root, &f.device).unwrap();
    let review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let session = review.runtime_session(&inspected, &token).unwrap();
    crate::settings::unlock_settings(session.clone()).unwrap();
    let state = crate::store::AppState::new(std::sync::Arc::new(
        Database::init_with_secrets(session).unwrap(),
    ))
    .unwrap();
    (inspected, review, token, state)
}

fn assert_native_result(app: &AppType, expected: &str) {
    match app {
        AppType::Claude => {
            let config: serde_json::Value = serde_json::from_slice(
                &std::fs::read(crate::config::get_claude_settings_path()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                config["env"]["ANTHROPIC_AUTH_TOKEN"],
                format!("synthetic-{expected}")
            );
            assert_eq!(config["unowned"], json!({"keep":true}));
        }
        AppType::Gemini => {
            let text =
                std::fs::read_to_string(crate::gemini_config::get_gemini_env_path()).unwrap();
            assert!(text.contains(&format!("GEMINI_API_KEY=synthetic-{expected}")));
            assert!(text.contains("USER_OPTION=keep"));
            let config: serde_json::Value = serde_json::from_slice(
                &std::fs::read(crate::gemini_config::get_gemini_settings_path()).unwrap(),
            )
            .unwrap();
            assert_eq!(config["model"]["name"], format!("synthetic-{expected}"));
            assert_eq!(config["ui"]["theme"], "keep");
        }
        AppType::Codex => {
            let text =
                std::fs::read_to_string(crate::codex_config::get_codex_config_path()).unwrap();
            let doc: toml_edit::DocumentMut = text.parse().unwrap();
            assert_eq!(
                doc["model"].as_str(),
                Some(format!("synthetic-{expected}").as_str())
            );
            assert_eq!(doc["ui"]["theme"].as_str(), Some("keep"));
            assert!(text.contains("# synthetic-user"));
            let auth: serde_json::Value = serde_json::from_slice(
                &std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap(),
            )
            .unwrap();
            assert_eq!(auth["tokens"]["account_id"], "synthetic-native");
        }
        AppType::GrokBuild => {
            let text = std::fs::read_to_string(crate::grok_config::get_grok_config_path()).unwrap();
            let doc: toml_edit::DocumentMut = text.parse().unwrap();
            assert_eq!(doc["models"]["default"].as_str(), Some(expected));
            assert_eq!(doc["ui"]["theme"].as_str(), Some("keep"));
            assert_eq!(
                doc["model"]["mine"]["api_key"].as_str(),
                Some("synthetic-user-key")
            );
        }
        _ => unreachable!(),
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovers_original_native_switch() {
    for app in [
        AppType::Claude,
        AppType::Gemini,
        AppType::Codex,
        AppType::GrokBuild,
    ] {
        let partial = if app == AppType::Codex {
            "published:1"
        } else {
            "published:0"
        };
        for point in ["pending", "marked", partial] {
            let f = Fixture::new();
            let (inspected, review, token, runtime) = native_runtime(&f, &app);
            assert!(
                review
                    .review_app(&inspected, &token, &app)
                    .unwrap()
                    .can_complete_app
            );
            let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
            let peer = peer_value(&f, &app);
            operation::failpoint::crash_at(Some(point));
            let result = crate::services::ProviderService::switch(&runtime, app.clone(), "b");
            operation::failpoint::crash_at(None);
            assert!(
                result.is_err(),
                "actual original switch must stop at the injected publication"
            );
            assert!(crate::mode::state::pending(
                &f.device,
                &runtime.db.secret_session().read().unwrap(),
                app.as_str()
            )
            .unwrap()
            .is_some());
            let before = snapshot(f.home.path());
            assert!(operation::AppWrite::begin_mode(&runtime.proxy_service, &app).is_err());
            let view = review.review_app(&inspected, &token, &app).unwrap();
            assert_eq!(
                snapshot(f.home.path()),
                before,
                "review/admission must not write"
            );
            assert!(
            view.can_recover_operation,
            "original native journal must reach controlled recovery while checkpoint is retained"
        );
            if point != partial || matches!(app, AppType::Codex | AppType::GrokBuild) {
                let before = snapshot(f.home.path());
                assert!(review
                    .recover_app(&inspected, &token, &app, &view.revision)
                    .is_err());
                assert_eq!(snapshot(f.home.path()), before);
            }
            let recovered = review
                .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
                .unwrap();
            assert_eq!(recovered.has_pending_operation, Some(false));
            assert!(recovered.can_complete_app);
            let expected = if point == "pending" { "a" } else { "b" };
            assert_eq!(
                runtime
                    .db
                    .get_current_provider(app.as_str())
                    .unwrap()
                    .as_deref(),
                Some(expected)
            );
            assert_eq!(
                crate::settings::get_current_provider_ready(&app)
                    .unwrap()
                    .as_deref(),
                Some(expected)
            );
            assert_native_result(&app, expected);
            assert_eq!(peer_value(&f, &app), peer);
            println!("PASS original native recovery: {app:?}/{point}");
            assert_eq!(
                std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
                checkpoint
            );
            assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
            let before = snapshot(f.home.path());
            assert!(review
                .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
                .is_err());
            assert_eq!(
                snapshot(f.home.path()),
                before,
                "lost reply must not replay the old revision"
            );
        }
    }
}

fn interrupt_switch(runtime: &crate::store::AppState, app: &AppType, point: &str) {
    operation::failpoint::crash_at(Some(point));
    let result = crate::services::ProviderService::switch(runtime, app.clone(), "b");
    operation::failpoint::crash_at(None);
    assert!(
        result.is_err(),
        "{app:?}/{point}: expected original failpoint"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_rejects_stale_inputs_without_writes() {
    for scenario in [
        "staging",
        "native",
        "row",
        "settings",
        "future-own-state",
        "checkpoint",
        "session",
    ] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let (inspected, review, token, runtime) = native_runtime(&f, &app);
        interrupt_switch(&runtime, &app, "pending");
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(view.can_recover_operation);
        let pending = crate::mode::state::pending(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            app.as_str(),
        )
        .unwrap()
        .unwrap();
        let mut other = None;
        match scenario {
            "staging" => std::fs::write(pending.files[0].staged.as_ref().unwrap(), b"synthetic changed staging").unwrap(),
            "native" => std::fs::write(crate::config::get_claude_settings_path(), br#"{"unowned":"external"}"#).unwrap(),
            "row" => {
                let mut row = runtime.db.get_provider_by_id("b", "claude").unwrap().unwrap();
                row.name = "Changed externally".into();
                runtime.db.save_provider("claude", &row).unwrap();
            }
            "settings" => f.write_settings(&crate::settings::AppSettings {current_provider_claude:Some("external".into()), ..Default::default()}),
            "future-own-state" => f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"future-mode"},"codex":{"mode":"future-mode","opaque":900719925474099312345}}}"#),
            "checkpoint" => {
                let path = f.device.root().join(checkpoint::FILE);
                let mut bytes = std::fs::read(&path).unwrap(); bytes.push(b'\n'); std::fs::write(path, bytes).unwrap();
            }
            "session" => {
                let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
                other = Some(crate::store::AppState::new(std::sync::Arc::new(Database::from_connection(rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(), session))).unwrap());
            }
            _ => unreachable!(),
        }
        let before = snapshot(f.home.path());
        assert!(
            review
                .recover_app_with_state(
                    &inspected,
                    &token,
                    &app,
                    &view.revision,
                    other.as_ref().unwrap_or(&runtime)
                )
                .is_err(),
            "{scenario}"
        );
        assert_eq!(
            snapshot(f.home.path()),
            before,
            "{scenario}: stale submission wrote files"
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_checks_late_identity_and_metadata() {
    for (point, effect, interruption) in [
        ("upgrade:native_recovery_owner", "checkpoint", "pending"),
        ("recover:begin", "row", "pending"),
        ("recover:begin", "checkpoint", "marked"),
        ("recover:target", "row", "marked"),
        ("recover:verified", "preference", "marked"),
        ("recover:verified", "journal", "marked"),
    ] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let (inspected, review, token, runtime) = native_runtime(&f, &app);
        interrupt_switch(&runtime, &app, interruption);
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(view.can_recover_operation);
        let path = f.device.root().join(checkpoint::FILE);
        let db = runtime.db.clone();
        let home = f.home.path().to_path_buf();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
        let captured = seen.clone();
        operation::failpoint::on_boundary(Some(Box::new(move |at| {
            if at != point || captured.borrow().is_some() {
                return;
            }
            match effect {
                "checkpoint" => {
                    let mut bytes = std::fs::read(&path).unwrap();
                    bytes.push(b'\n');
                    std::fs::write(&path, bytes).unwrap();
                }
                "row" => {
                    let mut row = db.get_provider_by_id("b", "claude").unwrap().unwrap();
                    row.name = "Changed at recovery boundary".into();
                    db.save_provider("claude", &row).unwrap();
                }
                "preference" => crate::proxy::auto_strategy::set_model_pref(
                    &db,
                    "claude",
                    Some("external-model"),
                )
                .unwrap(),
                "journal" => {
                    let store = DeviceStore::for_device();
                    let vault = db.secret_session().read().unwrap();
                    let mut pending = crate::mode::state::pending(&store, &vault, "claude")
                        .unwrap()
                        .unwrap();
                    pending.op = crate::mode::state::op::APPLY.into();
                    crate::mode::state::set_pending(&store, &vault, "claude", Some(pending))
                        .unwrap();
                }
                _ => unreachable!(),
            }
            *captured.borrow_mut() = Some(snapshot(&home));
        })));
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        operation::failpoint::on_boundary(None);
        assert!(result.is_err(), "{point}/{effect}: late drift accepted");
        assert!(
            seen.borrow().is_some(),
            "{point}/{effect}: boundary was not reached"
        );
        assert_eq!(
            snapshot(f.home.path()),
            *seen.borrow().as_ref().unwrap(),
            "{point}/{effect}: wrote after drift"
        );
        assert!(crate::mode::state::pending(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            "claude"
        )
        .unwrap()
        .is_some());
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_retries_original_codex_journal() {
    let f = Fixture::new();
    let app = AppType::Codex;
    let (inspected, review, token, runtime) = native_runtime(&f, &app);
    interrupt_switch(&runtime, &app, "published:1");
    let view = review.review_app(&inspected, &token, &app).unwrap();
    assert!(view.can_recover_operation);
    operation::failpoint::crash_at(Some("recover:target"));
    let first = review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
    operation::failpoint::crash_at(None);
    assert!(first.is_err());
    let fresh = review.review_app(&inspected, &token, &app).unwrap();
    assert!(fresh.can_recover_operation);
    assert_ne!(fresh.revision, view.revision);
    let before = snapshot(f.home.path());
    assert!(review
        .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let recovered = review
        .recover_app_with_state(&inspected, &token, &app, &fresh.revision, &runtime)
        .unwrap();
    assert_eq!(recovered.has_pending_operation, Some(false));
    assert!(recovered.can_complete_app);
    assert_native_result(&app, "b");
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}

#[cfg(not(windows))]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_rejects_replaced_database() {
    let f = Fixture::new();
    let app = AppType::Claude;
    let (inspected, review, token, runtime) = native_runtime(&f, &app);
    interrupt_switch(&runtime, &app, "pending");
    let replacement = f.root.join("synthetic-replacement.db");
    let mut copy = rusqlite::Connection::open(&replacement).unwrap();
    rusqlite::backup::Backup::new(&runtime.db.conn.lock().unwrap(), &mut copy)
        .unwrap()
        .run_to_completion(10, std::time::Duration::ZERO, None)
        .unwrap();
    drop(copy);
    let view = review.review_app(&inspected, &token, &app).unwrap();
    std::fs::rename(replacement, f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let before = snapshot(f.home.path());
    assert!(review
        .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
}

fn peer_value(f: &Fixture, app: &AppType) -> String {
    let peer = if *app == AppType::Codex {
        "claude"
    } else {
        "codex"
    };
    let file = crate::secrets::owned_file::DeviceFile::registered(
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    )
    .unwrap();
    let plain = file
        .decode(&f.vault, &std::fs::read(f.device.state_path()).unwrap())
        .unwrap();
    let root: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&plain).unwrap();
    let apps: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(root["apps"].get()).unwrap();
    apps[peer].get().to_owned()
}

fn reviewed_late_drift(point: &'static str, effect: &'static str) {
    let f = Fixture::new();
    let app = AppType::Claude;
    let (inspected, review, token, runtime) = native_runtime(&f, &app);
    interrupt_switch(&runtime, &app, "marked");
    if effect == "duplicate" {
        let mut row = runtime
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .unwrap();
        row.id = "c".into();
        runtime.db.save_provider("claude", &row).unwrap();
        runtime.db.conn.lock().unwrap().execute("UPDATE providers SET is_current=CASE WHEN id IN ('b','c') THEN 1 ELSE 0 END WHERE app_type='claude'", []).unwrap();
    }
    let view = review.review_app(&inspected, &token, &app).unwrap();
    assert!(view.can_recover_operation);
    let db = runtime.db.clone();
    let home = f.home.path().to_path_buf();
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let captured = seen.clone();
    operation::failpoint::on_boundary(Some(Box::new(move |at| {
        if at != point || captured.borrow().is_some() {
            return;
        }
        match effect {
            "endpoint" => db
                .add_custom_endpoint("claude", "b", "https://external.example.invalid/v1")
                .unwrap(),
            "pointer" => {
                let vault = db.secret_session().read().unwrap();
                crate::settings::set_current_provider_with_vault(
                    &AppType::Claude,
                    Some("a"),
                    db.secret_session(),
                    &vault,
                )
                .unwrap();
                db.set_current_provider("claude", "a").unwrap();
            }
            "duplicate" => {
                db.conn
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE providers SET is_current=1 WHERE app_type='claude' AND id='c'",
                        [],
                    )
                    .unwrap();
            }
            "priority" => crate::proxy::application_routing::write_order_on(
                &db.conn.lock().unwrap(),
                "claude",
                &["b".into(), "a".into()],
            )
            .unwrap(),
            "queue" => {
                db.conn.lock().unwrap().execute("UPDATE providers SET in_failover_queue=1 WHERE app_type='claude' AND id='b'", []).unwrap();
            }
            "native" => std::fs::write(
                crate::config::get_claude_settings_path(),
                br#"{"unowned":"external-after-target"}"#,
            )
            .unwrap(),
            "journal" => {
                let store = DeviceStore::for_device();
                let vault = db.secret_session().read().unwrap();
                let mut pending = crate::mode::state::pending(&store, &vault, "claude")
                    .unwrap()
                    .unwrap();
                pending.files[0].planned = pending.files[0].pre.clone();
                pending.files[0].staged = None;
                crate::mode::state::set_pending(&store, &vault, "claude", Some(pending)).unwrap();
            }
            _ => unreachable!(),
        }
        // Stabilize the same runtime connection's SQLite reader mark after
        // injected SQL writes; keep DB, WAL, SHM and intent bytes in the assertion.
        db.get_current_provider("claude").unwrap();
        *captured.borrow_mut() = Some(snapshot(&home));
    })));
    let result = review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
    operation::failpoint::on_boundary(None);
    assert!(
        seen.borrow().is_some(),
        "{point}/{effect}: boundary not reached"
    );
    assert!(
        result.is_err(),
        "{point}/{effect}: unreviewed evidence was accepted"
    );
    let after = snapshot(f.home.path());
    let before = seen.borrow();
    let before = before.as_ref().unwrap();
    let changed = after
        .keys()
        .chain(before.keys())
        .filter(|path| after.get(*path) != before.get(*path))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        changed.is_empty(),
        "{point}/{effect}: wrote after evidence changed: {changed:?}"
    );
    assert!(crate::mode::state::pending(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "claude"
    )
    .unwrap()
    .is_some());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_binds_late_endpoint() {
    reviewed_late_drift("recover:verified", "endpoint");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_binds_cleanup_files() {
    reviewed_late_drift("recover:verified", "native");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_binds_original_pending() {
    reviewed_late_drift("recover:load", "journal");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_proves_final_target() {
    reviewed_late_drift("recover:verified", "pointer");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_binds_late_priority() {
    reviewed_late_drift("recover:verified", "priority");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_binds_late_queue() {
    reviewed_late_drift("recover:verified", "queue");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_requires_unique_final_pointer() {
    reviewed_late_drift("recover:verified", "duplicate");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_allows_owned_row_reordering() {
    let f = Fixture::new();
    let app = AppType::Claude;
    let (inspected, review, token, runtime) = native_runtime(&f, &app);
    for (id, sort) in [("a", 10), ("b", 20)] {
        let mut row = runtime
            .db
            .get_provider_by_id(id, "claude")
            .unwrap()
            .unwrap();
        row.sort_index = Some(sort);
        runtime.db.save_provider("claude", &row).unwrap();
    }
    let mut row = runtime
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    row.sort_index = Some(0);
    row.created_at = Some(123);
    runtime.db.conn.lock().unwrap().execute_batch("CREATE TEMP TRIGGER synthetic_row_failure BEFORE UPDATE OF sort_index ON providers BEGIN SELECT RAISE(FAIL, 'synthetic row failure'); END;").unwrap();
    let result = crate::services::ProviderService::update(&runtime, app.clone(), None, row.clone());
    runtime
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER synthetic_row_failure;")
        .unwrap();
    assert!(result.is_err());
    let view = review.review_app(&inspected, &token, &app).unwrap();
    assert!(view.can_recover_operation);
    let result = review
        .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
        .unwrap();
    assert_eq!(result.has_pending_operation, Some(false));
    let actual = runtime
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    assert_eq!(actual.sort_index, row.sort_index);
    assert_eq!(actual.created_at, row.created_at);
    assert_native_result(&app, "a");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_defers_attached_listener_dependency() {
    for boundary in ["current", "target"] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let (inspected, review, token, runtime) = native_runtime(&f, &app);
        interrupt_switch(&runtime, &app, "marked");
        crate::mode::state::update_app(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            "claude",
            |entry| {
                let attached = crate::mode::state::ModeState {
                    mode: Some(crate::mode::state::Mode::Proxy),
                    attached: true,
                    proxy_route: Some("b".into()),
                    ..Default::default()
                };
                if boundary == "current" {
                    entry.set_mode_state(attached).unwrap();
                } else {
                    entry.pending.as_mut().unwrap().target.state = Some(attached);
                }
                Ok(())
            },
        )
        .unwrap();
        let before = snapshot(f.home.path());
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(
            !view.can_recover_operation,
            "{boundary}: listener-dependent recovery was offered"
        );
        assert!(review
            .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
            .is_err());
        assert!(
            snapshot(f.home.path()) == before,
            "{boundary}: defer must not write"
        );
        assert!(crate::mode::state::pending(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            "claude"
        )
        .unwrap()
        .is_some());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_replays_combined_selection() {
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        let f = Fixture::new();
        let (inspected, review, token, runtime) = native_runtime(&f, &app);
        let mut row = runtime
            .db
            .get_provider_by_id("b", app.as_str())
            .unwrap()
            .unwrap();
        row.settings_config["modelCatalog"] = json!({"models":[{"model":"selected-model"}]});
        runtime.db.save_provider(app.as_str(), &row).unwrap();
        operation::failpoint::crash_at(Some("marked"));
        let result = crate::services::application_selection::select_with_commit(
            &runtime,
            &app,
            &crate::services::application_selection::TierSelection {
                provider_id: "b".into(),
                model: Some("selected-model".into()),
            },
            Some(&crate::services::application_selection::RoutingOrder {
                profile_name: "default".into(),
                provider_ids: vec!["b".into(), "a".into()],
            }),
        );
        operation::failpoint::crash_at(None);
        assert!(
            result.is_err(),
            "{app:?}: original combined selection must be interrupted"
        );
        let pending = crate::mode::state::pending(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            app.as_str(),
        )
        .unwrap()
        .unwrap();
        assert!(
            pending.target.saved_row.is_some()
                && pending.target.routing_order.is_some()
                && pending.target.model_preference.is_some()
        );
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(view.can_recover_operation, "{app:?}");
        let recovered = review
            .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
            .unwrap();
        assert_eq!(recovered.has_pending_operation, Some(false));
        assert_eq!(
            runtime
                .db
                .get_current_provider(app.as_str())
                .unwrap()
                .as_deref(),
            Some("b")
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&runtime.db, app.as_str())
                .unwrap()
                .as_deref(),
            Some("selected-model")
        );
        assert_eq!(
            crate::database::order_profiles::snapshot(&runtime.db, app.as_str()).unwrap(),
            pending.target.routing_order.unwrap().planned
        );
        assert_eq!(
            Database::provider_update_digest(
                &runtime
                    .db
                    .get_provider_by_id("b", app.as_str())
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            Database::provider_update_digest(
                &operation::saved_provider(pending.target.saved_row.as_ref().unwrap()).unwrap()
            )
            .unwrap()
        );
        println!("PASS combined selection recovery {app:?}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_reuses_managed_generation_owner() {
    for known in [false, true] {
        let f = Fixture::new();
        let app = AppType::Codex;
        let (inspected, review, token, mut runtime) = native_runtime(&f, &app);
        // Original manager preparation reads the complete device state. Its
        // future-peer refusal remains intact; this generation test uses a known
        // peer, while the ordinary native cases above retain an opaque peer.
        f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","attached":false},"claude":{"mode":"direct","attached":false}}}"#);
        let account = "synthetic-account";
        crate::rt::block_on(
            runtime
                .codex_oauth_manager
                .add_test_account_with_access_token(
                    account,
                    "access-old",
                    Some("synthetic-id-token"),
                ),
        )
        .unwrap();
        let mut row = Provider::with_id(
            "managed".into(),
            "Synthetic managed".into(),
            json!({"auth":{}, "config":"model = 'gpt-5.5'\n"}),
            None,
        );
        row.category = Some("official".into());
        row.meta = Some(crate::provider::ProviderMeta {
            auth_binding: Some(crate::provider::AuthBinding {
                source: crate::provider::AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".into()),
                account_id: Some(account.into()),
            }),
            ..Default::default()
        });
        runtime.db.save_provider("codex", &row).unwrap();
        operation::failpoint::crash_at(Some("target"));
        let result = crate::services::ProviderService::switch(&runtime, app.clone(), "managed");
        operation::failpoint::crash_at(None);
        assert!(result.is_err());
        assert!(
            crate::mode::state::pending(
                &f.device,
                &runtime.db.secret_session().read().unwrap(),
                "codex"
            )
            .unwrap()
            .is_some(),
            "original managed switch did not create intent: {result:?}"
        );
        let mut auth: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap(),
        )
        .unwrap();
        if known {
            crate::rt::block_on(
                runtime
                    .codex_oauth_manager
                    .add_test_account_with_access_token(
                        account,
                        "known-newer-access",
                        Some("synthetic-id-token"),
                    ),
            )
            .unwrap();
            crate::rt::block_on(
                runtime
                    .codex_oauth_manager
                    .test_set_bundle_time(account, chrono::Utc::now().timestamp_millis() + 10_000),
            );
            let bundle = crate::rt::block_on(
                runtime
                    .codex_oauth_manager
                    .prepare_live_token_bundle(account),
            )
            .unwrap();
            auth = crate::codex_config::codex_managed_oauth_auth_value(
                account,
                &bundle.access_token,
                bundle.id_token.as_deref(),
                &bundle.refresh_token,
                &bundle.last_refresh,
            );
        } else {
            auth["tokens"]["access_token"] = json!("unproven-access");
            auth["last_refresh"] =
                json!((chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc3339());
        }
        crate::config::write_json_file_private(&crate::codex_config::get_codex_auth_path(), &auth)
            .unwrap();
        let native = std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap();
        // A future peer may arrive after the original owner's preparation.
        // Recovery must stay app-scoped and leave that peer untouched.
        let owned = crate::secrets::owned_file::DeviceFile::registered(
            crate::secrets::owned_file::DEVICE_STATE_FILE,
        )
        .unwrap();
        let plain = owned
            .decode(&f.vault, &std::fs::read(f.device.state_path()).unwrap())
            .unwrap();
        let mut live: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        live["apps"]["claude"] = json!({"mode":"future-mode", "opaque":"retained-peer"});
        f.write_raw_mode(&serde_json::to_vec(&live).unwrap());
        let peer = peer_value(&f, &app);
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(view.can_recover_operation);
        let before = snapshot(f.home.path());
        if known {
            operation::failpoint::crash_at(Some("recover:adopted"));
        }
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        operation::failpoint::crash_at(None);
        assert!(result.is_err() || result.as_ref().unwrap().has_pending_operation == Some(true));
        if !known {
            assert!(
                snapshot(f.home.path()) == before,
                "unproven auth must not be published or cleared"
            );
        } else {
            runtime = crate::store::AppState::new(runtime.db.clone()).unwrap();
            let fresh = review.review_app(&inspected, &token, &app).unwrap();
            assert_ne!(fresh.revision, view.revision);
            let recovered = review
                .recover_app_with_state(&inspected, &token, &app, &fresh.revision, &runtime)
                .unwrap();
            assert_eq!(recovered.has_pending_operation, Some(false));
            assert!(
                !recovered.can_complete_app,
                "recovery does not invent managed native completion proof"
            );
        }
        assert_eq!(
            std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap(),
            native
        );
        assert_eq!(peer_value(&f, &app), peer);
        println!("PASS checkpoint managed generation known={known}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_replays_detached_mode_target() {
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        let f = Fixture::new();
        let (inspected, review, token, runtime) = native_runtime(&f, &app);
        crate::mode::state::update_app(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            app.as_str(),
            |entry| {
                entry
                    .set_mode_state(crate::mode::state::ModeState {
                        mode: Some(crate::mode::state::Mode::Proxy),
                        attached: false,
                        proxy_route: Some("b".into()),
                        ..Default::default()
                    })
                    .unwrap();
                Ok(())
            },
        )
        .unwrap();
        runtime
            .db
            .set_proxy_flags_sync(app.as_str(), true, true)
            .unwrap();
        runtime.db.conn.lock().unwrap().execute_batch("CREATE TEMP TRIGGER synthetic_mode_failure BEFORE UPDATE OF enabled ON proxy_config BEGIN SELECT RAISE(FAIL, 'synthetic mode failure'); END;").unwrap();
        let switch = crate::rt::block_on(runtime.proxy_service.lock_switch_for_app(app.as_str()));
        let result = crate::mode::controller::exit_locked(&runtime.proxy_service, &app, false);
        drop(switch);
        runtime
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER synthetic_mode_failure;")
            .unwrap();
        assert!(result.is_err());
        let view = review.review_app(&inspected, &token, &app).unwrap();
        assert!(view.can_recover_operation, "{app:?}");
        let recovered = review
            .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
            .unwrap();
        assert_eq!(recovered.has_pending_operation, Some(false));
        let mode = crate::mode::state::mode_state(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            app.as_str(),
        )
        .unwrap();
        assert_eq!(mode.mode, Some(crate::mode::state::Mode::Direct));
        assert!(!mode.attached);
        assert_eq!(mode.proxy_route.as_deref(), Some("b"));
        assert_eq!(
            runtime.db.get_proxy_flags_checked(app.as_str()).unwrap(),
            (false, true)
        );
        assert_native_result(&app, "a");
        println!("PASS detached mode partial commit {app:?}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovery_requires_committed_written() {
    let f = Fixture::new();
    let app = AppType::Codex;
    let (inspected, review, token, runtime) = native_runtime(&f, &app);
    interrupt_switch(&runtime, &app, "marked");
    let view = review.review_app(&inspected, &token, &app).unwrap();
    let db = runtime.db.clone();
    let home = f.home.path().to_path_buf();
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let captured = seen.clone();
    operation::failpoint::on_boundary(Some(Box::new(move |at| {
        if at != "recover:verified" || captured.borrow().is_some() {
            return;
        }
        crate::mode::state::update_app(
            &DeviceStore::for_device(),
            &db.secret_session().read().unwrap(),
            "codex",
            |entry| {
                assert!(entry.written.is_some());
                entry.written = None;
                Ok(())
            },
        )
        .unwrap();
        *captured.borrow_mut() = Some(snapshot(&home));
    })));
    let result = review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
    operation::failpoint::on_boundary(None);
    assert!(result.is_err());
    assert!(
        snapshot(f.home.path()) == *seen.borrow().as_ref().unwrap(),
        "Written drift must retain every observed byte"
    );
    assert!(crate::mode::state::pending(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "codex"
    )
    .unwrap()
    .is_some());
}
