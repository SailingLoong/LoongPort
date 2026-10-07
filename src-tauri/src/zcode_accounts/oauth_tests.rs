use super::*;

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
