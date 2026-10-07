use super::*;

fn saved_input() -> SavedCodingInput {
    let home = super::super::synthetic_test_path("saved-home");
    let context = LibraryContext::from_os_identity(
        home.to_str().unwrap(),
        "synthetic-user",
        &home.join(".zcode/v2"),
    )
    .unwrap();
    let native = context.cipher().unwrap();
    let identity = super::super::core::AccountIdentity::new(
        native.context(),
        OAuthFamily::BigModel,
        "declared-cache-account",
    )
    .unwrap();
    let mut fields = BTreeMap::new();
    let keys = identity.credential_keys();
    for (index, plaintext) in [
        (0, "bigmodel"),
        (1, "saved-business"),
        (
            3,
            r#"{"id":"declared-cache-account","username":"Saved account","displayName":"Saved account"}"#,
        ),
        (4, "saved-jwt"),
        (6, "saved-jwt"),
    ] {
        fields.insert(keys[index].clone(), native.encrypt(plaintext).unwrap());
    }
    let snapshot = native
        .inspect(
            &super::super::core::CredentialDocument::parse(&serde_json::to_vec(&fields).unwrap())
                .unwrap(),
        )
        .unwrap();
    SavedCodingInput {
        target: SavedCodingTarget {
            snapshot,
            revision: "saved-revision".into(),
            details: super::super::checkpoint::ProfileDetails::default(),
        },
        context,
        app_version: None,
    }
}

#[test]
fn saved_coding_begin_reuses_target_without_any_oauth_ready_or_poll_token() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let first = store.begin_saved(binding(), saved_input(), now).unwrap();
    assert!(first.needs_prepare);
    let second = store.begin_saved(binding(), saved_input(), now).unwrap();
    assert!(!second.needs_prepare);
    assert_eq!(first.flow_id, second.flow_id);
    let lease = store
        .acquire(&first.flow_id, WorkKind::Prepare, now)
        .unwrap();
    let (_, draft) = lease.draft().unwrap();
    assert!(draft.ready.is_none() && draft.init.is_none() && draft.poll_token.is_none());
    assert_eq!(draft.business.unwrap().expose(), "saved-business");
    let progress = store.progress(&first.flow_id, now).unwrap();
    assert_eq!(progress.purpose, LoginPurpose::CompleteCoding);
    assert_eq!(progress.account.unwrap().identity_source, "packageDeclared");
    let mut changed = saved_input();
    changed.target.revision = "changed-revision".into();
    let existing = store.begin_saved(binding(), changed, now).unwrap();
    assert_eq!(existing.flow_id, first.flow_id);
    assert_eq!(
        store
            .progress(&first.flow_id, now)
            .unwrap()
            .source_catalog_revision
            .as_deref(),
        Some("saved-revision")
    );
}

#[test]
fn saved_coding_uncertain_cancelled_target_cannot_start_a_second_creation_flow() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let first = store.begin_saved(binding(), saved_input(), now).unwrap();
    let prepare = store
        .acquire(&first.flow_id, WorkKind::Prepare, now)
        .unwrap();
    prepare.finish(FlowStage::KeyRequired, |_| {}).unwrap();
    let create = store
        .acquire(&first.flow_id, WorkKind::CreateKey, now)
        .unwrap();
    create.mark_key_intent().unwrap();
    store.cancel(&first.flow_id, now).unwrap();
    drop(create);
    let second = store.begin_saved(binding(), saved_input(), now).unwrap();
    assert!(second.needs_prepare);
    assert_eq!(first.flow_id, second.flow_id);
    assert!(store.progress(&first.flow_id, now).unwrap().key_may_exist);
}

#[test]
fn saved_coding_old_saved_receipt_does_not_shadow_a_new_current_revision() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let target = saved_input();
    let target_id = target.target.snapshot.identity().opaque_id();
    let context_id = target.context.context_id().to_owned();
    let first = store.begin_saved(binding(), target, now).unwrap();
    let prepare = store
        .acquire(&first.flow_id, WorkKind::Prepare, now)
        .unwrap();
    prepare.finish(FlowStage::Review, |_| {}).unwrap();
    store
        .acquire(&first.flow_id, WorkKind::Save, now)
        .unwrap()
        .saved(target_id.clone(), now)
        .unwrap();
    assert_eq!(
        store
            .saved_entry(&binding(), &context_id, &target_id, "saved-revision", now)
            .unwrap()
            .as_deref(),
        Some(first.flow_id.as_str())
    );
    assert!(store
        .saved_entry(
            &binding(),
            &context_id,
            &target_id,
            "new-current-revision",
            now
        )
        .unwrap()
        .is_none());
    let mut next = saved_input();
    next.target.revision = "new-current-revision".into();
    let second = store.begin_saved(binding(), next, now).unwrap();
    assert_ne!(first.flow_id, second.flow_id);
    assert!(second.needs_prepare);
}

