//! In-memory ownership checkpoint. No GUI, provisioning or real configuration writes.
#[path = "../src/services/routing_groups.rs"]
mod routing_groups;
#[allow(dead_code)]
#[path = "../src/services/service_sources.rs"]
mod service_sources;
use routing_groups::*;
use rusqlite::Connection;
use service_sources::*;

// The original fixture-only projection adapters belong to this sole consumer.
// Keep the original assertions against the real read/resolve_references owner.
/// Resolve existing native source projections, never discover/provision them.
/// This checkpoint accepts only the model explicitly evidenced by a binding.
/// Broader catalogs must be supplied by their native capability owner at integration.
/// This is ordered reference resolution, NOT another forwarding/failover engine.
#[cfg(test)]
pub fn resolve(
    db: &Connection,
    id: &str,
    app: &str,
    sources: &[ServiceSource],
) -> Result<Resolved, Error> {
    let group = read(db, id)?.ok_or(Error::Unresolved)?;
    resolve_members(group, |member| {
        let mut matches = sources
            .iter()
            .filter(|source| source.source_id == member.source_id);
        let source = matches.next().ok_or(Error::Unresolved)?;
        if matches.next().is_some() {
            return Err(Error::Unresolved);
        }
        let mut bindings = source
            .bindings
            .iter()
            .filter(|binding| binding.app_id == app);
        let binding = bindings.next().ok_or(Error::Unresolved)?;
        if bindings.next().is_some()
            || binding.provider_id != source.source_id
            || !binding.can_select
            || binding.model.as_deref() != Some(member.model.as_str())
        {
            return Err(Error::Unresolved);
        }
        Ok(binding.provider_id.clone())
    })
}

