//! One temporary native-attachment gesture, never an account store or journal.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Reason {
    UnsupportedPlatform,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    UnsafePath,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Io,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Journal,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    SourceChanged,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    RecoveryRequired,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    RequestConflict,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    RolledBack,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountSummary {
    id: String,
    label: String,
    realm: String,
}
#[derive(Clone, Serialize)]
pub(crate) struct Saved {
    selection: super::native_source::SelectionKey,
    label: String,
    realm: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Preparing,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Ready,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Applying,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Saved,
    Unknown,
    #[cfg(any(
        test,
        all(target_os = "macos", target_arch = "aarch64", feature = "gui")
    ))]
    Cancelled,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    flow_id: Option<String>,
    state: Phase,
    available: bool,
    account: Option<AccountSummary>,
    saved: Option<Saved>,
    reason: Option<Reason>,
    expires_in_ms: u64,
}
#[derive(Default)]
pub(crate) struct State {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    slot: std::sync::Mutex<Option<slot::Entry<supported::Prepared>>>,
}

#[cfg(any(test, feature = "gui"))]
fn unknown(flow_id: Option<String>, available: bool, reason: Option<Reason>) -> View {
    View {
        flow_id,
        state: Phase::Unknown,
        available,
        account: None,
        saved: None,
        reason,
        expires_in_ms: 0,
    }
}

#[cfg(any(
    all(test, unix),
    all(target_os = "macos", target_arch = "aarch64", feature = "gui")
))]
mod slot;
#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
mod supported;

#[cfg(feature = "gui")]
pub(crate) async fn prepare(
    state: std::sync::Arc<State>,
    db: std::sync::Arc<crate::database::Database>,
    id: String,
) -> Result<View, Reason> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(Reason::UnsupportedPlatform);
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        supported::prepare(state, db, id).await
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (state, db, id);
        Err(Reason::UnsupportedPlatform)
    }
}
#[cfg(feature = "gui")]
pub(crate) async fn confirm(
    state: std::sync::Arc<State>,
    db: std::sync::Arc<crate::database::Database>,
    flow_id: String,
) -> Result<View, Reason> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        supported::confirm(state, db, flow_id).await
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (state, db, flow_id);
        Err(Reason::UnsupportedPlatform)
    }
}
#[cfg(feature = "gui")]
pub(crate) fn inspect(
    state: &State,
    session: &std::sync::Arc<crate::secrets::session::SecretSession>,
    flow_id: Option<&str>,
) -> View {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        supported::inspect(state, session, flow_id)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (state, session);
        unknown(
            flow_id.map(str::to_owned),
            false,
            Some(Reason::UnsupportedPlatform),
        )
    }
}
#[cfg(feature = "gui")]
pub(crate) fn cancel(
    state: &State,
    session: &std::sync::Arc<crate::secrets::session::SecretSession>,
    flow_id: &str,
) -> View {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        supported::cancel(state, session, flow_id)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (state, session);
        unknown(
            Some(flow_id.into()),
            false,
            Some(Reason::UnsupportedPlatform),
        )
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn attachment_reasons_and_phases_keep_the_ipc_wire_names() {
        for (reason, wire) in [
            (Reason::UnsupportedPlatform, "unsupportedPlatform"),
            (Reason::UnsafePath, "unsafePath"),
            (Reason::Io, "io"),
            (Reason::Journal, "journal"),
            (Reason::SourceChanged, "sourceChanged"),
            (Reason::RecoveryRequired, "recoveryRequired"),
            (Reason::RequestConflict, "requestConflict"),
            (Reason::RolledBack, "rolledBack"),
        ] {
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                serde_json::json!(wire)
            );
        }
        for (phase, wire) in [
            (Phase::Preparing, "preparing"),
            (Phase::Ready, "ready"),
            (Phase::Applying, "applying"),
            (Phase::Saved, "saved"),
            (Phase::Unknown, "unknown"),
            (Phase::Cancelled, "cancelled"),
        ] {
            assert_eq!(
                serde_json::to_value(phase).unwrap(),
                serde_json::json!(wire)
            );
        }
    }
}
