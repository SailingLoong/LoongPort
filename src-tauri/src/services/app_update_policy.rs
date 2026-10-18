//! Bounded update transport policy, independent of the desktop runtime.

use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use std::{future::Future, time::Duration};
use tokio::{sync::watch, time::Instant};

pub const CHECK_BUDGET: Duration = Duration::from_secs(12);
pub const STARTUP_CHECK_BUDGET: Duration = Duration::from_secs(5);
pub const CHECK_HEDGE_DELAY: Duration = Duration::from_millis(750);
pub const DOWNLOAD_IDLE_BUDGET: Duration = Duration::from_secs(60);
pub const DOWNLOAD_TOTAL_BUDGET: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, PartialEq, Eq)]
pub enum CheckFailure<E> {
    TimedOut,
    Failed(Vec<E>),
    NoCandidates,
}

/// Each future checks one endpoint and transport. Dropping this aggregate also
/// drops every losing request: it never spawns detached updater work.
pub async fn race_update_checks<T, E, F>(
    checks: Vec<(Duration, F)>,
    budget: Duration,
    newer: impl Fn(&T, &T) -> bool,
) -> Result<Option<T>, CheckFailure<E>>
where
    F: Future<Output = Result<Option<T>, E>>,
{
    if checks.is_empty() {
        return Err(CheckFailure::NoCandidates);
    }
    let mut pending: FuturesUnordered<_> = checks
        .into_iter()
        .map(|(delay, check)| async move {
            tokio::time::sleep(delay).await;
            check.await
        })
        .collect();
    let mut errors = Vec::new();
    let deadline = tokio::time::sleep(budget);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = &mut deadline => return Err(CheckFailure::TimedOut),
            result = pending.next() => match result {
                Some(Ok(Some(mut update))) => {
                    // Compare results already ready now, without waiting for a
                    // slow backup or promising consensus across every source.
                    while let Some(Some(result)) = pending.next().now_or_never() {
                        if let Ok(Some(candidate)) = result {
                            if newer(&candidate, &update) { update = candidate; }
                        }
                    }
                    return Ok(Some(update));
                },
                Some(Ok(None)) => {},
                Some(Err(error)) => errors.push(error),
                None if errors.is_empty() => return Ok(None),
                None => return Err(CheckFailure::Failed(errors)),
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadFailure<E> {
    Stalled,
    TimedOut,
    Operation(E),
}

/// Bound the actual updater future, including time before its first byte.
/// Only increasing byte counts reset idle time; a running slow transfer keeps
/// its independent, much longer total budget. Success still comes from the
/// original future, after its signature validation has completed.
pub async fn bounded_download<T, E>(
    download: impl Future<Output = Result<T, E>>,
    mut progress: watch::Receiver<u64>,
    idle_budget: Duration,
    total_budget: Duration,
) -> Result<T, DownloadFailure<E>> {
    tokio::pin!(download);
    let total = tokio::time::sleep(total_budget);
    let idle = tokio::time::sleep(idle_budget);
    tokio::pin!(total, idle);
    let mut observed = *progress.borrow_and_update();
    let mut progress_open = true;
    loop {
        tokio::select! {
            biased;
            _ = &mut total => return Err(DownloadFailure::TimedOut),
            _ = &mut idle => return Err(DownloadFailure::Stalled),
            result = &mut download => return result.map_err(DownloadFailure::Operation),
            changed = progress.changed(), if progress_open => {
                if changed.is_err() {
                    progress_open = false;
                } else {
                    let downloaded = *progress.borrow_and_update();
                    if downloaded > observed {
                        observed = downloaded;
                        idle.as_mut().reset(Instant::now() + idle_budget);
                    }
                }
            },
        }
    }
}

/// Mirrors are derived only from the website's exact release-asset allowlist.
/// No query, credentials, encoded traversal, other repository or new host is
/// accepted, and the original manifest's tag and filename stay unchanged.
pub fn trusted_asset_pair(url: &str, version: &str) -> Option<(String, String)> {
    const OFFICIAL: &str = "https://loongport.dev/api/download/";
    const GITHUB: &str = "https://github.com/SailingLoong/LoongPort/releases/download/";
    let suffix = url
        .strip_prefix(OFFICIAL)
        .or_else(|| url.strip_prefix(GITHUB))?;
    let (tag, file) = suffix.split_once('/')?;
    let safe = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            && part != "."
            && part != ".."
    };
    if !safe(tag) || !safe(file) || tag != format!("v{version}") {
        return None;
    }
    Some((format!("{OFFICIAL}{suffix}"), format!("{GITHUB}{suffix}")))
}

pub fn is_withdrawn_version(version: &str) -> bool {
    version == "6.27.0-beta.3"
}

pub fn staged_version_floor<'a>(
    versions: impl IntoIterator<Item = &'a str>,
    installed: &str,
    beta: bool,
) -> Option<String> {
    let installed = semver::Version::parse(installed).ok()?;
    versions
        .into_iter()
        .filter(|version| !is_withdrawn_version(version))
        .filter_map(|version| semver::Version::parse(version).ok())
        .filter(|version| version > &installed && (beta || version.pre.is_empty()))
        .max()
        .map(|version| version.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRoute {
    pub url: String,
    pub source: &'static str,
    pub direct: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    Reachable,
    Unknown,
    Withdrawn,
}

pub fn probe_result(official: bool, status: Option<u16>) -> ProbeResult {
    match status {
        Some(410) if official => ProbeResult::Withdrawn,
        Some(200..=299) => ProbeResult::Reachable,
        _ => ProbeResult::Unknown,
    }
}

/// Wait for all bounded header probes before calling this: a fast GitHub result
/// must never bypass an official 410 that is still in flight. An unsupported or
/// failed Range probe stays eligible for its normal full GET.
pub fn rank_download_routes(
    mut routes: Vec<(DownloadRoute, ProbeResult, Duration)>,
    preferred: Option<&str>,
) -> Result<Vec<DownloadRoute>, &'static str> {
    if routes
        .iter()
        .any(|(_, result, _)| *result == ProbeResult::Withdrawn)
    {
        return Err("该版本已由官网撤回，请检查更新版本");
    }
    routes.sort_by_key(|(route, result, latency)| {
        (
            preferred.is_some_and(|source| source != route.source),
            *result != ProbeResult::Reachable,
            if *result == ProbeResult::Reachable {
                *latency
            } else {
                Duration::MAX
            },
            route.source != "official",
        )
    });
    Ok(routes.into_iter().map(|(route, _, _)| route).collect())
}

/// Called only after minisign has authenticated the trusted comment.
pub fn verify_signed_version(comment: &str, announced: &str, required: bool) -> Result<(), String> {
    let signed = comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"));
    let Some(signed) = signed else {
        return if required {
            Err("更新签名缺少版本信息".into())
        } else {
            Ok(())
        };
    };
    let matches = match (
        semver::Version::parse(signed.trim_start_matches('v')),
        semver::Version::parse(announced.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(announced)) => signed == announced,
        _ => signed == announced,
    };
    if matches {
        Ok(())
    } else {
        Err("更新签名版本与清单不符".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::{pending, BoxFuture};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tokio::sync::watch;

    type Check = BoxFuture<'static, Result<Option<u8>, &'static str>>;

    #[tokio::test]
    async fn simultaneously_ready_updates_choose_the_highest_version() {
        let checks: Vec<(Duration, Check)> = vec![
            (Duration::ZERO, Box::pin(async { Ok(Some(2)) })),
            (Duration::ZERO, Box::pin(async { Ok(Some(4)) })),
        ];
        assert_eq!(
            race_update_checks(checks, Duration::from_secs(1), |a, b| a > b)
                .await
                .unwrap(),
            Some(4)
        );
    }

    #[tokio::test]
    async fn delayed_backup_beats_a_stuck_primary_and_drops_it() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let primary: Check = Box::pin(async move {
            let _guard = guard;
            pending().await
        });
        let backup: Check = Box::pin(async { Ok(Some(2)) });
        assert_eq!(
            race_update_checks(
                vec![
                    (Duration::ZERO, primary),
                    (Duration::from_millis(1), backup)
                ],
                Duration::from_secs(1),
                |a, b| a > b
            )
            .await
            .unwrap(),
            Some(2)
        );
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn empty_or_invalid_manifest_does_not_win_over_an_available_update() {
        let checks: Vec<(Duration, Check)> = vec![
            (Duration::ZERO, Box::pin(async { Ok(None) })),
            (Duration::ZERO, Box::pin(async { Err("invalid manifest") })),
            (Duration::from_millis(1), Box::pin(async { Ok(Some(3)) })),
        ];
        assert_eq!(
            race_update_checks(checks, Duration::from_secs(1), |a, b| a > b)
                .await
                .unwrap(),
            Some(3)
        );
    }

    #[tokio::test]
    async fn empty_result_with_unfinished_candidates_is_an_incomplete_check() {
        let checks: Vec<(Duration, Check)> = vec![
            (Duration::ZERO, Box::pin(async { Ok(None) })),
            (Duration::ZERO, Box::pin(pending())),
        ];
        assert!(matches!(
            race_update_checks(checks, Duration::from_millis(5), |a, b| a > b).await,
            Err(CheckFailure::TimedOut)
        ));
    }

    #[tokio::test]
    async fn only_completed_consistent_empty_checks_report_up_to_date() {
        let empty: Vec<(Duration, Check)> = vec![
            (Duration::ZERO, Box::pin(async { Ok(None) })),
            (Duration::ZERO, Box::pin(async { Ok(None) })),
        ];
        assert_eq!(
            race_update_checks(empty, Duration::from_secs(1), |a, b| a > b)
                .await
                .unwrap(),
            None
        );
        let failed: Vec<(Duration, Check)> = vec![
            (Duration::ZERO, Box::pin(async { Ok(None) })),
            (Duration::ZERO, Box::pin(async { Err("HTTP 503") })),
        ];
        assert!(matches!(
            race_update_checks(failed, Duration::from_secs(1), |a, b| a > b).await,
            Err(CheckFailure::Failed(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_download_without_first_byte_ends_at_the_idle_deadline() {
        let (_tx, rx) = watch::channel(0);
        let result = bounded_download(
            pending::<Result<(), &str>>(),
            rx,
            Duration::from_millis(5),
            Duration::from_secs(1),
        );
        tokio::pin!(result);
        assert!(futures::poll!(&mut result).is_pending());
        tokio::time::advance(Duration::from_millis(5)).await;
        assert!(matches!(result.await, Err(DownloadFailure::Stalled)));
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_progress_does_not_extend_the_idle_deadline() {
        let (tx, rx) = watch::channel(0);
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_sent = sent.clone();
        let repeated = async move {
            loop {
                tx.send_replace(0);
                sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
            #[allow(unreachable_code)]
            Ok::<(), &str>(())
        };
        let result = bounded_download(
            repeated,
            rx,
            Duration::from_millis(5),
            Duration::from_secs(1),
        );
        tokio::pin!(result);
        assert!(futures::poll!(&mut result).is_pending());
        let mut previous = observed_sent.load(std::sync::atomic::Ordering::SeqCst);
        assert!(previous > 0);
        for _ in 0..4 {
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(futures::poll!(&mut result).is_pending());
            let current = observed_sent.load(std::sync::atomic::Ordering::SeqCst);
            assert!(
                current > previous,
                "progress was delivered before the deadline"
            );
            previous = current;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(result.await, Err(DownloadFailure::Stalled)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_continuous_download_can_outlive_one_idle_period() {
        let (tx, rx) = watch::channel(0);
        let transfer = async move {
            for n in 1..=6 {
                tokio::time::sleep(Duration::from_millis(5)).await;
                tx.send_replace(n);
            }
            Ok::<_, &str>("verified bytes")
        };
        let result = bounded_download(
            transfer,
            rx,
            Duration::from_millis(20),
            Duration::from_secs(1),
        );
        tokio::pin!(result);
        assert!(futures::poll!(&mut result).is_pending());
        // Poll every delivered chunk so scheduler latency is not confused with
        // a download that actually stopped making progress.
        for _ in 0..5 {
            tokio::time::advance(Duration::from_millis(5)).await;
            assert!(futures::poll!(&mut result).is_pending());
        }
        tokio::time::advance(Duration::from_millis(5)).await;
        assert_eq!(result.await.unwrap(), "verified bytes");
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_progress_cannot_escape_the_total_budget() {
        let (tx, rx) = watch::channel(0);
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_sent = sent.clone();
        let transfer = async move {
            let mut n = 0;
            loop {
                n += 1;
                tx.send_replace(n);
                sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
            #[allow(unreachable_code)]
            Ok::<(), &str>(())
        };
        let result = bounded_download(
            transfer,
            rx,
            Duration::from_secs(1),
            Duration::from_millis(5),
        );
        tokio::pin!(result);
        assert!(futures::poll!(&mut result).is_pending());
        let mut previous = observed_sent.load(std::sync::atomic::Ordering::SeqCst);
        assert!(previous > 0);
        for _ in 0..4 {
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(futures::poll!(&mut result).is_pending());
            let current = observed_sent.load(std::sync::atomic::Ordering::SeqCst);
            assert!(
                current > previous,
                "progress was delivered before the deadline"
            );
            previous = current;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(result.await, Err(DownloadFailure::TimedOut)));
    }

    #[tokio::test]
    async fn verification_failure_remains_an_error() {
        let (_tx, rx) = watch::channel(0);
        let result = bounded_download(
            async { Err::<(), _>("signature mismatch") },
            rx,
            Duration::from_secs(1),
            Duration::from_secs(2),
        )
        .await;
        assert!(matches!(
            result,
            Err(DownloadFailure::Operation("signature mismatch"))
        ));
    }

    #[test]
    fn mirrored_candidates_keep_the_exact_tag_and_filename() {
        let official = "https://loongport.dev/api/download/v6.27.0-beta.4/LoongPort-v6.27.0-beta.4-macOS.tar.gz";
        let github = "https://github.com/SailingLoong/LoongPort/releases/download/v6.27.0-beta.4/LoongPort-v6.27.0-beta.4-macOS.tar.gz";
        assert_eq!(
            trusted_asset_pair(official, "6.27.0-beta.4"),
            Some((official.into(), github.into()))
        );
        assert_eq!(
            trusted_asset_pair(github, "6.27.0-beta.4"),
            Some((official.into(), github.into()))
        );
    }

    #[test]
    fn unsafe_or_mismatched_asset_urls_have_no_derived_mirror() {
        for url in [
            "https://other.example/api/download/v1.0.0/app.tar.gz",
            "https://github.com/other/repo/releases/download/v1.0.0/app.tar.gz",
            "https://loongport.dev/api/download/v2.0.0/app.tar.gz",
            "https://loongport.dev/api/download/v1.0.0/app.tar.gz?token=x",
            "https://loongport.dev/api/download/v1.0.0/%2e%2e",
            "https://loongport.dev/api/download/v1.0.0/../app.tar.gz",
            "http://loongport.dev/api/download/v1.0.0/app.tar.gz",
        ] {
            assert!(trusted_asset_pair(url, "1.0.0").is_none(), "{url}");
        }
    }

    #[test]
    fn a_known_withdrawn_version_is_never_downloadable() {
        assert!(is_withdrawn_version("6.27.0-beta.3"));
        assert!(!is_withdrawn_version("6.27.0-beta.4"));
    }

    #[test]
    fn disk_floor_ignores_installed_withdrawn_and_other_channel_versions() {
        let versions = ["6.26.2", "6.27.0-beta.3", "6.27.0-beta.4", "invalid"];
        assert_eq!(
            staged_version_floor(versions, "6.26.2", true).as_deref(),
            Some("6.27.0-beta.4")
        );
        assert_eq!(staged_version_floor(versions, "6.26.2", false), None);
    }

    #[test]
    fn official_withdrawal_overrides_a_faster_reachable_github_probe() {
        let route = |source| DownloadRoute {
            url: String::new(),
            source,
            direct: false,
        };
        assert!(rank_download_routes(
            vec![
                (
                    route("github"),
                    ProbeResult::Reachable,
                    Duration::from_millis(1)
                ),
                (
                    route("official"),
                    ProbeResult::Withdrawn,
                    Duration::from_millis(50)
                ),
            ],
            None
        )
        .is_err());
    }

    #[test]
    fn range_probe_failures_remain_eligible_for_normal_get() {
        for status in [None, Some(403), Some(405), Some(416), Some(501)] {
            let result = probe_result(true, status);
            assert_eq!(result, ProbeResult::Unknown);
            let routes = rank_download_routes(
                vec![(
                    DownloadRoute {
                        url: "same-artifact".into(),
                        source: "official",
                        direct: false,
                    },
                    result,
                    Duration::ZERO,
                )],
                None,
            )
            .unwrap();
            assert_eq!(routes.len(), 1);
        }
    }

    #[test]
    fn signed_version_recheck_preserves_the_plugin_contract() {
        assert!(verify_signed_version("timestamp:1\tversion:v1.2.3", "1.2.3", true).is_ok());
        assert!(verify_signed_version("timestamp:1\tversion:1.2.2", "1.2.3", false).is_err());
        assert!(verify_signed_version("timestamp:1", "1.2.3", true).is_err());
        assert!(verify_signed_version("timestamp:1", "1.2.3", false).is_ok());
    }
}
