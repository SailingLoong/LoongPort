//! Test-only real native app journal and group owner in the integration test HOME.
use crate::mode::{operation, state};
use crate::services::{routing_groups as groups, shared_routing as owner};
use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
    store::AppState,
};
use serde_json::json;

pub async fn verify(app_state: &AppState, case: &str) {
    let app = if matches!(
        case,
        "codex_recovery"
            | "codex_client_dirty"
            | "bound_detached_codex"
            | "official_bound"
            | "official_direct"
    ) {
        AppType::Codex
    } else if case == "gemini_confirm" {
        AppType::Gemini
    } else if case == "grok_confirm" {
        AppType::GrokBuild
    } else {
        AppType::Claude
    };
    let expected = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("cc-switch-test-home-{}", std::process::id()));
    assert_eq!(
        std::env::var_os("CC_SWITCH_TEST_HOME").map(std::path::PathBuf::from),
        Some(expected.clone())
    );
    assert_eq!(
        app_state.db.secret_session().root(),
        expected.join(crate::config::APP_DIR_NAME)
    );
    let db = &app_state.db;
    let a = crate::relay::managed::provider_id_for("https://route-a.example", Some(11), 1);
    let b = crate::relay::managed::provider_id_for("https://route-b.example", Some(22), 1);
    db.conn.lock().unwrap().execute("INSERT INTO loongport_relay(id,site_origin,account_id) VALUES(1,'https://route-a.example',11),(2,'https://route-b.example',22)",[]).unwrap();
    for (id, site, account, model) in [
        (&a, "https://route-a.example", 11, "model-a"),
        (&b, "https://route-b.example", 22, "model-b"),
    ] {
        let mut provider = Provider::with_id(
            id.clone(),
            "Synthetic".into(),
            crate::relay::provider_config::settings_config_for(
                &app,
                "synthetic-key",
                "Synthetic",
                site,
                model,
            )
            .unwrap(),
            Some(site.into()),
        );
        provider.meta = Some(ProviderMeta {
            loongport_account_id: Some(account),
            ..Default::default()
        });
        db.save_provider(app.as_str(), &provider).unwrap();
    }
    let draft = owner::GroupDraft {
        id: None,
        expected_revision: None,
        name: "Daily".into(),
        members: vec![
            groups::Member {
                source_id: a.clone(),
                model: "model-a".into(),
            },
            groups::Member {
                source_id: b.clone(),
                model: "model-b".into(),
            },
        ],
    };
    let prepared_group =
        tokio::task::block_in_place(|| owner::prepare_group(app_state, draft)).unwrap();
    if case == "group_source_changed" {
        let mut provider = db.get_provider_by_id(&a, app.as_str()).unwrap().unwrap();
        provider.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!("https://changed.example");
        db.save_provider(app.as_str(), &provider).unwrap();
        assert!(tokio::task::block_in_place(|| owner::confirm_group(
            app_state,
            prepared_group.clone()
        ))
        .is_err());
        assert!(
            groups::read(&db.conn.lock().unwrap(), &prepared_group.group.id)
                .unwrap()
                .is_none()
        );
        return;
    }
    #[cfg(feature = "test-hooks")]
    if case == "group_atomic" {
        let (hook, blocked) = source_writer_hook(app_state, &a, "shared_group:validated");
        let result = tokio::task::block_in_place(|| {
            operation::failpoint::on_boundary(Some(hook));
            let result = owner::confirm_group(app_state, prepared_group.clone());
            operation::failpoint::on_boundary(None);
            result
        });
        assert!(
            blocked.load(std::sync::atomic::Ordering::SeqCst),
            "second native DB connection wrote source after evidence validation"
        );
        assert_eq!(
            db.get_provider_by_id(&a, app.as_str())
                .unwrap()
                .unwrap()
                .name,
            "Synthetic"
        );
        assert_eq!(result.unwrap(), prepared_group.group);
        return;
    }
    let group =
        tokio::task::block_in_place(|| owner::confirm_group(app_state, prepared_group.clone()))
            .unwrap();
    assert_eq!(
        tokio::task::block_in_place(|| owner::confirm_group(app_state, prepared_group)).unwrap(),
        group
    );
    let manual = groups::RouteTarget::Manual {
        source_id: a.clone(),
        model: "model-a".into(),
    };
    assert!(
        tokio::task::block_in_place(|| owner::prepare_target(app_state, &app, manual.clone()))
            .is_err(),
        "direct/disconnected cannot silently become gateway routing"
    );
    db.set_current_provider(app.as_str(), &a).unwrap();
    crate::settings::set_current_provider(&app, Some(&a)).unwrap();
    let mut config = db.get_proxy_config().await.unwrap();
    config.listen_port = 0;
    config.listen_address = "127.0.0.1".into();
    db.update_proxy_config(config).await.unwrap();
    app_state
        .proxy_service
        .set_takeover_for_app(app.as_str(), true)
        .await
        .unwrap();
    let files = crate::mode::controller::files(&app).unwrap();
    let bytes = || {
        files
            .iter()
            .map(|f| std::fs::read(&f.path).ok())
            .collect::<Vec<_>>()
    };
    let before_bytes = bytes();
    if case == "prepare_client_dirty" {
        let (path, original, changed) = dirty_gateway_file(&files);
        std::fs::write(&path, &changed).unwrap();
        let result =
            tokio::task::block_in_place(|| owner::prepare_target(app_state, &app, manual.clone()));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            changed,
            "preview may not repair external client edits"
        );
        std::fs::write(&path, original).unwrap();
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        assert!(
            result.is_err(),
            "dirty attached URL must not be adopted as a fresh gateway baseline"
        );
        return;
    }

    let legacy_change = crate::services::application_selection::ApplicationRoutingChange {
        selection: Some(crate::services::application_selection::TierSelection {
            provider_id: b.clone(),
            model: None,
        }),
        order: None,
        catalog_decision: None,
        catalog_preview_only: false,
    };
    if case == "unbound_legacy" {
        use crate::services::application_selection as legacy;
        legacy::initialize_manual_routing(app_state, &app)
            .await
            .unwrap();
        tokio::task::block_in_place(|| {
            legacy::select_with_commit(
                app_state,
                &app,
                legacy_change.selection.as_ref().unwrap(),
                None,
            )
        })
        .unwrap();
        app_state
            .proxy_service
            .set_failover_for_app(app.as_str(), true)
            .await
            .unwrap();
        app_state
            .proxy_service
            .set_failover_for_app(app.as_str(), false)
            .await
            .unwrap();
        tokio::task::block_in_place(|| {
            legacy::apply_current_order_change(app_state, &app, &[b.clone(), a.clone()])
        })
        .unwrap();
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(app_state, app.clone(), &a)
        })
        .unwrap();
        assert!(groups::binding(&db.conn.lock().unwrap(), app.as_str())
            .unwrap()
            .is_none());
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        return;
    }
    let legacy_request = if case == "bound_legacy_receipt" {
        match tokio::task::block_in_place(|| {
            crate::services::application_selection::prepare_routing_strategy(
                app_state,
                &app,
                &legacy_change,
                &uuid::Uuid::new_v4().to_string(),
            )
        })
        .unwrap()
        {
            crate::services::application_selection::PreparedRoutingRequest::Native { request } => {
                Some(request)
            }
            _ => panic!("native fixture must use original native receipt"),
        }
    } else {
        None
    };
    let prepared =
        tokio::task::block_in_place(|| owner::prepare_target(app_state, &app, manual.clone()))
            .unwrap();
    let request = prepared.request.clone();
    if case == "target_source_changed" {
        let original = db.get_provider_by_id(&a, app.as_str()).unwrap().unwrap();
        let mut changed = original.clone();
        changed.name = "Changed before confirmation".into();
        db.save_provider(app.as_str(), &changed).unwrap();
        let result =
            tokio::task::block_in_place(|| owner::apply_target(app_state, &app, prepared.clone()));
        let binding = groups::binding(&db.conn.lock().unwrap(), app.as_str()).unwrap();
        assert_eq!(bytes(), before_bytes);
        db.save_provider(app.as_str(), &original).unwrap();
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        assert!(result.is_err());
        assert!(binding.is_none());
        return;
    }

    #[cfg(feature = "test-hooks")]
    if case == "target_atomic" {
        let (hook, blocked) = source_writer_hook(app_state, &a, "route_target:validated");
        let result = tokio::task::block_in_place(|| {
            operation::failpoint::on_boundary(Some(hook));
            let result = owner::apply_target(app_state, &app, prepared.clone());
            operation::failpoint::on_boundary(None);
            result
        });
        let source_name = db
            .get_provider_by_id(&a, app.as_str())
            .unwrap()
            .unwrap()
            .name;
        if !blocked.load(std::sync::atomic::Ordering::SeqCst) {
            let mut original = db.get_provider_by_id(&a, app.as_str()).unwrap().unwrap();
            original.name = "Synthetic".into();
            db.save_provider(app.as_str(), &original).unwrap();
            let _ = tokio::task::block_in_place(|| {
                owner::recover_target(app_state, &app, request.clone())
            });
        }
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        assert!(
            blocked.load(std::sync::atomic::Ordering::SeqCst),
            "second connection changed source between target validation and CAS"
        );
        assert_eq!(source_name, "Synthetic");
        assert_eq!(result.unwrap().status, "completed");
        return;
    }

    #[cfg(feature = "test-hooks")]
    if matches!(case, "journal_client_dirty" | "codex_client_dirty") {
        let interrupted = tokio::task::block_in_place(|| {
            operation::failpoint::crash_at(Some("target"));
            let result = owner::apply_target(app_state, &app, prepared.clone());
            operation::failpoint::crash_at(None);
            result
        });
        assert!(interrupted.is_err());
        assert_eq!(
            tokio::task::block_in_place(|| owner::read_state(app_state, &app))
                .unwrap()
                .pending_request,
            Some(request.clone())
        );
        let (path, original, changed) = dirty_gateway_file(&files);
        std::fs::write(&path, &changed).unwrap();
        let result =
            tokio::task::block_in_place(|| owner::recover_target(app_state, &app, request.clone()));
        let still_pending = tokio::task::block_in_place(|| owner::read_state(app_state, &app))
            .unwrap()
            .pending_request;
        let actual = std::fs::read(&path).unwrap();
        std::fs::write(&path, original).unwrap();
        let recovery =
            tokio::task::block_in_place(|| owner::recover_target(app_state, &app, request.clone()));
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        assert!(
            result.is_err() || result.as_ref().is_ok_and(|r| r.status != "completed"),
            "external URL change must block pending completion"
        );
        assert_eq!(still_pending, Some(request));
        assert_eq!(
            actual, changed,
            "recovery must never overwrite external client edit"
        );
        assert_eq!(recovery.unwrap().status, "completed");
        return;
    }

    if matches!(
        case,
        "journal_recovery" | "codex_recovery" | "journal_source_changed"
    ) {
        let _switch = app_state
            .proxy_service
            .lock_switch_for_app(app.as_str())
            .await;
        let publication =
            tokio::task::block_in_place(|| owner::prepare_publication(app_state, &app, &prepared))
                .unwrap();
        let target = state::PendingTarget {
            save_request: Some(request.clone()),
            route_target: Some(publication),
            ..Default::default()
        };
        let plan = tokio::task::block_in_place(|| {
            crate::mode::controller::gateway_route_plan(&app_state.proxy_service, &app, &request.id)
        })
        .unwrap();
        let changes = plan
            .files
            .iter()
            .map(|(file, patch)| operation::FileChange {
                file: file.clone(),
                patch: patch as &dyn crate::live::patch::LivePatch,
            })
            .collect::<Vec<_>>();
        let write = operation::AppWrite::begin_mode(&app_state.proxy_service, &app).unwrap();
        // The real journal publishes its marker, then the injected commit failure
        // leaves its original pending record; no fake recovery store is created.
        let result = operation::run(
            &write.store,
            &write.vault,
            &write.guard,
            state::op::APPLY,
            &changes,
            target,
            &|_| {
                Err(crate::error::AppError::Message(
                    "synthetic interruption before target commit".into(),
                ))
            },
        );
        assert!(result.is_err());
        drop(write);
        drop(_switch);
        let projection =
            tokio::task::block_in_place(|| owner::read_state(app_state, &app)).unwrap();
        assert!(!projection.can_apply);
        assert_eq!(projection.pending_request, Some(request.clone()));
        assert!(tokio::task::block_in_place(|| owner::prepare_target(
            app_state,
            &app,
            manual.clone()
        ))
        .is_err());
        let mut wrong = request.clone();
        wrong.id = uuid::Uuid::new_v4().to_string();
        assert!(
            tokio::task::block_in_place(|| owner::recover_target(app_state, &app, wrong)).is_err()
        );
        if case == "journal_source_changed" {
            let original = db.get_provider_by_id(&a, app.as_str()).unwrap().unwrap();
            let mut changed = original.clone();
            changed.name = "Changed during pending".into();
            db.save_provider(app.as_str(), &changed).unwrap();
            assert!(tokio::task::block_in_place(|| owner::recover_target(
                app_state,
                &app,
                request.clone()
            ))
            .is_err());
            assert_eq!(
                tokio::task::block_in_place(|| owner::read_state(app_state, &app))
                    .unwrap()
                    .pending_request,
                Some(request.clone())
            );
            db.save_provider(app.as_str(), &original).unwrap();
        }
        assert_eq!(
            tokio::task::block_in_place(|| owner::recover_target(app_state, &app, request.clone()))
                .unwrap()
                .status,
            "completed"
        );
    } else {
        assert_eq!(
            tokio::task::block_in_place(|| owner::apply_target(app_state, &app, prepared.clone()))
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            tokio::task::block_in_place(|| owner::apply_target(app_state, &app, prepared))
                .unwrap()
                .status,
            "completed"
        );
    }
    if matches!(case, "official_bound" | "official_direct") {
        if case == "official_direct" {
            app_state
                .proxy_service
                .set_takeover_for_app(app.as_str(), false)
                .await
                .unwrap();
            db.ensure_official_seed_by_id(crate::database::CODEX_OFFICIAL_PROVIDER_ID, app.clone())
                .unwrap();
        }
        let auth_path = crate::codex_config::get_codex_auth_path();
        let original_auth = std::fs::read(&auth_path).ok();
        let synthetic_auth = br#"{"OPENAI_API_KEY":"synthetic-only-key"}"#;
        std::fs::create_dir_all(auth_path.parent().unwrap()).unwrap();
        std::fs::write(&auth_path, synthetic_auth).unwrap();
        let backups = || {
            let mut paths = std::fs::read_dir(db.secret_session().root().join("backups"))
                .into_iter()
                .flatten()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        };
        let before_backups = backups();
        let quit_calls = std::cell::Cell::new(0);
        let app_lock_held = std::cell::Cell::new(false);
        let relaunch_calls = std::cell::Cell::new(0);
        let result = tokio::task::block_in_place(|| {
            crate::commands::restore_official_login_operation(
                app_state,
                &auth_path,
                || {
                    app_lock_held.set(
                        futures::FutureExt::now_or_never(
                            app_state.proxy_service.lock_switch_for_app(app.as_str()),
                        )
                        .is_none(),
                    );
                    quit_calls.set(quit_calls.get() + 1);
                    crate::relay::chatgpt_app::QuitOutcome::NotRunning
                },
                || {
                    relaunch_calls.set(relaunch_calls.get() + 1);
                    Ok(())
                },
            )
        });
        let after_backups = backups();
        let after_auth = std::fs::read(&auth_path).ok();
        if case == "official_bound" {
            match original_auth {
                Some(bytes) => std::fs::write(&auth_path, bytes).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&auth_path);
                }
            }
            app_state
                .proxy_service
                .set_takeover_for_app(app.as_str(), false)
                .await
                .unwrap();
            assert!(result.is_err());
            assert_eq!(quit_calls.get(), 0, "bound restore must reject before quit");
            assert_eq!(
                after_backups, before_backups,
                "bound restore must reject before backup"
            );
            assert_eq!(after_auth.as_deref(), Some(synthetic_auth.as_slice()));
        } else {
            let ((backup, _), _) = result.unwrap();
            assert!(backup.is_some());
            assert_eq!(quit_calls.get(), 1);
            assert!(
                app_lock_held.get(),
                "original app lock must cover external effects"
            );
            assert!(after_auth.is_none());
            assert_eq!(
                crate::mode::current::direct_provider(db, &app)
                    .unwrap()
                    .unwrap()
                    .id,
                crate::database::CODEX_OFFICIAL_PROVIDER_ID
            );
        }
        assert_eq!(relaunch_calls.get(), 0);
        assert_eq!(
            groups::binding(&db.conn.lock().unwrap(), app.as_str())
                .unwrap()
                .unwrap()
                .target,
            manual
        );
        return;
    }
    if case == "bound_detached_codex" {
        let _switch = app_state
            .proxy_service
            .lock_switch_for_app(app.as_str())
            .await;
        tokio::task::block_in_place(|| {
            crate::mode::controller::exit_locked(&app_state.proxy_service, &app, true)
        })
        .unwrap();
        let detached_bytes = bytes();
        let provider = db.get_provider_by_id(&b, app.as_str()).unwrap().unwrap();
        let preview = tokio::task::block_in_place(|| {
            crate::mode::controller::preview_catalog_selection(
                &app_state.proxy_service,
                &provider,
                None,
            )
        });
        let selection = tokio::task::block_in_place(|| {
            crate::mode::controller::select_locked(
                &app_state.proxy_service,
                &app,
                &provider,
                state::PendingTarget::default(),
            )
        });
        drop(_switch);
        let tray = tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(app_state, app.clone(), &b)
        });
        let hot = app_state
            .proxy_service
            .hot_switch_provider(app.as_str(), &b)
            .await;
        let actual_bytes = bytes();
        let actual_mode = {
            let vault = db.secret_session().read().unwrap();
            crate::mode::current::validate_known_mode(
                &crate::live::engine::DeviceStore::for_device(),
                &vault,
                &app,
            )
            .unwrap()
        };
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        assert!(preview
            .unwrap_err()
            .to_string()
            .contains("routing_groups.explicit_target"));
        assert!(selection.is_err() && tray.is_err() && hot.is_err());
        assert!(actual_mode.is_proxy() && !actual_mode.attached);
        assert_eq!(actual_mode.proxy_route.as_deref(), Some(a.as_str()));
        assert_eq!(actual_bytes, detached_bytes);
        assert_eq!(
            groups::binding(&db.conn.lock().unwrap(), app.as_str())
                .unwrap()
                .unwrap()
                .target,
            manual
        );
        return;
    }
    if case == "bound_keep_for_compat_command" {
        return;
    }
    if case == "bound_exit_direct" {
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(app_state, app.clone(), &b)
        })
        .unwrap();
        let vault = db.secret_session().read().unwrap();
        assert!(!crate::mode::current::validate_known_mode(
            &crate::live::engine::DeviceStore::for_device(),
            &vault,
            &app
        )
        .unwrap()
        .is_proxy());
        assert_eq!(
            groups::binding(&db.conn.lock().unwrap(), app.as_str())
                .unwrap()
                .unwrap()
                .target,
            manual
        );
        return;
    }
    if case.starts_with("bound_legacy_") {
        use crate::services::application_selection as legacy;
        let expected_target = if case == "bound_legacy_order_auto" {
            let target = groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            };
            let prepared = tokio::task::block_in_place(|| {
                owner::prepare_target(app_state, &app, target.clone())
            })
            .unwrap();
            assert_eq!(
                tokio::task::block_in_place(|| owner::apply_target(app_state, &app, prepared))
                    .unwrap()
                    .status,
                "completed"
            );
            target
        } else {
            manual.clone()
        };
        let native_mode = || {
            let vault = db.secret_session().read().unwrap();
            crate::mode::current::validate_known_mode(
                &crate::live::engine::DeviceStore::for_device(),
                &vault,
                &app,
            )
            .unwrap()
        };
        let before_mode = native_mode();
        let mut rejected = Vec::new();
        match case {
            "bound_legacy_selection" => {
                rejected.push((
                    "legacy selection",
                    tokio::task::block_in_place(|| {
                        legacy::select_with_commit(
                            app_state,
                            &app,
                            legacy_change.selection.as_ref().unwrap(),
                            None,
                        )
                    })
                    .is_err(),
                ));
                rejected.push((
                    "tray provider switch",
                    tokio::task::block_in_place(|| {
                        crate::services::ProviderService::switch(app_state, app.clone(), &b)
                    })
                    .is_err(),
                ));
                rejected.push((
                    "hot switch",
                    app_state
                        .proxy_service
                        .hot_switch_provider(app.as_str(), &b)
                        .await
                        .is_err(),
                ));
                rejected.push((
                    "legacy prepare",
                    tokio::task::block_in_place(|| {
                        legacy::prepare_routing_strategy(
                            app_state,
                            &app,
                            &legacy_change,
                            &uuid::Uuid::new_v4().to_string(),
                        )
                    })
                    .is_err(),
                ));
            }
            "bound_legacy_order_auto" => {
                rejected.push((
                    "legacy easy manual alias",
                    legacy::initialize_manual_routing(app_state, &app)
                        .await
                        .is_err(),
                ));
                rejected.push((
                    "legacy current order",
                    tokio::task::block_in_place(|| {
                        legacy::apply_current_order_change(app_state, &app, &[b.clone(), a.clone()])
                    })
                    .is_err(),
                ));
                rejected.push((
                    "legacy auto on",
                    app_state
                        .proxy_service
                        .set_failover_for_app(app.as_str(), true)
                        .await
                        .is_err(),
                ));
                rejected.push((
                    "legacy auto off",
                    app_state
                        .proxy_service
                        .set_failover_for_app(app.as_str(), false)
                        .await
                        .is_err(),
                ));
            }
            "bound_legacy_receipt" => {
                let _switch = app_state
                    .proxy_service
                    .lock_switch_for_app(app.as_str())
                    .await;
                rejected.push((
                    "prepared before binding receipt",
                    tokio::task::block_in_place(|| {
                        legacy::apply_requested_routing_locked(
                            app_state,
                            &app,
                            &legacy_change,
                            None,
                            legacy_request.as_ref().unwrap(),
                            false,
                            |_| panic!("bound legacy receipt must not request external quit"),
                        )
                    })
                    .is_err(),
                ));
            }
            _ => unreachable!(),
        }
        let after_bytes = bytes();
        let after_mode = native_mode();
        let binding = groups::binding(&db.conn.lock().unwrap(), app.as_str())
            .unwrap()
            .unwrap();
        app_state
            .proxy_service
            .set_takeover_for_app(app.as_str(), false)
            .await
            .unwrap();
        for (entry, denied) in rejected {
            assert!(denied, "bound explicit target accepted ignored {entry}");
        }
        assert_eq!(binding.target, expected_target);
        assert_eq!(
            after_mode, before_mode,
            "rejected old entry may not change legacy projection"
        );
        assert_eq!(
            after_bytes, before_bytes,
            "rejected old entry may not rewrite client files"
        );
        return;
    }
    assert_eq!(
        bytes(),
        before_bytes,
        "manual policy must not rewrite client URL/key or bypass gateway"
    );
    let read = tokio::task::block_in_place(|| owner::read_state(app_state, &app)).unwrap();
    assert_eq!(read.binding.as_ref().unwrap().target, manual);
    assert_eq!(read.binding.as_ref().unwrap().revision, 1);
    let ordered = groups::RouteTarget::OrderedGroup {
        group_id: group.id.clone(),
    };
    let prepared =
        tokio::task::block_in_place(|| owner::prepare_target(app_state, &app, ordered.clone()))
            .unwrap();
    assert_eq!(
        tokio::task::block_in_place(|| owner::apply_target(app_state, &app, prepared.clone()))
            .unwrap()
            .status,
        "completed"
    );
    assert_eq!(
        tokio::task::block_in_place(|| owner::query_target(app_state, &app, prepared.request))
            .unwrap()
            .status,
        "completed"
    );
    assert_eq!(
        tokio::task::block_in_place(|| owner::read_state(app_state, &app))
            .unwrap()
            .binding
            .unwrap()
            .target,
        ordered
    );
    assert_eq!(
        bytes(),
        before_bytes,
        "ordered mode shares the same gateway client contract"
    );
    app_state
        .proxy_service
        .set_takeover_for_app(app.as_str(), false)
        .await
        .unwrap();
    // Exiting the last gateway application already stops its owned listener.
}