/// The native reference resolver supplies eligibility; this owner alone defines order/model intent.
#[cfg(test)]
pub fn resolve_members(
    group: Group,
    mut reference: impl FnMut(&Member) -> Result<String, Error>,
) -> Result<Resolved, Error> {
    resolve_references(group, |member| {
        let provider_id = reference(member)?;
        Ok(ResolvedReference {
            source_id: provider_id.clone(),
            provider_id,
        })
    })
}

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db
}
fn members() -> Vec<Member> {
    vec![
        Member {
            source_id: "source-a".into(),
            model: "model-a".into(),
        },
        Member {
            source_id: "source-b".into(),
            model: "model-b".into(),
        },
    ]
}
fn source(id: &str, model: &str) -> ServiceSource {
    ServiceSource {
        source_id: id.into(),
        account: SourceAccountReference::Relay { id: 7 },
        name: "Identical display name".into(),
        service_name: None,
        account_label: None,
        configuration_name: None,
        supported_targets: Vec::new(),
        bindings: ["codex", "claude"]
            .into_iter()
            .map(|app| ServiceSourceBinding {
                app_id: app.into(),
                provider_id: id.into(),
                model: Some(model.into()),
                usage_state: if id == "source-b" {
                    SourceUsageState::Current
                } else {
                    SourceUsageState::Inactive
                },
                can_select: true,
            })
            .collect(),
    }
}
#[test]
fn saved_order_roundtrips_and_stale_revision_cannot_overwrite() {
    let mut db = database();
    let first = save(&mut db, "daily", None, &members()).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(read(&db, "daily").unwrap().unwrap().members, members());
    let mut reversed = members();
    reversed.reverse();
    assert_eq!(
        save(&mut db, "daily", Some(1), &reversed).unwrap().revision,
        2
    );
    assert!(matches!(
        save(&mut db, "daily", Some(1), &members()),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        save(&mut db, "daily", None, &members()),
        Err(Error::Conflict)
    ));
    assert_eq!(read(&db, "daily").unwrap().unwrap().members, reversed);
    assert_eq!(
        db.query_row("SELECT count(*) FROM loongport_routing_groups", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
}
#[test]
fn two_apps_resolve_one_order_without_pinning_current_source_or_writes() {
    let mut db = database();
    save(&mut db, "daily", None, &members()).unwrap();
    let sources = vec![source("source-b", "model-b"), source("source-a", "model-a")];
    let before = db.total_changes();
    for app in ["codex", "claude"] {
        let result = resolve(&db, "daily", app, &sources).unwrap();
        assert_eq!(result.revision, 1);
        assert_eq!(
            result
                .candidates
                .iter()
                .map(|c| c.provider_id.as_str())
                .collect::<Vec<_>>(),
            ["source-a", "source-b"]
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .map(|c| c.model.as_str())
                .collect::<Vec<_>>(),
            ["model-a", "model-b"]
        );
    }
    assert_eq!(db.total_changes(), before);
    assert!(read(&db, "absent").unwrap().is_none());
    assert_eq!(db.total_changes(), before);
}
#[test]
fn invalid_edits_preserve_the_existing_group() {
    let mut db = database();
    save(&mut db, "daily", None, &members()).unwrap();
    for invalid in [
        vec![],
        vec![members()[0].clone(), members()[0].clone()],
        vec![Member {
            source_id: " ".into(),
            model: "x".into(),
        }],
        vec![Member {
            source_id: "a".into(),
            model: "".into(),
        }],
    ] {
        assert!(matches!(
            save(&mut db, "daily", Some(1), &invalid),
            Err(Error::Invalid)
        ));
    }
    assert!(matches!(
        save(&mut db, "", None, &members()),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        save(&mut db, "missing", Some(1), &members()),
        Err(Error::Conflict)
    ));
    assert_eq!(read(&db, "daily").unwrap().unwrap().revision, 1);
}
#[test]
fn missing_ambiguous_or_incompatible_identity_fails_closed_without_provisioning() {
    let mut db = database();
    save(&mut db, "daily", None, &members()).unwrap();
    let before = db.total_changes();
    let mut sources = vec![source("source-a", "model-a"), source("source-b", "model-b")];
    assert!(matches!(
        resolve(&db, "daily", "unknown-app", &sources),
        Err(Error::Unresolved)
    ));
    sources[0].bindings[0].can_select = false;
    assert!(matches!(
        resolve(&db, "daily", "codex", &sources),
        Err(Error::Unresolved)
    ));
    sources[0].bindings[0].can_select = true;
    sources[0].bindings[0].model = None;
    assert!(matches!(
        resolve(&db, "daily", "codex", &sources),
        Err(Error::Unresolved)
    ));
    sources[0].bindings[0].model = Some("model-a".into());
    sources.push(source("source-a", "model-a"));
    assert!(matches!(
        resolve(&db, "daily", "codex", &sources),
        Err(Error::Unresolved)
    ));
    sources.pop();
    sources.remove(0);
    assert!(matches!(
        resolve(&db, "daily", "codex", &sources),
        Err(Error::Unresolved)
    ));
    assert_eq!(db.total_changes(), before);
}
#[test]
fn corrupt_storage_is_an_error_not_an_empty_success() {
    let db = database();
    db.execute(
        "INSERT INTO loongport_routing_groups(id,name,revision,members) VALUES('daily','Daily',1,'garbage')",
        [],
    )
    .unwrap();
    assert!(read(&db, "daily").is_err());
}

#[test]
fn separate_connections_cannot_overwrite_a_reviewed_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.sqlite");
    let mut first = Connection::open(&path).unwrap();
    first.execute_batch(SCHEMA).unwrap();
    let mut second = Connection::open(&path).unwrap();
    save(&mut first, "daily", None, &members()).unwrap();
    let reviewed = read(&second, "daily").unwrap().unwrap().revision;
    let mut reversed = members();
    reversed.reverse();
    save(&mut first, "daily", Some(1), &reversed).unwrap();
    assert_eq!(
        save(&mut second, "daily", Some(reviewed), &members()),
        Err(Error::Conflict)
    );
    assert_eq!(read(&second, "daily").unwrap().unwrap().members, reversed);
    let sources = vec![source("source-a", "model-a"), source("source-b", "model-b")];
    for app in ["claude", "codex"] {
        let result = resolve(&second, "daily", app, &sources).unwrap();
        assert_eq!(result.revision, 2);
        assert_eq!(result.candidates[0].provider_id, "source-b");
        assert_eq!(result.candidates[1].provider_id, "source-a");
    }
}

#[test]
fn duplicate_binding_wrong_provider_and_wrong_model_are_not_candidates() {
    let mut db = database();
    save(&mut db, "daily", None, &members()).unwrap();
    let before = db.total_changes();
    for case in ["duplicate", "provider", "model"] {
        let mut sources = vec![source("source-a", "model-a"), source("source-b", "model-b")];
        match case {
            "duplicate" => sources[0].bindings.push(ServiceSourceBinding {
                app_id: "codex".into(),
                provider_id: "source-a".into(),
                model: Some("model-a".into()),
                usage_state: SourceUsageState::Inactive,
                can_select: true,
            }),
            "provider" => sources[0].bindings[0].provider_id = "source-b".into(),
            _ => sources[0].bindings[0].model = Some("other-model".into()),
        }
        assert_eq!(
            resolve(&db, "daily", "codex", &sources),
            Err(Error::Unresolved),
            "{case}"
        );
    }
    assert_eq!(db.total_changes(), before);
}

#[test]
fn missing_schema_does_not_get_created_by_read_or_write() {
    let mut db = Connection::open_in_memory().unwrap();
    assert_eq!(read(&db, "daily"), Err(Error::Storage));
    assert_eq!(
        save(&mut db, "daily", None, &members()),
        Err(Error::Storage)
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn empty_installed_tables_missing_required_columns_fail_validation() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE loongport_routing_groups(id TEXT); CREATE TABLE loongport_application_route_bindings(app_type TEXT);").unwrap();
    assert!(
        validate_schema(&db).is_err(),
        "empty broken tables must not pass the version30 schema gate"
    );
}

#[test]
fn application_binding_cas_checks_group_revision_and_reads_one_owner() {
    let mut db = Connection::open_in_memory().unwrap();
    create_schema(&db).unwrap();
    let first = save(&mut db, "daily", None, &members()).unwrap();
    assert_eq!(binding(&db, "claude").unwrap(), None);
    assert_eq!(
        bind(&mut db, "claude", "daily", None, 2),
        Err(Error::Conflict)
    );
    assert_eq!(
        bind(&mut db, "claude", "daily", None, first.revision)
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        bind(&mut db, "codex", "daily", None, first.revision)
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        bind(&mut db, "claude", "daily", None, first.revision),
        Err(Error::Conflict)
    );
    let mut next = members();
    next.reverse();
    let second = save(&mut db, "daily", Some(first.revision), &next).unwrap();
    assert_eq!(
        bind(&mut db, "claude", "daily", Some(1), first.revision),
        Err(Error::Conflict)
    );
    assert_eq!(
        bind(&mut db, "claude", "daily", Some(1), second.revision)
            .unwrap()
            .revision,
        2
    );
    for app in ["claude", "codex"] {
        let bound = binding(&db, app).unwrap().unwrap();
        assert_eq!(
            read(
                &db,
                match &bound.target {
                    RouteTarget::OrderedGroup { group_id } => group_id,
                    _ => panic!("expected group"),
                }
            )
            .unwrap()
            .unwrap(),
            second
        );
    }
    validate_schema(&db).unwrap();
}

#[test]
fn sole_binding_schema_can_record_explicit_manual_target() {
    let db = database();
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(loongport_application_route_bindings)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        columns.contains(&"target".into()),
        "sole binding owner cannot persist explicit manual versus ordered target"
    );
}

#[test]
fn explicit_manual_and_ordered_targets_replace_one_binding_with_cas() {
    let mut db = database();
    let group = save(&mut db, "daily", None, &members()).unwrap();
    let ordered = RouteTarget::OrderedGroup {
        group_id: group.id.clone(),
    };
    let first = set_target(
        &mut db,
        "claude",
        ordered.clone(),
        None,
        Some(group.revision),
    )
    .unwrap();
    let manual = RouteTarget::Manual {
        source_id: "source-a".into(),
        model: "model-a".into(),
    };
    assert_eq!(
        set_target(&mut db, "claude", manual.clone(), None, None),
        Err(Error::Conflict)
    );
    let second = set_target(
        &mut db,
        "claude",
        manual.clone(),
        Some(first.revision),
        None,
    )
    .unwrap();
    assert_eq!(binding(&db, "claude").unwrap(), Some(second.clone()));
    assert_eq!(second.target, manual);
    assert_eq!(
        set_target(
            &mut db,
            "claude",
            ordered,
            Some(first.revision),
            Some(group.revision)
        ),
        Err(Error::Conflict)
    );
    assert_eq!(read(&db, "daily").unwrap(), Some(group));
}
