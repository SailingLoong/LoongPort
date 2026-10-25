//! Shared dirty-signal batching, not a shared backend or upload state machine.
//! Each caller keeps its own bounded receiver, suppression state and upload loop.

use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

const AUTO_SYNC_DEBOUNCE: Duration = Duration::from_millis(1000);
const MAX_AUTO_SYNC_WAIT: Duration = Duration::from_millis(10_000);

// Await directly: cancelling this future after receiving a signal drops that
// batch, just as cancelling the original worker did. No caller races it in select.
pub(super) async fn next_auto_sync_batch(rx: &mut Receiver<String>) -> Option<(String, usize)> {
    let first_table = rx.recv().await?;
    let started_at = Instant::now();
    let mut merged_count = 1usize;

    loop {
        let elapsed = Instant::now().saturating_duration_since(started_at);
        if elapsed >= MAX_AUTO_SYNC_WAIT {
            break;
        }
        let wait_for = AUTO_SYNC_DEBOUNCE.min(MAX_AUTO_SYNC_WAIT - elapsed);
        match tokio::time::timeout(wait_for, rx.recv()).await {
            Ok(Some(_)) => merged_count += 1,
            // Preserve the existing shutdown behavior: do not upload a partial
            // batch after the sender has closed.
            Ok(None) => return None,
            Err(_) => break,
        }
    }

    Some((first_table, merged_count))
}

#[cfg(test)]
mod tests {
    use super::next_auto_sync_batch;
    use std::time::Duration;
    use tokio::sync::mpsc::channel;
    use tokio::time::{advance, Instant};

    #[tokio::test(start_paused = true)]
    async fn waits_for_one_second_of_quiet() {
        let started = Instant::now();
        let (tx, mut rx) = channel(1);
        tx.send("providers".to_string()).await.unwrap();
        let batch = tokio::spawn(async move { next_auto_sync_batch(&mut rx).await });
        tokio::task::yield_now().await;
        advance(Duration::from_millis(999)).await;
        assert!(!batch.is_finished());
        advance(Duration::from_millis(1)).await;
        assert_eq!(batch.await.unwrap(), Some(("providers".to_string(), 1)));
        assert_eq!(started.elapsed(), Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_quiet_period_without_replacing_first_table() {
        let started = Instant::now();
        let (tx, mut rx) = channel(1);
        tx.send("providers".to_string()).await.unwrap();
        let batch = tokio::spawn(async move { next_auto_sync_batch(&mut rx).await });
        tokio::task::yield_now().await;
        advance(Duration::from_millis(500)).await;
        tx.send("skills".to_string()).await.unwrap();
        tokio::task::yield_now().await;
        advance(Duration::from_millis(500)).await;
        assert!(!batch.is_finished());
        advance(Duration::from_millis(500)).await;
        assert_eq!(batch.await.unwrap(), Some(("providers".to_string(), 2)));
        assert_eq!(started.elapsed(), Duration::from_millis(1500));
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_changes_flush_at_ten_seconds() {
        let started = Instant::now();
        let (tx, mut rx) = channel(1);
        tx.send("providers".to_string()).await.unwrap();
        let batch = tokio::spawn(async move { next_auto_sync_batch(&mut rx).await });
        tokio::task::yield_now().await;
        for _ in 0..19 {
            advance(Duration::from_millis(500)).await;
            tx.send("settings".to_string()).await.unwrap();
            tokio::task::yield_now().await;
            assert!(!batch.is_finished());
        }
        advance(Duration::from_millis(500)).await;
        assert_eq!(batch.await.unwrap(), Some(("providers".to_string(), 20)));
        assert_eq!(started.elapsed(), Duration::from_millis(10000));
    }

    #[tokio::test(start_paused = true)]
    async fn closed_channel_does_not_flush_an_incomplete_batch() {
        let (tx, mut rx) = channel(1);
        tx.send("providers".to_string()).await.unwrap();
        drop(tx);
        assert_eq!(next_auto_sync_batch(&mut rx).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn receivers_keep_independent_batches() {
        let (first_tx, mut first_rx) = channel(1);
        let (second_tx, mut second_rx) = channel(1);
        first_tx.send("providers".to_string()).await.unwrap();
        let first = tokio::spawn(async move { next_auto_sync_batch(&mut first_rx).await });
        let second = tokio::spawn(async move { next_auto_sync_batch(&mut second_rx).await });
        tokio::task::yield_now().await;
        advance(Duration::from_secs(1)).await;
        assert_eq!(first.await.unwrap(), Some(("providers".to_string(), 1)));
        assert!(!second.is_finished());
        second_tx.send("skills".to_string()).await.unwrap();
        tokio::task::yield_now().await;
        advance(Duration::from_secs(1)).await;
        assert_eq!(second.await.unwrap(), Some(("skills".to_string(), 1)));
    }

    #[tokio::test(start_paused = true)]
    async fn next_round_retains_one_change_queued_between_batches() {
        let (tx, mut rx) = channel(1);
        tx.send("providers".to_string()).await.unwrap();
        assert_eq!(
            next_auto_sync_batch(&mut rx).await,
            Some(("providers".to_string(), 1))
        );
        // The backend uploads between calls. Its existing bounded channel retains
        // one dirty signal whether that upload succeeds, fails, or is disabled.
        tx.try_send("skills".to_string()).unwrap();
        assert!(tx.try_send("settings".to_string()).is_err());
        assert_eq!(
            next_auto_sync_batch(&mut rx).await,
            Some(("skills".to_string(), 1))
        );
    }
}