fn dirty_gateway_file(
    files: &[crate::live::engine::LiveFile],
) -> (std::path::PathBuf, Vec<u8>, Vec<u8>) {
    for file in files {
        if let Ok(bytes) = std::fs::read(&file.path) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                if text.contains("127.0.0.1") {
                    return (
                        file.path.clone(),
                        bytes.clone(),
                        text.replace("127.0.0.1", "203.0.113.8").into_bytes(),
                    );
                }
            }
        }
    }
    panic!("fixture requires an actual original gateway projection");
}

#[cfg(feature = "test-hooks")]
fn source_writer_hook(
    state: &AppState,
    id: &str,
    boundary: &'static str,
) -> (
    Box<dyn FnMut(&str)>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let connection = rusqlite::Connection::open(
        state
            .db
            .secret_session()
            .root()
            .join(crate::config::DB_FILE_NAME),
    )
    .unwrap();
    crate::database::storage::check_identity(
        &connection,
        &state.db.secret_session().read().unwrap(),
    )
    .unwrap();
    connection.busy_timeout(std::time::Duration::ZERO).unwrap();
    let blocked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let captured = blocked.clone();
    let id = id.to_owned();
    let hook = Box::new(move |point: &str| {
        if point == boundary {
            match connection.execute(
                "UPDATE providers SET name='Concurrent writer' WHERE id=?1",
                [&id],
            ) {
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    captured.store(true, std::sync::atomic::Ordering::SeqCst)
                }
                Ok(_) => {}
                Err(error) => panic!("unexpected second-connection error: {error}"),
            }
        }
    });
    (hook, blocked)
}