fn binding() -> VaultBinding {
    VaultBinding {
        root: super::super::synthetic_test_path("vault"),
        vault_id: "synthetic-vault".into(),
        key_id: "synthetic-key".into(),
        revision: 1,
    }
}
fn flow(store: &LoginFlowStore, now: Instant) -> String {
    store
        .begin(
            binding(),
            OAuthFamily::BigModel,
            PollToken::new("synthetic-poll-token").unwrap(),
            now,
        )
        .unwrap()
}
fn waiting(store: &LoginFlowStore, now: Instant) -> String {
    let id = flow(store, now);
    let init = store.acquire(&id, WorkKind::Init, now).unwrap();
    init.finish(FlowStage::Waiting, |_| {}).unwrap();
    id
}

#[test]
fn old_completed_poll_drop_cannot_release_successor_in_same_flow() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = waiting(&store, now);
    let old = store.acquire(&id, WorkKind::Poll, now).unwrap();
    old.finish(FlowStage::Waiting, |_| {}).unwrap();
    let next = store.acquire(&id, WorkKind::Poll, now).unwrap();
    drop(old);
    assert!(store.status(&id, now).unwrap().busy);
    assert!(matches!(
        store.acquire(&id, WorkKind::Poll, now),
        Err(FlowError::Busy)
    ));
    assert_eq!(next.check(), Ok(()));
}

#[test]
fn dropped_unpolled_worker_releases_only_its_own_lease() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = waiting(&store, now);
    let lease = store.acquire(&id, WorkKind::Poll, now).unwrap();
    let future = async move {
        let _held = lease;
        std::future::pending::<()>().await;
    };
    assert!(store.status(&id, now).unwrap().busy);
    drop(future);
    assert!(!store.status(&id, now).unwrap().busy);
    assert!(store.acquire(&id, WorkKind::Poll, now).is_ok());
}

#[test]
fn cancelled_work_cannot_publish_or_send_but_keeps_remote_key_fact() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = flow(&store, now);
    let init = store.acquire(&id, WorkKind::Init, now).unwrap();
    init.finish(FlowStage::KeyRequired, |_| {}).unwrap();
    let key = store.acquire(&id, WorkKind::CreateKey, now).unwrap();
    key.mark_key_intent().unwrap();
    store.cancel(&id, now).unwrap();
    assert_eq!(key.check(), Err(FlowError::Cancelled));
    assert_eq!(
        key.finish(FlowStage::Review, |_| panic!("late mutation")),
        Err(FlowError::Cancelled)
    );
    key.mark_key_created().unwrap();
    let status = store.status(&id, now).unwrap();
    assert_eq!(status.stage, FlowStage::Cancelled);
    assert!(status.key_created && status.key_may_exist);
}

#[test]
fn cancelled_before_send_can_clear_only_uncreated_key_intent() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = flow(&store, now);
    let init = store.acquire(&id, WorkKind::Init, now).unwrap();
    init.finish(FlowStage::KeyRequired, |_| {}).unwrap();
    let key = store.acquire(&id, WorkKind::CreateKey, now).unwrap();
    key.mark_key_intent().unwrap();
    store.cancel(&id, now).unwrap();
    key.mark_key_not_sent().unwrap();
    assert!(!store.status(&id, now).unwrap().key_may_exist);
}

#[test]
fn committed_save_receipt_survives_original_flow_expiry_and_cancel() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = flow(&store, now);
    let init = store.acquire(&id, WorkKind::Init, now).unwrap();
    init.finish(FlowStage::Review, |_| {}).unwrap();
    let save = store
        .acquire(&id, WorkKind::Save, now + Duration::from_secs(599))
        .unwrap();
    let committed_at = now + Duration::from_secs(601);
    save.saved("synthetic-account".into(), committed_at)
        .unwrap();
    store.cancel(&id, committed_at).unwrap();
    let status = store
        .status(&id, committed_at + Duration::from_secs(10))
        .unwrap();
    assert_eq!(status.stage, FlowStage::Saved);
    assert_eq!(
        status.saved_account_id.as_deref(),
        Some("synthetic-account")
    );
    assert!(matches!(
        store.acquire(&id, WorkKind::Save, committed_at),
        Err(FlowError::WrongStage)
    ));
}

#[test]
fn newer_flow_is_untouched_by_cancelled_older_work() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let old_id = waiting(&store, now);
    let old = store.acquire(&old_id, WorkKind::Poll, now).unwrap();
    store.cancel(&old_id, now).unwrap();
    let new_id = waiting(&store, now);
    let next = store.acquire(&new_id, WorkKind::Poll, now).unwrap();
    assert_eq!(
        old.finish(FlowStage::Review, |_| panic!("late write")),
        Err(FlowError::Cancelled)
    );
    drop(old);
    assert_eq!(next.check(), Ok(()));
    assert!(store.status(&new_id, now).unwrap().busy);
}

