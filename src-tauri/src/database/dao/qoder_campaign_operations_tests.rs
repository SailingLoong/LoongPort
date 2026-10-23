use super::*;

#[test]
fn cn_schema_keeps_real_optional_target_fields() {
    let conn = Connection::open_in_memory().unwrap();
    create_schema(&conn).unwrap();
    let nullable: i64 = conn.query_row("SELECT [notnull] FROM pragma_table_info('qoder_campaign_operations') WHERE name='campaign_key'", [], |r|r.get(0)).unwrap();
    assert_eq!(
        nullable, 0,
        "CN campaign keys must be SQL NULL when the server omits them"
    );
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result,provider_id) VALUES('attempt','subject','china','campaign',NULL,NULL,NULL,'generation','unknown','provider')", []).unwrap();
    assert_eq!(read_rows(&conn).unwrap().len(), 1);
}

fn identity(region: Region, provider: &str) -> Identity {
    Identity {
        provider_id: provider.into(),
        uid: "synthetic-uid".into(),
        region,
    }
}
fn cn(id: &str, campaign: &str) -> CnOperation {
    CnOperation {
        id: id.into(),
        generation: "generation-1".into(),
        identity: identity(Region::China, "provider-1"),
        target: CnTarget {
            campaign_id: campaign.into(),
            campaign_key: None,
            start_at: None,
            end_at: None,
        },
        outcome: Outcome::Unknown,
        receipt_confirmed: false,
        listing_claimed: false,
    }
}
fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    create_schema(&c).unwrap();
    c
}
fn insert(c: &Connection, operation: &CnOperation) -> Result<Option<CnOperation>, AppError> {
    let tx = c.unchecked_transaction()?;
    let result = insert_cn_unknown_on(&tx, operation)?;
    tx.commit()?;
    Ok(result)
}
fn terminal(c: &Connection, operation: &CnOperation) {
    let tx = c.unchecked_transaction().unwrap();
    update_cn_result_on(&tx, operation, Outcome::ClaimedDeltaUnknown).unwrap();
    tx.commit().unwrap();
}
fn version(c: &Connection, value: i32) {
    c.execute_batch(&format!("CREATE TABLE IF NOT EXISTS loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER);INSERT OR REPLACE INTO loongport_schema_version VALUES(1,{value})")).unwrap();
}
fn old() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(LEGACY_CREATE).unwrap();
    version(&c, 27);
    c.execute("INSERT INTO qoder_campaign_operations VALUES('intl-attempt','synthetic-uid','international','intl-campaign','intl-key',100,200,'intl-generation','unknown')",[]).unwrap();
    c
}
#[test]
fn cn_many_campaigns_share_uid_without_crossing_region_or_provider_binding() {
    let c = db();
    let a = cn("a", "welcome");
    insert(&c, &a).unwrap();
    let mut b = cn("b", "daily");
    b.identity.provider_id = "provider-2".into();
    b.generation = "generation-2".into();
    insert(&c, &b).unwrap();
    let read = read_cn_on(&c, &identity(Region::China, "provider-duplicate")).unwrap();
    assert_eq!(read, vec![a.clone(), b.clone()]);
    assert!(read_cn_on(&c, &identity(Region::International, "provider-1")).is_err());
    assert!(read_on(&c, &identity(Region::International, "provider-1"))
        .unwrap()
        .is_empty());
    let mut duplicate = a.clone();
    duplicate.id = "other-attempt".into();
    duplicate.generation = "rotated".into();
    duplicate.identity.provider_id = "other-provider".into();
    assert!(insert(&c, &duplicate).is_err());
    assert_eq!(read_rows(&c).unwrap().len(), 2);
}
#[test]
fn cn_unknown_survives_arbitrarily_later_server_window_and_rotation() {
    let c = db();
    let mut a = cn("a", "daily");
    a.target.start_at = Some(1);
    a.target.end_at = Some(2);
    insert(&c, &a).unwrap();
    let mut later = a.clone();
    later.id = "later".into();
    later.generation = "rotated".into();
    later.target.start_at = Some(900_000);
    later.target.end_at = Some(900_100);
    assert!(insert(&c, &later).is_err());
    assert_eq!(read_cn_on(&c, &a.identity).unwrap(), vec![a]);
}
#[test]
fn cn_terminal_needs_both_complete_proven_nonoverlap_intervals() {
    for (start, end) in [
        (None, None),
        (Some(100), None),
        (None, Some(200)),
        (Some(100), Some(200)),
    ] {
        let c = db();
        let mut a = cn("a", "daily");
        a.target.start_at = start;
        a.target.end_at = end;
        insert(&c, &a).unwrap();
        terminal(&c, &a);
        let mut later = a.clone();
        later.id = "later".into();
        later.target.start_at = Some(200);
        later.target.end_at = Some(300);
        if start.is_some() && end.is_some() {
            let old = insert(&c, &later).unwrap().unwrap();
            assert_eq!(old.outcome, Outcome::ClaimedDeltaUnknown);
            assert_eq!(read_cn_on(&c, &a.identity).unwrap(), vec![later]);
        } else {
            assert!(insert(&c, &later).is_err());
            assert_eq!(read_cn_on(&c, &a.identity).unwrap()[0].id, "a");
        }
    }
    let c = db();
    let mut a = cn("a", "daily");
    a.target.start_at = Some(100);
    a.target.end_at = Some(200);
    insert(&c, &a).unwrap();
    terminal(&c, &a);
    for target in [
        CnTarget {
            start_at: Some(199),
            end_at: Some(300),
            ..a.target.clone()
        },
        CnTarget {
            start_at: Some(200),
            end_at: None,
            ..a.target.clone()
        },
        CnTarget {
            start_at: None,
            end_at: Some(300),
            ..a.target.clone()
        },
    ] {
        let later = CnOperation {
            id: "later".into(),
            target,
            ..a.clone()
        };
        assert!(insert(&c, &later).is_err());
    }
}
#[test]
fn cn_optional_keys_and_windows_roundtrip_exactly() {
    let c = db();
    for (i, (key, start, end)) in [
        (None, None, None),
        (Some("opaque key"), Some(100), None),
        (None, None, Some(200)),
        (Some("server-key"), Some(100), Some(200)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut a = cn(&format!("a{i}"), &format!("c{i}"));
        a.target.campaign_key = key.map(str::to_owned);
        a.target.start_at = start;
        a.target.end_at = end;
        insert(&c, &a).unwrap();
        assert!(read_cn_on(&c, &a.identity).unwrap().contains(&a));
    }
}
#[test]
fn cn_result_compare_and_swap_rejects_stale_or_forged_bindings() {
    let c = db();
    let a = cn("a", "welcome");
    insert(&c, &a).unwrap();
    let mut altered = vec![];
    let mut v = a.clone();
    v.identity.provider_id = "other".into();
    altered.push(v);
    let mut v = a.clone();
    v.generation = "other".into();
    altered.push(v);
    let mut v = a.clone();
    v.identity.uid = "other".into();
    altered.push(v);
    let mut v = a.clone();
    v.identity.region = Region::International;
    altered.push(v);
    let mut v = a.clone();
    v.target.campaign_key = Some("forged".into());
    altered.push(v);
    let mut v = a.clone();
    v.target.start_at = Some(100);
    altered.push(v);
    for bad in altered {
        let tx = c.unchecked_transaction().unwrap();
        assert!(update_cn_result_on(&tx, &bad, Outcome::ClaimedDeltaUnknown).is_err());
    }
    for bad in [Outcome::NewlyClaimed, Outcome::AlreadyClaimed] {
        let tx = c.unchecked_transaction().unwrap();
        assert!(update_cn_result_on(&tx, &a, bad).is_err());
    }
    terminal(&c, &a);
    terminal(&c, &a);
    assert_eq!(
        read_cn_on(&c, &a.identity).unwrap()[0].outcome,
        Outcome::ClaimedDeltaUnknown
    );
}
#[test]
fn cn_invalid_values_and_attempt_collision_never_erase_receipts() {
    let c = db();
    let a = cn("collision", "one");
    insert(&c, &a).unwrap();
    let b = cn("collision", "two");
    assert!(insert(&c, &b).is_err());
    let mut bad = cn("bad", "..");
    assert!(insert(&c, &bad).is_err());
    bad.target.campaign_id = "valid".into();
    bad.target.campaign_key = Some(" ".into());
    assert!(insert(&c, &bad).is_err());
    bad.target.campaign_key = None;
    bad.target.start_at = Some(-1);
    assert!(insert(&c, &bad).is_err());
    bad.target.start_at = Some(2);
    bad.target.end_at = Some(1);
    assert!(insert(&c, &bad).is_err());
    bad.target.start_at = None;
    bad.target.end_at = None;
    bad.identity.region = Region::International;
    assert!(insert(&c, &bad).is_err());
    assert_eq!(read_cn_on(&c, &a.identity).unwrap(), vec![a]);
}
#[test]
fn migration_preserves_all_legacy_receipts_and_is_idempotent() {
    let c = old();
    let before = read_rows(&c).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    migrate_cn_schema(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(read_rows(&c).unwrap(), before);
    version(&c, 29);
    migrate_cn_schema(&c).unwrap();
    let intl = read_on(&c, &identity(Region::International, "new-provider")).unwrap();
    assert_eq!(intl[0].outcome, Outcome::Unknown);
    assert_eq!(intl[0].id, "intl-attempt");
    insert(&c, &cn("cn-attempt", "intl-campaign")).unwrap();
    assert_eq!(read_rows(&c).unwrap().len(), 2);
    assert!(c.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result) VALUES('second-intl','synthetic-uid','international','other','key',300,400,'generation','unknown')",[]).is_err());
}
#[test]
fn installed_current_cannot_downgrade_or_repair_missing_schema() {
    let c = old();
    version(&c, 29);
    assert!(read_rows(&c).is_err());
    assert!(migrate_cn_schema(&c).is_err());
    let c = db();
    c.execute_batch("DROP INDEX qoder_operations_cn_campaign")
        .unwrap();
    assert!(create_schema(&c).is_err());
    assert!(read_rows(&c).is_err());
}
#[test]
fn rejects_schema_forgery_and_untrusted_rows() {
    for sql in ["DROP INDEX qoder_operations_cn_campaign; CREATE INDEX qoder_operations_cn_campaign ON qoder_campaign_operations(subject,region,campaign_id)","CREATE TRIGGER rogue AFTER INSERT ON qoder_campaign_operations BEGIN DELETE FROM qoder_campaign_operations; END", "CREATE INDEX extra ON qoder_campaign_operations(subject)"] {
        let c=db();c.execute_batch(sql).unwrap();assert!(read_rows(&c).is_err());
    }
    for sql in [
        "UPDATE qoder_campaign_operations SET result='claimed'",
        "UPDATE qoder_campaign_operations SET provider_id=NULL",
        "UPDATE qoder_campaign_operations SET campaign_key=' '",
        "UPDATE qoder_campaign_operations SET campaign_id='..'",
        "UPDATE qoder_campaign_operations SET window_start=50,window_end=10",
        "UPDATE qoder_campaign_operations SET region='other'",
    ] {
        let c = db();
        insert(&c, &cn("a", "welcome")).unwrap();
        c.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        c.execute_batch(sql).unwrap();
        assert!(read_rows(&c).is_err(), "{sql}");
    }
}
#[test]
fn restore_preserves_local_cn_and_intl_and_discards_foreign_claims() {
    let live = old();
    {
        let tx = live.unchecked_transaction().unwrap();
        migrate_cn_schema(&tx).unwrap();
        tx.commit().unwrap();
    }
    version(&live, 29);
    let a = cn("local-cn", "welcome");
    insert(&live, &a).unwrap();
    let target = old();
    let before = read_rows(&live).unwrap();
    preserve_local(&live, &target).unwrap();
    assert_eq!(read_rows(&target).unwrap(), before);
    let target = db();
    insert(&target, &cn("foreign", "different")).unwrap();
    preserve_local(&live, &target).unwrap();
    assert_eq!(read_rows(&target).unwrap(), before);
    let empty = db();
    preserve_local(&empty, &target).unwrap();
    assert!(read_rows(&target).unwrap().is_empty());
}
#[test]
fn restore_rejects_malformed_current_target_without_touching_local_or_target() {
    let live = db();
    insert(&live, &cn("local", "campaign")).unwrap();
    let target = db();
    insert(&target, &cn("foreign", "campaign")).unwrap();
    target
        .execute_batch("CREATE INDEX extra ON qoder_campaign_operations(subject)")
        .unwrap();
    let before = read_rows(&live).unwrap();
    assert!(preserve_local(&live, &target).is_err());
    assert_eq!(read_rows(&live).unwrap(), before);
    let id: String = target
        .query_row(
            "SELECT attempt_id FROM qoder_campaign_operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(id, "foreign");
}

#[test]
fn cn_opaque_campaign_ids_preserve_punctuation_without_guessing_format() {
    let c = db();
    for (i, id) in ["benefit:2026", "a.b", "campaign/one", "活动"]
        .into_iter()
        .enumerate()
    {
        let a = cn(&format!("opaque-{i}"), id);
        insert(&c, &a).unwrap();
        assert!(read_cn_on(&c, &a.identity).unwrap().contains(&a));
    }
    for id in [".", "..", "\u{202e}campaign", "a\n"] {
        assert!(insert(&c, &cn("bad", id)).is_err());
    }
}

#[test]
fn intl_original_receipt_lifecycle_is_unchanged_with_cn_rows_present() {
    let c = db();
    let identity = identity(Region::International, "intl-provider");
    let make = |id: &str, start, end| Operation {
        id: id.into(),
        generation: "intl-generation".into(),
        binding: BoundCampaign::from_operation(
            identity.clone(),
            "intl-campaign".into(),
            "intl-key".into(),
            start,
            end,
        )
        .unwrap(),
        outcome: Outcome::Unknown,
    };
    let first = make("intl-first", 100, 200);
    let next = make("intl-next", 200, 300);
    {
        let tx = c.unchecked_transaction().unwrap();
        assert!(insert_unknown_on(&tx, &first).unwrap().is_none());
        tx.commit().unwrap();
    }
    insert(&c, &cn("cn-attempt", "intl-campaign")).unwrap();
    {
        let tx = c.unchecked_transaction().unwrap();
        assert!(insert_unknown_on(&tx, &next).is_err());
    }
    {
        let tx = c.unchecked_transaction().unwrap();
        update_result_on(&tx, &first, Outcome::NewlyClaimed).unwrap();
        tx.commit().unwrap();
    }
    {
        let tx = c.unchecked_transaction().unwrap();
        let prior = insert_unknown_on(&tx, &next).unwrap().unwrap();
        assert_eq!(prior.outcome, Outcome::NewlyClaimed);
        tx.commit().unwrap();
    }
    assert_eq!(read_on(&c, &identity).unwrap(), vec![next]);
    assert_eq!(
        read_cn_on(&c, &cn("unused", "unused").identity)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn cn_capacity_limit_never_commits_oversized_ledger() {
    let c = db();
    {
        let tx = c.unchecked_transaction().unwrap();
        for n in 0..4096 {
            tx.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,credential_generation,result,provider_id) VALUES(?1,'synthetic-uid','china',?1,'generation','unknown','provider')",[format!("campaign-{n}")]).unwrap();
        }
        tx.commit().unwrap();
    }
    assert!(insert(&c, &cn("overflow", "overflow")).is_err());
    assert_eq!(read_rows(&c).unwrap().len(), 4096);
}
#[test]
fn cn_mutation_requires_existing_provider_transaction() {
    let c = db();
    let a = cn("a", "daily");
    assert!(insert_cn_unknown_on(&c, &a).is_err());
    insert(&c, &a).unwrap();
    assert!(update_cn_result_on(&c, &a, Outcome::ClaimedDeltaUnknown).is_err());
    assert_eq!(
        read_cn_on(&c, &a.identity).unwrap()[0].outcome,
        Outcome::Unknown
    );
}
#[test]
fn migration_rejects_forged_legacy_rows_and_rolls_back_colliding_objects() {
    let c = old();
    c.execute_batch(
        "PRAGMA ignore_check_constraints=ON;UPDATE qoder_campaign_operations SET result='forged'",
    )
    .unwrap();
    let tx = c.unchecked_transaction().unwrap();
    assert!(migrate_cn_schema(&tx).is_err());
    drop(tx);
    assert!(legacy_schema(&c).unwrap());
    let c = old();
    c.execute_batch("CREATE TABLE qoder_campaign_operations_v27(value TEXT)")
        .unwrap();
    let before = read_rows(&c).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    assert!(migrate_cn_schema(&tx).is_err());
    drop(tx);
    assert_eq!(read_rows(&c).unwrap(), before);
    assert!(legacy_schema(&c).unwrap());
}

fn observe(c: &Connection, operation: &CnOperation) -> Result<(), AppError> {
    let tx = c.unchecked_transaction()?;
    observe_cn_claimed_on(&tx, operation)?;
    tx.commit()?;
    Ok(())
}
fn reconcile(c: &Connection, operation: &CnOperation) -> Result<(), AppError> {
    let tx = c.unchecked_transaction()?;
    reconcile_cn_result_on(&tx, operation)?;
    tx.commit()?;
    Ok(())
}
#[test]
fn cn_receipt_then_independent_claimed_observation_permits_one_same_target_intent() {
    let c = db();
    let a = cn("first", "welcome");
    insert(&c, &a).unwrap();
    terminal(&c, &a);
    let confirmed = read_cn_on(&c, &a.identity).unwrap().remove(0);
    assert!(confirmed.receipt_confirmed);
    assert!(!confirmed.listing_claimed);
    let next = cn("next", "welcome");
    assert!(insert(&c, &next).is_err());
    assert!(insert(&c, &next).is_err());
    assert!(
        observe(&c, &a).is_err(),
        "the original Unknown cannot double as later observation"
    );
    observe(&c, &confirmed).unwrap();
    let observed = read_cn_on(&c, &a.identity).unwrap().remove(0);
    assert!(observed.receipt_confirmed && observed.listing_claimed);
    let prior = insert(&c, &next).unwrap().unwrap();
    assert_eq!(prior, observed);
    assert_eq!(read_cn_on(&c, &a.identity).unwrap(), vec![next.clone()]);
    let third = cn("third", "welcome");
    assert!(insert(&c, &third).is_err());
    assert!(
        observe(&c, &confirmed).is_err(),
        "old observation cannot mark the new attempt"
    );
}
#[test]
fn cn_get_reconciled_unknown_never_gains_same_target_replay_marker() {
    let c = db();
    let a = cn("unknown", "welcome");
    insert(&c, &a).unwrap();
    assert!(observe(&c, &a).is_err());
    let mut forged = a.clone();
    forged.outcome = Outcome::ClaimedDeltaUnknown;
    forged.receipt_confirmed = true;
    assert!(
        observe(&c, &forged).is_err(),
        "flags cannot promote a persisted Unknown"
    );
    reconcile(&c, &a).unwrap();
    let reconciled = read_cn_on(&c, &a.identity).unwrap().remove(0);
    assert_eq!(reconciled.outcome, Outcome::ClaimedDeltaUnknown);
    assert!(!reconciled.receipt_confirmed && !reconciled.listing_claimed);
    assert!(observe(&c, &reconciled).is_err());
    assert!(observe(&c, &forged).is_err());
    assert!(insert(&c, &cn("later", "welcome")).is_err());
    reconcile(&c, &a).unwrap();
    assert_eq!(read_cn_on(&c, &a.identity).unwrap(), vec![reconciled]);
}
#[test]
fn cn_observation_and_same_target_advance_require_exact_original_binding() {
    let c = db();
    let a = cn("first", "welcome");
    insert(&c, &a).unwrap();
    terminal(&c, &a);
    let confirmed = read_cn_on(&c, &a.identity).unwrap().remove(0);
    let mut stale = vec![];
    let mut v = confirmed.clone();
    v.generation = "rotated".into();
    stale.push(v);
    let mut v = confirmed.clone();
    v.identity.provider_id = "duplicate-provider".into();
    stale.push(v);
    let mut v = confirmed.clone();
    v.target.campaign_key = Some("new-key".into());
    stale.push(v);
    let mut v = confirmed.clone();
    v.target.start_at = Some(100);
    stale.push(v);
    let mut v = confirmed.clone();
    v.identity.region = Region::International;
    stale.push(v);
    for bad in stale {
        assert!(observe(&c, &bad).is_err());
    }
    observe(&c, &confirmed).unwrap();
    for field in 0..5 {
        let mut next = cn("next", "welcome");
        match field {
            0 => next.generation = "rotated".into(),
            1 => next.identity.provider_id = "duplicate-provider".into(),
            2 => next.target.campaign_key = Some("new-key".into()),
            3 => next.target.start_at = Some(100),
            _ => next.target.end_at = Some(200),
        };
        assert!(insert(&c, &next).is_err());
    }
    assert_eq!(read_cn_on(&c, &a.identity).unwrap()[0].id, a.id);
}
#[test]
fn cn_receipt_marker_and_reconciliation_survive_restore_without_foreign_adoption() {
    let live = db();
    let a = cn("receipt", "receipt-campaign");
    insert(&live, &a).unwrap();
    terminal(&live, &a);
    let confirmed = read_cn_on(&live, &a.identity).unwrap().remove(0);
    observe(&live, &confirmed).unwrap();
    let b = cn("reconciled", "other-campaign");
    insert(&live, &b).unwrap();
    reconcile(&live, &b).unwrap();
    let target = db();
    insert(&target, &cn("foreign", "foreign-campaign")).unwrap();
    preserve_local(&live, &target).unwrap();
    assert_eq!(read_rows(&live).unwrap(), read_rows(&target).unwrap());
    let rows = read_cn_on(&target, &a.identity).unwrap();
    assert!(rows
        .iter()
        .any(|r| r.id == "receipt" && r.receipt_confirmed && r.listing_claimed));
    assert!(rows
        .iter()
        .any(|r| r.id == "reconciled" && !r.receipt_confirmed && !r.listing_claimed));
}
#[test]
fn cn_marker_survives_reopening_and_is_consumed_by_replacement() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "loongport-synthetic-cn-marker-{}-{stamp}.sqlite",
        std::process::id()
    ));
    let a = cn("first", "welcome");
    {
        let c = Connection::open(&path).unwrap();
        create_schema(&c).unwrap();
        insert(&c, &a).unwrap();
        terminal(&c, &a);
        let confirmed = read_cn_on(&c, &a.identity).unwrap().remove(0);
        observe(&c, &confirmed).unwrap();
    }
    {
        let c = Connection::open(&path).unwrap();
        let rows = read_cn_on(&c, &a.identity).unwrap();
        assert!(rows[0].listing_claimed && rows[0].receipt_confirmed);
        insert(&c, &cn("next", "welcome")).unwrap();
    }
    {
        let c = Connection::open(&path).unwrap();
        assert!(!read_cn_on(&c, &a.identity).unwrap()[0].listing_claimed);
        assert!(insert(&c, &cn("third", "welcome")).is_err());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cn_marker_cannot_be_forged_on_unknown_intent_or_downgraded_by_late_recovery() {
    let c = db();
    let mut bad = cn("forged", "welcome");
    bad.receipt_confirmed = true;
    assert!(insert(&c, &bad).is_err());
    bad.receipt_confirmed = false;
    bad.listing_claimed = true;
    assert!(insert(&c, &bad).is_err());
    let a = cn("a", "welcome");
    insert(&c, &a).unwrap();
    terminal(&c, &a);
    let confirmed = read_cn_on(&c, &a.identity).unwrap().remove(0);
    observe(&c, &confirmed).unwrap();
    reconcile(&c, &a).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    update_cn_result_on(&tx, &a, Outcome::Unknown).unwrap();
    tx.commit().unwrap();
    let still = read_cn_on(&c, &a.identity).unwrap().remove(0);
    assert!(still.receipt_confirmed && still.listing_claimed);
    observe(&c, &confirmed).unwrap();
}
#[test]
fn intl_schema_rejects_cn_only_confirmation_variants() {
    let c = old();
    {
        let tx = c.unchecked_transaction().unwrap();
        migrate_cn_schema(&tx).unwrap();
        tx.commit().unwrap();
    }
    for marker in ["cn_claimed_observed", "cn_reconciled"] {
        assert!(c
            .execute("UPDATE qoder_campaign_operations SET result=?1", [marker])
            .is_err());
        c.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        c.execute("UPDATE qoder_campaign_operations SET result=?1", [marker])
            .unwrap();
        assert!(read_rows(&c).is_err());
        c.execute("UPDATE qoder_campaign_operations SET result='unknown'", [])
            .unwrap();
        c.execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
    }
}

#[test]
fn published_main_v28_legacy_ledger_is_readable_before_cn_migration() {
    let c = old();
    let before = read_rows(&c).unwrap();
    version(&c, 28);
    assert_eq!(read_rows(&c).unwrap(), before);
    let tx = c.unchecked_transaction().unwrap();
    migrate_cn_schema(&tx).unwrap();
    tx.commit().unwrap();
    version(&c, 29);
    assert_eq!(read_rows(&c).unwrap(), before);
    assert_eq!(
        read_on(&c, &identity(Region::International, "new-provider")).unwrap()[0].outcome,
        Outcome::Unknown
    );
}
