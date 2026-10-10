//! Retained upgrade recovery must restore the original real listener dependency.
use super::*;
use crate::{app_config::AppType, mode::operation};

fn health(port: u16) {
    use std::io::{Read, Write};
    let mut client = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_secs(3),
    )
    .unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "real ProxyServer health failed: {response}"
    );
}

fn assert_no_recovery_writes(
    before: &std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    after: &std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    label: &str,
) {
    let mut observed = after.clone();
    // SQLite WAL readers may advance aReadMark[0..5] at bytes 100..120.
    // Keep DB/WAL/journal, both WAL-index headers, backfill counters and every
    // other SHM byte exact; a reader mark is not a SQL or native-file write.
    let shm = std::path::PathBuf::from(format!(".loongport/{}-shm", crate::config::DB_FILE_NAME));
    if let (Some(old), Some(now)) = (before.get(&shm), observed.get_mut(&shm)) {
        assert_eq!(old.len(), now.len(), "{label}: SHM length changed");
        if old.len() >= 120 {
            now[100..120].copy_from_slice(&old[100..120]);
        }
    }
    let changed: Vec<_> = before
        .keys()
        .chain(observed.keys())
        .filter(|path| before.get(*path) != observed.get(*path))
        .collect();
    assert!(
        before == &observed,
        "{label}: rejected recovery wrote {changed:?}"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovers_enter_listener_after_restart() {
    let f = Fixture::new();
    let app = AppType::Claude;
    let reactor = tokio::runtime::Runtime::new().unwrap();
    let (inspected, review, token, runtime) =
        super::native_recovery_tests::native_runtime(&f, &app);
    let mut global = reactor
        .block_on(runtime.db.get_global_proxy_config())
        .unwrap();
    global.listen_address = "127.0.0.1".into();
    global.listen_port = 0;
    reactor
        .block_on(runtime.db.update_global_proxy_config(global))
        .unwrap();
    operation::failpoint::crash_at(Some("marked"));
    let result = reactor.block_on(runtime.proxy_service.set_takeover_for_app("claude", true));
    operation::failpoint::crash_at(None);
    assert!(result.is_err());
    let pending = crate::mode::state::pending(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "claude",
    )
    .unwrap()
    .expect("original ENTER intent");
    assert_eq!(pending.op, crate::mode::state::op::ENTER);
    assert!(pending.target.state.as_ref().unwrap().attached);
    let port = reactor
        .block_on(runtime.db.get_global_proxy_config())
        .unwrap()
        .listen_port;
    assert_ne!(port, 0);
    health(port);
    reactor.block_on(runtime.proxy_service.stop()).unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    let restarted = crate::store::AppState::new(runtime.db.clone()).unwrap();
    let view = review.review_app(&inspected, &token, &app).unwrap();
    let before = snapshot(f.home.path());
    let context = reactor.enter();
    let recovered =
        review.recover_app_with_state(&inspected, &token, &app, &view.revision, &restarted);
    drop(context);
    let running = reactor.block_on(restarted.proxy_service.is_running());
    if running {
        health(port);
        reactor.block_on(restarted.proxy_service.stop()).unwrap();
    }
    assert!(
        view.can_recover_operation,
        "original ENTER target requires controlled listener recovery"
    );
    let recovered = recovered.unwrap();
    assert_eq!(recovered.has_pending_operation, Some(false));
    assert!(running, "cleared attached journal without its listener");
    assert_ne!(snapshot(f.home.path()), before);
    let mode = crate::mode::state::mode_state(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "claude",
    )
    .unwrap();
    assert!(mode.attached);
    assert_eq!(mode.proxy_route.as_deref(), Some("a"));
}

fn interrupted(
    f: &Fixture,
    app: &AppType,
    kind: &str,
    point: &str,
    reactor: &tokio::runtime::Runtime,
) -> (
    UpgradeInspection,
    AuthenticatedUpgrade,
    String,
    crate::store::AppState,
    u16,
) {
    let (inspected, review, token, runtime) = super::native_recovery_tests::native_runtime(f, app);
    let mut global = reactor
        .block_on(runtime.db.get_global_proxy_config())
        .unwrap();
    global.listen_address = "127.0.0.1".into();
    global.listen_port = 0;
    reactor
        .block_on(runtime.db.update_global_proxy_config(global))
        .unwrap();
    if kind == crate::mode::state::op::ATTACH {
        crate::mode::state::update_app(
            &f.device,
            &runtime.db.secret_session().read().unwrap(),
            app.as_str(),
            |entry| {
                entry
                    .set_mode_state(crate::mode::state::ModeState {
                        mode: Some(crate::mode::state::Mode::Proxy),
                        attached: false,
                        proxy_route: Some("a".into()),
                        ..Default::default()
                    })
                    .unwrap();
                Ok(())
            },
        )
        .unwrap();
        runtime
            .db
            .set_proxy_flags_sync(app.as_str(), true, false)
            .unwrap();
    }
    operation::failpoint::crash_at(Some(point));
    let result = reactor.block_on(async {
        let _locks = runtime
            .proxy_service
            .lock_recovery_for_app(app.as_str())
            .await;
        crate::mode::controller::enter_locked(
            &runtime.proxy_service,
            app,
            if kind == crate::mode::state::op::ATTACH {
                crate::mode::state::op::ATTACH
            } else {
                crate::mode::state::op::ENTER
            },
        )
        .await
    });
    operation::failpoint::crash_at(None);
    let pending = crate::mode::state::pending(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        app.as_str(),
    )
    .unwrap();
    assert!(
        result.is_err() && pending.is_some(),
        "{app:?}/{kind}/{point}: did not reach original intent: {result:?}"
    );
    let pending = pending.unwrap();
    assert_eq!(pending.op, kind);
    assert!(pending.target.state.as_ref().unwrap().contract.is_some());
    let port = reactor
        .block_on(runtime.db.get_global_proxy_config())
        .unwrap()
        .listen_port;
    health(port);
    (inspected, review, token, runtime, port)
}

fn pending_exists(f: &Fixture, runtime: &crate::store::AppState, app: &AppType) -> bool {
    crate::mode::state::pending(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        app.as_str(),
    )
    .unwrap()
    .is_some()
}

fn stop_listener(reactor: &tokio::runtime::Runtime, runtime: &crate::store::AppState, port: u16) {
    if reactor.block_on(runtime.proxy_service.is_running()) {
        reactor.block_on(runtime.proxy_service.stop()).unwrap();
    }
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "owned listener was not released"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovers_original_attached_matrix() {
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        let partial = if app == AppType::Codex {
            "published:1"
        } else {
            "published:0"
        };
        for (kind, point) in [
            (crate::mode::state::op::ENTER, "pending"),
            (crate::mode::state::op::ENTER, "marked"),
            (crate::mode::state::op::ENTER, partial),
            (crate::mode::state::op::ENTER, "target"),
            (crate::mode::state::op::ATTACH, "marked"),
        ] {
            let f = Fixture::new();
            let reactor = tokio::runtime::Runtime::new().unwrap();
            let (inspected, review, token, runtime, port) =
                interrupted(&f, &app, kind, point, &reactor);
            let (reactor, runtime) = if point == partial {
                // Abrupt runtime loss leaves the persisted enabled bit true.
                // Recreate the actual service, rather than calling stop first.
                let db = runtime.db.clone();
                drop(reactor);
                drop(runtime);
                assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
                let reactor = tokio::runtime::Runtime::new().unwrap();
                assert!(
                    reactor
                        .block_on(db.get_global_proxy_config())
                        .unwrap()
                        .proxy_enabled
                );
                (reactor, crate::store::AppState::new(db).unwrap())
            } else {
                stop_listener(&reactor, &runtime, port);
                let runtime = crate::store::AppState::new(runtime.db.clone()).unwrap();
                (reactor, runtime)
            };
            let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
            let view = review.review_app(&inspected, &token, &app).unwrap();
            let context = reactor.enter();
            let result =
                review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
            drop(context);
            let running = reactor
                .block_on(runtime.proxy_service.get_status())
                .unwrap()
                .running;
            if running {
                health(port);
            }
            stop_listener(&reactor, &runtime, port);
            let recovered =
                result.unwrap_or_else(|error| panic!("{app:?}/{kind}/{point}: {error}"));
            assert_eq!(recovered.has_pending_operation, Some(false));
            assert_eq!(
                running,
                point != "pending",
                "{app:?}/{kind}/{point}: listener dependency"
            );
            let mode = crate::mode::state::mode_state(
                &f.device,
                &runtime.db.secret_session().read().unwrap(),
                app.as_str(),
            )
            .unwrap();
            assert_eq!(mode.attached, point != "pending");
            assert_eq!(
                std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
                checkpoint
            );
            let before = snapshot(f.home.path());
            let context = reactor.enter();
            assert!(review
                .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
                .is_err());
            drop(context);
            assert!(snapshot(f.home.path()) == before, "stale retry wrote");
            println!("PASS attached matrix {app:?}/{kind}/{point}");
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_attached_recovery_refuses_endpoint_changes() {
    for effect in [
        "occupied",
        "zero",
        "wildcard",
        "changed-port",
        "stale-port",
        "running-mismatch",
    ] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let (inspected, review, token, runtime, port) =
            interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
        let original = review.review_app(&inspected, &token, &app).unwrap();
        if effect != "running-mismatch" {
            stop_listener(&reactor, &runtime, port);
        }
        let occupied = (effect == "occupied")
            .then(|| std::net::TcpListener::bind(("127.0.0.1", port)).unwrap());
        if matches!(
            effect,
            "zero" | "wildcard" | "changed-port" | "stale-port" | "running-mismatch"
        ) {
            let mut config = reactor
                .block_on(runtime.db.get_global_proxy_config())
                .unwrap();
            if effect == "zero" {
                config.listen_port = 0;
            } else if effect == "wildcard" {
                config.listen_address = "0.0.0.0".into();
            } else {
                config.listen_port = if port == u16::MAX { port - 1 } else { port + 1 };
            }
            reactor
                .block_on(runtime.db.update_global_proxy_config(config))
                .unwrap();
        }
        let view = if effect == "stale-port" {
            original
        } else {
            review.review_app(&inspected, &token, &app).unwrap()
        };
        let before = snapshot(f.home.path());
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        drop(context);
        assert!(result.is_err(), "{effect}: unproven endpoint accepted");
        let after = snapshot(f.home.path());
        assert_no_recovery_writes(&before, &after, effect);
        assert!(pending_exists(&f, &runtime, &app));
        if effect == "running-mismatch" {
            health(port);
        }
        drop(occupied);
        stop_listener(&reactor, &runtime, port);
        println!("PASS endpoint boundary {effect}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_attached_recovery_retries_and_cleans_owned_listener() {
    for existing in [false, true] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let (inspected, review, token, runtime, port) =
            interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
        if !existing {
            stop_listener(&reactor, &runtime, port);
        }
        let view = review.review_app(&inspected, &token, &app).unwrap();
        let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        operation::failpoint::crash_at(Some("upgrade:native_recovery_owner"));
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        drop(context);
        operation::failpoint::crash_at(None);
        assert!(result.is_err());
        assert!(pending_exists(&f, &runtime, &app));
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            checkpoint
        );
        assert_eq!(
            reactor
                .block_on(runtime.proxy_service.get_status())
                .unwrap()
                .running,
            existing
        );
        if existing {
            health(port);
        } else {
            assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
        }
        let fresh = review.review_app(&inspected, &token, &app).unwrap();
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &fresh.revision, &runtime);
        drop(context);
        let running = reactor
            .block_on(runtime.proxy_service.get_status())
            .unwrap()
            .running;
        if running {
            health(port);
        }
        stop_listener(&reactor, &runtime, port);
        assert_eq!(result.unwrap().has_pending_operation, Some(false));
        assert!(running);
        println!("PASS listener retry existing={existing}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_attached_recovery_rejects_dead_accept_loop() {
    let f = Fixture::new();
    let app = AppType::Claude;
    let reactor = tokio::runtime::Runtime::new().unwrap();
    let (inspected, review, token, runtime, port) =
        interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
    drop(reactor); // The actual socket task is gone; the service slot remains.
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    let reactor = tokio::runtime::Runtime::new().unwrap();
    let view = review.review_app(&inspected, &token, &app).unwrap();
    let before = snapshot(f.home.path());
    let context = reactor.enter();
    let result = review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
    drop(context);
    assert!(
        result.is_err(),
        "dead accept loop borrowed stale running status"
    );
    assert!(snapshot(f.home.path()) == before);
    assert!(pending_exists(&f, &runtime, &app));
    let _ = reactor.block_on(runtime.proxy_service.stop());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovers_attached_exit_and_detach() {
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        for (keep_mode, point) in [(false, "pending"), (false, "marked"), (true, "marked")] {
            let f = Fixture::new();
            let reactor = tokio::runtime::Runtime::new().unwrap();
            let (inspected, review, token, runtime, port) =
                interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
            let view = review.review_app(&inspected, &token, &app).unwrap();
            let context = reactor.enter();
            review
                .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
                .unwrap();
            drop(context);
            // Create an original pre-upgrade attached exit journal. Ordinary
            // writes remain refused with a retained checkpoint; recovery does
            // not relax that gate. Restore the identical fixture checkpoint
            // before any review/recovery below.
            let checkpoint_path = f.device.root().join(checkpoint::FILE);
            let parked = checkpoint_path.with_extension("synthetic-parked");
            std::fs::rename(&checkpoint_path, &parked).unwrap();
            operation::failpoint::crash_at(Some(point));
            let result = {
                let _locks =
                    reactor.block_on(runtime.proxy_service.lock_recovery_for_app(app.as_str()));
                // This is the synchronous owner; do not wrap its Codex bridge
                // in another Runtime::block_on (the GUI uses a blocking worker).
                crate::mode::controller::exit_locked(&runtime.proxy_service, &app, keep_mode)
            };
            operation::failpoint::crash_at(None);
            std::fs::rename(&parked, &checkpoint_path).unwrap();
            assert!(
                result.is_err() && pending_exists(&f, &runtime, &app),
                "exit fault missed: {result:?}"
            );
            stop_listener(&reactor, &runtime, port);
            let runtime = crate::store::AppState::new(runtime.db.clone()).unwrap();
            let view = review.review_app(&inspected, &token, &app).unwrap();
            let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
            let context = reactor.enter();
            let result =
                review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
            drop(context);
            let running = reactor
                .block_on(runtime.proxy_service.get_status())
                .unwrap()
                .running;
            if running {
                health(port);
            }
            stop_listener(&reactor, &runtime, port);
            let recovered =
                result.unwrap_or_else(|error| panic!("{app:?}/{keep_mode}/{point}: {error}"));
            assert_eq!(recovered.has_pending_operation, Some(false));
            let mode = crate::mode::state::mode_state(
                &f.device,
                &runtime.db.secret_session().read().unwrap(),
                app.as_str(),
            )
            .unwrap();
            assert_eq!(mode.attached, point == "pending");
            assert_eq!(mode.is_proxy(), keep_mode || point == "pending");
            assert_eq!(running, point == "pending");
            assert_eq!(
                std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
                checkpoint
            );
            println!("PASS exit matrix {app:?}/{keep_mode}/{point}");
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_listener_start_rechecks_all_reviewed_facts() {
    for (point, effect) in [
        ("upgrade:listener_start", "database"),
        ("upgrade:listener_start", "endpoint"),
        ("upgrade:listener_bound", "checkpoint"),
        ("upgrade:listener_bound", "row"),
        ("upgrade:listener_bound", "staged"),
        ("upgrade:listener_bound", "native"),
        ("upgrade:listener_bound", "settings"),
    ] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let (inspected, review, token, runtime, port) =
            interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
        stop_listener(&reactor, &runtime, port);
        let view = review.review_app(&inspected, &token, &app).unwrap();
        let db = runtime.db.clone();
        let home = f.home.path().to_path_buf();
        let root = f.root.clone();
        let checkpoint_path = f.device.root().join(checkpoint::FILE);
        let pending = crate::mode::state::pending(
            &f.device,
            &db.secret_session().read().unwrap(),
            app.as_str(),
        )
        .unwrap()
        .unwrap();
        let staged = pending
            .files
            .iter()
            .find_map(|file| file.staged.clone())
            .unwrap();
        let observed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let seen = observed.clone();
        operation::failpoint::on_boundary(Some(Box::new(move |at| {
            if at != point || seen.borrow().is_some() {
                return;
            }
            match effect {
                "database" => {
                    let path = root.join(crate::config::DB_FILE_NAME);
                    let old = path.with_extension("synthetic-replaced");
                    std::fs::rename(&path, &old).unwrap();
                    std::fs::copy(&old, &path).unwrap();
                }
                "endpoint" => {
                    db.conn
                        .lock()
                        .unwrap()
                        .execute("UPDATE proxy_config SET listen_port=listen_port+1", [])
                        .unwrap();
                }
                "checkpoint" => {
                    let mut bytes = std::fs::read(&checkpoint_path).unwrap();
                    bytes.push(b'\n');
                    std::fs::write(&checkpoint_path, bytes).unwrap();
                }
                "row" => {
                    let mut row = db.get_provider_by_id("b", "claude").unwrap().unwrap();
                    row.name = "Synthetic drift".into();
                    db.save_provider("claude", &row).unwrap();
                }
                "staged" => std::fs::write(&staged, b"synthetic staged drift").unwrap(),
                "native" => std::fs::write(
                    crate::config::get_claude_settings_path(),
                    br#"{"synthetic":"external-native"}"#,
                )
                .unwrap(),
                "settings" => {
                    let path = crate::config::get_app_config_dir().join("settings.json");
                    let mut settings: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    settings["currentProviderClaude"] = serde_json::json!("b");
                    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
                }
                _ => unreachable!(),
            }
            *seen.borrow_mut() = Some(snapshot(&home));
        })));
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        drop(context);
        operation::failpoint::on_boundary(None);
        assert!(result.is_err(), "{point}/{effect}: drift accepted");
        assert!(
            observed.borrow().is_some(),
            "{point}/{effect}: boundary missed"
        );
        assert_no_recovery_writes(
            observed.borrow().as_ref().unwrap(),
            &snapshot(f.home.path()),
            effect,
        );
        assert!(pending_exists(&f, &runtime, &app));
        stop_listener(&reactor, &runtime, port);
        println!("PASS listener drift {point}/{effect}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_recovers_attached_route_and_row_owners() {
    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ] {
        for effect in ["route", "save"] {
            let f = Fixture::new();
            let reactor = tokio::runtime::Runtime::new().unwrap();
            let (inspected, review, token, runtime, port) =
                interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
            let view = review.review_app(&inspected, &token, &app).unwrap();
            let context = reactor.enter();
            review
                .recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime)
                .unwrap();
            drop(context);
            let checkpoint_path = f.device.root().join(checkpoint::FILE);
            let checkpoint = std::fs::read(&checkpoint_path).unwrap();
            let parked = checkpoint_path.with_extension("synthetic-parked");
            std::fs::rename(&checkpoint_path, &parked).unwrap();
            let previous = runtime
                .db
                .get_provider_by_id("a", app.as_str())
                .unwrap()
                .unwrap();
            let mut planned = runtime
                .db
                .get_provider_by_id("b", app.as_str())
                .unwrap()
                .unwrap();
            if effect == "save" {
                planned.id = "a".into();
            }
            operation::failpoint::crash_at(Some("marked"));
            let result = {
                let _locks =
                    reactor.block_on(runtime.proxy_service.lock_recovery_for_app(app.as_str()));
                if effect == "route" {
                    reactor.block_on(crate::mode::controller::switch_route_locked(
                        &runtime.proxy_service,
                        &app,
                        planned.clone(),
                    ))
                } else {
                    crate::mode::controller::save_row_locked(
                        &runtime.proxy_service,
                        &app,
                        &previous,
                        &planned,
                        false,
                    )
                    .map_err(|error| error.to_string())
                }
            };
            operation::failpoint::crash_at(None);
            std::fs::rename(&parked, &checkpoint_path).unwrap();
            assert!(
                result.is_err() && pending_exists(&f, &runtime, &app),
                "{app:?}/{effect}: original journal missed: {result:?}"
            );
            stop_listener(&reactor, &runtime, port);
            let runtime = crate::store::AppState::new(runtime.db.clone()).unwrap();
            let view = review.review_app(&inspected, &token, &app).unwrap();
            let context = reactor.enter();
            let result =
                review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
            drop(context);
            let running = reactor
                .block_on(runtime.proxy_service.get_status())
                .unwrap()
                .running;
            if running {
                health(port);
            }
            stop_listener(&reactor, &runtime, port);
            let recovered = result.unwrap_or_else(|error| panic!("{app:?}/{effect}: {error}"));
            assert_eq!(recovered.has_pending_operation, Some(false));
            assert!(running);
            let mode = crate::mode::state::mode_state(
                &f.device,
                &runtime.db.secret_session().read().unwrap(),
                app.as_str(),
            )
            .unwrap();
            assert!(mode.attached && mode.is_proxy());
            assert_eq!(
                mode.proxy_route.as_deref(),
                Some(if effect == "route" { "b" } else { "a" })
            );
            assert_eq!(
                runtime
                    .db
                    .get_current_provider(app.as_str())
                    .unwrap()
                    .as_deref(),
                Some("a")
            );
            if effect == "save" {
                let actual = runtime
                    .db
                    .get_provider_by_id("a", app.as_str())
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    Database::provider_update_digest(&actual).unwrap(),
                    Database::provider_update_digest(&planned).unwrap()
                );
            }
            assert_eq!(std::fs::read(&checkpoint_path).unwrap(), checkpoint);
            println!("PASS attached owner {app:?}/{effect}");
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_listener_binds_only_reviewed_config() {
    let f = Fixture::new();
    let app = AppType::Claude;
    let reactor = tokio::runtime::Runtime::new().unwrap();
    let (inspected, review, token, runtime, port) =
        interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
    stop_listener(&reactor, &runtime, port);
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let alternate = reservation.local_addr().unwrap().port();
    assert_ne!(alternate, port);
    drop(reservation);
    let view = review.review_app(&inspected, &token, &app).unwrap();
    let bound = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = bound.clone();
    let db = runtime.db.clone();
    operation::failpoint::on_boundary(Some(Box::new(move |at| {
        if at == "upgrade:listener_config" {
            db.conn
                .lock()
                .unwrap()
                .execute("UPDATE proxy_config SET listen_port=?1", [alternate])
                .unwrap();
        }
        if at == "upgrade:listener_bound" {
            observed.set(true);
        }
    })));
    let context = reactor.enter();
    let result = review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
    drop(context);
    operation::failpoint::on_boundary(None);
    assert!(result.is_err());
    assert!(pending_exists(&f, &runtime, &app));
    assert!(std::net::TcpStream::connect(("127.0.0.1", alternate)).is_err());
    assert!(
        !bound.get(),
        "actual unread config was bound before verification"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_listener_never_stops_a_concurrent_start() {
    for mismatch in [false, true] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let (inspected, review, token, runtime, port) =
            interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
        stop_listener(&reactor, &runtime, port);
        let concurrent_port = if mismatch {
            let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            reservation.local_addr().unwrap().port()
        } else {
            port
        };
        let view = review.review_app(&inspected, &token, &app).unwrap();
        let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let db = runtime.db.clone();
        let service = runtime.proxy_service.clone();
        let handle = reactor.handle().clone();
        let started = std::rc::Rc::new(std::cell::Cell::new(false));
        let observed = started.clone();
        operation::failpoint::on_boundary(Some(Box::new(move |at| {
            if at == "upgrade:listener_config" && !observed.replace(true) {
                if mismatch {
                    db.conn
                        .lock()
                        .unwrap()
                        .execute("UPDATE proxy_config SET listen_port=?1", [concurrent_port])
                        .unwrap();
                }
                let service = service.clone();
                let handle = handle.clone();
                std::thread::spawn(move || handle.block_on(service.start()))
                    .join()
                    .unwrap()
                    .unwrap();
                health(concurrent_port);
                if mismatch {
                    db.conn
                        .lock()
                        .unwrap()
                        .execute("UPDATE proxy_config SET listen_port=?1", [port])
                        .unwrap();
                }
            }
        })));
        operation::failpoint::crash_at(Some("upgrade:native_recovery_owner"));
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        drop(context);
        operation::failpoint::on_boundary(None);
        operation::failpoint::crash_at(None);
        let running = reactor
            .block_on(runtime.proxy_service.get_status())
            .unwrap()
            .running;
        if running {
            health(concurrent_port);
        }
        stop_listener(&reactor, &runtime, concurrent_port);
        assert!(result.is_err() && started.get());
        assert!(pending_exists(&f, &runtime, &app));
        assert!(running, "recovery stopped another start owner's listener");
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            checkpoint
        );
        println!("PASS concurrent listener owner mismatch={mismatch}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn retained_checkpoint_listener_rejects_late_disabled_mirror() {
    for point in ["recover:begin", "recover:target", "recover:verified"] {
        let f = Fixture::new();
        let app = AppType::Claude;
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let (inspected, review, token, runtime, port) =
            interrupted(&f, &app, crate::mode::state::op::ENTER, "marked", &reactor);
        stop_listener(&reactor, &runtime, port);
        let view = review.review_app(&inspected, &token, &app).unwrap();
        let checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let db = runtime.db.clone();
        let seen = std::rc::Rc::new(std::cell::Cell::new(false));
        let observed = seen.clone();
        operation::failpoint::on_boundary(Some(Box::new(move |at| {
            if at == point && !observed.replace(true) {
                let sql = if point == "recover:target" {
                    "UPDATE proxy_config SET proxy_enabled=0 WHERE app_type='claude'"
                } else {
                    "UPDATE proxy_config SET proxy_enabled=0"
                };
                db.conn.lock().unwrap().execute(sql, []).unwrap();
            }
        })));
        let context = reactor.enter();
        let result =
            review.recover_app_with_state(&inspected, &token, &app, &view.revision, &runtime);
        drop(context);
        operation::failpoint::on_boundary(None);
        stop_listener(&reactor, &runtime, port);
        assert!(seen.get(), "{point}: late enabled boundary missed");
        assert!(
            result.is_err(),
            "{point}: reverted global enabled bit accepted"
        );
        assert!(pending_exists(&f, &runtime, &app));
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            checkpoint
        );
        println!("PASS late global enabled {point}");
    }
}