#[test]
fn abandoned_live_work_is_not_evicted_to_make_capacity() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let a = flow(&store, now);
    let b = flow(&store, now);
    let held_a = store.acquire(&a, WorkKind::Init, now).unwrap();
    let held_b = store.acquire(&b, WorkKind::Init, now).unwrap();
    assert!(matches!(
        store.begin(
            binding(),
            OAuthFamily::BigModel,
            PollToken::new("new-poll-token").unwrap(),
            now + FLOW_LIFETIME
        ),
        Err(FlowError::Capacity)
    ));
    drop(held_a);
    drop(held_b);
    assert!(store
        .begin(
            binding(),
            OAuthFamily::BigModel,
            PollToken::new("new-poll-token").unwrap(),
            now + FLOW_LIFETIME
        )
        .is_ok());
}

#[test]
fn later_lease_cannot_clear_prior_unknown_key_creation() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = flow(&store, now);
    store
        .acquire(&id, WorkKind::Init, now)
        .unwrap()
        .finish(FlowStage::KeyRequired, |_| {})
        .unwrap();
    let original = store.acquire(&id, WorkKind::CreateKey, now).unwrap();
    original.mark_key_intent().unwrap();
    drop(original); // A possibly successful POST lost its reply.
    let later = store.acquire(&id, WorkKind::CreateKey, now).unwrap();
    assert_eq!(later.mark_key_not_sent(), Err(FlowError::WrongStage));
    assert!(store.status(&id, now).unwrap().key_may_exist);
    assert_eq!(later.mark_key_intent(), Err(FlowError::WrongStage));
}

#[test]
fn observing_expiry_erases_draft_and_cancels_outstanding_work() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = waiting(&store, now);
    let work = store.acquire(&id, WorkKind::Poll, now).unwrap();
    let _ = store.status(&id, now + FLOW_LIFETIME);
    assert!(matches!(
        work.check(),
        Err(FlowError::Expired | FlowError::Cancelled)
    ));
    let flows = store.0.lock().unwrap();
    let expired = flows.get(&id).unwrap();
    assert!(
        expired.draft.is_none(),
        "expired credentials must not await another login"
    );
    assert!(expired.cancelled.load(Ordering::Acquire));
}

#[test]
fn timer_expiry_clears_secrets_without_another_user_action() {
    let now = Instant::now();
    let store = LoginFlowStore::default();
    let id = waiting(&store, now);
    store.expire_due(now + FLOW_LIFETIME).unwrap();
    {
        let flows = store.0.lock().unwrap();
        let expired = flows.get(&id).unwrap();
        assert!(expired.draft.is_none());
        assert!(expired.cancelled.load(Ordering::Acquire));
        assert_eq!(expired.stage, FlowStage::Expired);
    }
    store
        .expire_due(now + FLOW_LIFETIME + RECEIPT_LIFETIME)
        .unwrap();
    assert!(!store.0.lock().unwrap().contains_key(&id));
}

#[test]
fn runtime_error_receipt_cannot_fail_newer_work_or_a_committed_save() {
    let store = LoginFlowStore::default();
    let now = Instant::now();
    let id = waiting(&store, now);
    let old = store.acquire(&id, WorkKind::Poll, now).unwrap();
    let ticket = old.ticket();
    drop(old);
    let newer = store.acquire(&id, WorkKind::Poll, now).unwrap();
    store.fail_abandoned(&ticket).unwrap();
    assert!(newer.check().is_ok());
    newer.finish(FlowStage::Review, |_| {}).unwrap();
    let save = store.acquire(&id, WorkKind::Save, now).unwrap();
    let ticket = save.ticket();
    save.saved("saved-profile".into(), now).unwrap();
    drop(save);
    store.fail_abandoned(&ticket).unwrap();
    assert_eq!(store.progress(&id, now).unwrap().phase, "saved");
    let id = waiting(&store, now);
    let abandoned = store.acquire(&id, WorkKind::Poll, now).unwrap();
    let ticket = abandoned.ticket();
    drop(abandoned);
    store.fail_abandoned(&ticket).unwrap();
    assert_eq!(store.progress(&id, now).unwrap().phase, "failed");
}

#[test]
fn reopened_login_finds_only_latest_same_vault_and_library_after_cancel() {
    let owner = LoginFlowStore::default();
    let now = Instant::now();
    let binding = binding();
    let old = owner
        .begin(
            binding.clone(),
            OAuthFamily::Zai,
            PollToken::new("synthetic-reopen-poll").unwrap(),
            now,
        )
        .unwrap();
    owner.bind_context(&old, "library-a").unwrap();
    owner.cancel(&old, now).unwrap();
    let newer = owner
        .begin(
            binding.clone(),
            OAuthFamily::Zai,
            PollToken::new("synthetic-reopen-poll").unwrap(),
            now + Duration::from_millis(1),
        )
        .unwrap();
    owner.bind_context(&newer, "library-b").unwrap();
    assert_eq!(owner.latest(&binding, "library-a", now).unwrap(), Some(old));
    assert_eq!(
        owner.latest(&binding, "library-b", now).unwrap(),
        Some(newer)
    );
    let mut changed = binding.clone();
    changed.revision += 1;
    assert!(owner.latest(&changed, "library-a", now).unwrap().is_none());
    assert!(owner
        .latest(&binding, "other-library", now)
        .unwrap()
        .is_none());
}
