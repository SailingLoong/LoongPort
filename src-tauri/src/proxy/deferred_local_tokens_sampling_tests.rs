use super::{count_request_tokens, DeferredLocalTokens};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn deferred_sampling_preserves_original_json() {
    let mut body =
        serde_json::json!({"model":"original-alias", "input":"original input", "tools":[]});
    let expected = count_request_tokens(&body);
    let sample = DeferredLocalTokens::new(&body);
    body["model"] = serde_json::json!("mapped-model");
    body["input"] = serde_json::json!("different transformed input ".repeat(100));
    assert_eq!(sample.resolve().await.unwrap(), expected);
    assert_eq!(sample.clone().resolve().await.unwrap(), expected);
    assert_eq!(DeferredLocalTokens::ready(17).resolve().await.unwrap(), 17);
}

#[test]
fn deferred_sampling_unresolved_drop_releases_input_without_counting() {
    let input = Arc::new(vec![1_u8; 1024]);
    let weak = Arc::downgrade(&input);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let sample = DeferredLocalTokens::from_count(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        input.len() as u64
    });
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(weak.upgrade().is_some());
    drop(sample);
    assert!(weak.upgrade().is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deferred_sampling_clones_share_one_job_after_waiter_cancellation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let sample = DeferredLocalTokens::from_count(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        let _ = started_tx.send(());
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("release blocking counter");
        23
    });
    let first = sample.clone();
    let waiter = tokio::spawn(async move { first.resolve().await });
    tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    // This runs on a current-thread runtime while the count is blocked elsewhere.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    let other = sample.clone();
    let (left, right) = tokio::join!(sample.resolve(), other.resolve());
    assert_eq!(left.unwrap(), 23);
    assert_eq!(right.unwrap(), 23);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn deferred_sampling_failed_job_is_shared_and_not_successful_zero() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let sample = DeferredLocalTokens::from_count(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        panic!("synthetic sampling failure")
    });
    let first = sample
        .resolve()
        .await
        .expect_err("panic must remain an error");
    let second = sample
        .clone()
        .resolve()
        .await
        .expect_err("error must be cached");
    assert!(first.is_panic());
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sample.resolve_for_usage().await,
        0,
        "usage persistence retains explicit unsampled convention"
    );
}
