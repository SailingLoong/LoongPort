//! Upstream Gemini direct writer using the shared mode transaction owner.
use super::gemini_auth::is_google_official_gemini;
use crate::app_config::AppType;
use crate::error::AppError;
use crate::gemini_config::{
    get_gemini_env_path, get_gemini_settings_path, validate_gemini_settings,
    validate_gemini_settings_strict,
};
use crate::live::engine::LiveFile;
use crate::live::project::gemini::GeminiProjection;
use crate::mode::operation::{self, AppWrite, FileChange, OperationReport, RecoveryOutcome};
use crate::mode::state::{op, PendingTarget};
use crate::provider::Provider;
use crate::store::AppState;

pub(crate) fn env_file() -> LiveFile {
    LiveFile::private(get_gemini_env_path())
}

pub(crate) fn settings_file() -> LiveFile {
    LiveFile::shared(get_gemini_settings_path())
}

pub(crate) fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official") || is_google_official_gemini(provider)
}

pub(crate) fn projection(provider: &Provider) -> Result<GeminiProjection, AppError> {
    validate_gemini_settings(&provider.settings_config)?;
    let official = is_official(provider);
    if !official {
        validate_gemini_settings_strict(&provider.settings_config)?;
    }
    Ok(GeminiProjection::of(&provider.settings_config, official))
}

/// Caller owns the application switch lock; no common-fragment merge/backfill.
pub(crate) fn switch_to(state: &AppState, target: &Provider) -> Result<OperationReport, AppError> {
    let projection = projection(target)?;
    let write = AppWrite::begin(state, &AppType::Gemini)?;
    let env = projection.env_patch();
    let settings = projection.settings_patch();
    write.run(
        op::SWITCH,
        &[
            FileChange {
                file: env_file(),
                patch: &env,
            },
            FileChange {
                file: settings_file(),
                patch: &settings,
            },
        ],
        PendingTarget::pointer(Some(target.id.clone())),
    )
}

#[allow(dead_code)] // Controlled recovery registration is a later integration.
pub(crate) fn recover_pending(state: &AppState) -> Result<Option<RecoveryOutcome>, AppError> {
    operation::recover_pending(state, &AppType::Gemini, &[env_file(), settings_file()])
}
