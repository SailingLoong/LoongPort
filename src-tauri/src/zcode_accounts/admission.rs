//! ZCode-specific admission facts. No filesystem, environment, process or credential reads.
//! Observations come from the backend probe; none of these types are IPC inputs.
#[cfg(test)]
use super::core::AccountIdentity;
use super::core::{js_trim, OAuthFamily, StrictRecord};
use super::native::NativeCipher;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Platform {
    MacOs,
    #[cfg(test)]
    Linux,
    #[cfg(test)]
    Windows,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct BuildFingerprint {
    pub platform: Platform,
    pub version: String,
    pub build: String,
    pub artifact_sha256: [u8; 32],
}
#[derive(Clone)]
pub(super) struct ContractEntry {
    pub fingerprint: BuildFingerprint,
    pub native_gate_passed: bool,
}
/// A user's requested verification mode, never proof about an earlier process.
#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum KeyMode {
    Standard,
    Custom,
    Unknown,
}
// Unsupported production platforms return before making any writer observation.
// Keep strict dead-code checks for the implemented macOS producer and all tests.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriterState {
    Stopped,
    Running,
    Unknown,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ContextObservation {
    pub install: BuildFingerprint,
    pub credential_root: PathBuf,
    pub settings_file: PathBuf,
    pub root_identity: [u64; 2],
    pub settings_identity: [u64; 2],
    pub home: String,
    pub settings_home: String,
    pub bootstrap_home: String,
    pub username: String,
    pub key_choice: KeyMode,
    pub writers: WriterState,
    pub settings: Vec<u8>,
}
pub(super) trait ContextProbe: Send + Sync {
    /// Read only installation/storage/process/settings evidence, never credentials.
    fn observe(&self) -> Result<ContextObservation, BlockedReason>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockedReason {
    SelectContext,
    UnsupportedPlatform,
    UnsupportedBuild,
    NativeGatePending,
    RootUnverified,
    KeyContextUnknown,
    CustomKeyContext,
    SettingsInvalid,
    LegacySelection,
    SelectionMissing,
    TeamUnsupported,
    AppRunning,
    WriterStateUnknown,
    ContextChanged,
    #[cfg(test)]
    TargetScopeMismatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Remedy {
    ChooseContext,
    UseVerifiedBuild,
    FinishPlatformCheck,
    ReviewDataLocation,
    UseStandardContext,
    OpenNativeSettings,
    QuitNativeWriters,
    VerifyWriterState,
    RefreshContext,
    #[cfg(test)]
    ChooseSavedAccount,
}
impl BlockedReason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::SelectContext => "zcode.account.select_context",
            Self::UnsupportedPlatform => "zcode.account.unsupported_platform",
            Self::UnsupportedBuild => "zcode.account.unsupported_build",
            Self::NativeGatePending => "zcode.account.native_gate_pending",
            Self::RootUnverified => "zcode.account.root_unverified",
            Self::KeyContextUnknown => "zcode.account.key_context_unknown",
            Self::CustomKeyContext => "zcode.account.custom_key_context",
            Self::SettingsInvalid => "zcode.account.settings_invalid",
            Self::LegacySelection => "zcode.account.legacy_selection",
            Self::SelectionMissing => "zcode.account.selection_missing",
            Self::TeamUnsupported => "zcode.account.team_unsupported",
            Self::AppRunning => "zcode.account.app_running",
            Self::WriterStateUnknown => "zcode.account.writer_state_unknown",
            Self::ContextChanged => "zcode.account.context_changed",
            #[cfg(test)]
            Self::TargetScopeMismatch => "zcode.account.target_scope_mismatch",
        }
    }
    pub(crate) fn remedy(self) -> Remedy {
        match self {
            Self::SelectContext => Remedy::ChooseContext,
            Self::UnsupportedBuild => Remedy::UseVerifiedBuild,
            Self::UnsupportedPlatform | Self::NativeGatePending => Remedy::FinishPlatformCheck,
            Self::RootUnverified => Remedy::ReviewDataLocation,
            Self::KeyContextUnknown | Self::CustomKeyContext => Remedy::UseStandardContext,
            Self::SettingsInvalid
            | Self::LegacySelection
            | Self::SelectionMissing
            | Self::TeamUnsupported => Remedy::OpenNativeSettings,
            Self::AppRunning => Remedy::QuitNativeWriters,
            Self::WriterStateUnknown => Remedy::VerifyWriterState,
            Self::ContextChanged => Remedy::RefreshContext,
            #[cfg(test)]
            Self::TargetScopeMismatch => Remedy::ChooseSavedAccount,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSettings {
    #[serde(default, deserialize_with = "present_string")]
    data_base_dir: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    provider_family_domain: Option<String>,
    provider_family_connection_selections: Option<StrictRecord<StrictRecord<serde_json::Value>>>,
    #[serde(default, deserialize_with = "present")]
    model_provider_family_modes: bool,
    #[serde(default, deserialize_with = "present")]
    model_provider_family_selected_keys: bool,
}
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}
fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
fn absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().is_empty()
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}
pub(super) struct VerifiedContext {
    observation: ContextObservation,
    family: OAuthFamily,
    context_id: String,
    settings_revision: [u8; 32],
}
impl VerifiedContext {
    pub(super) fn assess(
        mut observation: ContextObservation,
        contracts: &[ContractEntry],
    ) -> Result<Self, BlockedReason> {
        let settings_bytes = Zeroizing::new(std::mem::take(&mut observation.settings));
        if observation.install.platform != Platform::MacOs {
            return Err(BlockedReason::UnsupportedPlatform);
        }
        let contract = contracts
            .iter()
            .find(|entry| entry.fingerprint == observation.install)
            .ok_or(BlockedReason::UnsupportedBuild)?;
        if !contract.native_gate_passed {
            return Err(BlockedReason::NativeGatePending);
        }
        if observation.home != observation.settings_home
            || observation.home != observation.bootstrap_home
            || observation.home.contains('\0')
            || !absolute_path(Path::new(&observation.home))
            || !absolute_path(&observation.credential_root)
            || observation.settings_file
                != Path::new(&observation.settings_home).join(".zcode/v2/setting.json")
        {
            return Err(BlockedReason::RootUnverified);
        }
        match observation.key_choice {
            KeyMode::Custom => return Err(BlockedReason::CustomKeyContext),
            KeyMode::Unknown => return Err(BlockedReason::KeyContextUnknown),
            KeyMode::Standard => {}
        }
        if observation.username.is_empty() || observation.username.contains('\0') {
            return Err(BlockedReason::KeyContextUnknown);
        }
        match observation.writers {
            WriterState::Running => return Err(BlockedReason::AppRunning),
            WriterState::Unknown => return Err(BlockedReason::WriterStateUnknown),
            WriterState::Stopped => {}
        }
        if settings_bytes.len() > 1024 * 1024 {
            return Err(BlockedReason::SettingsInvalid);
        }
        let settings: NativeSettings =
            serde_json::from_slice(&settings_bytes).map_err(|_| BlockedReason::SettingsInvalid)?;
        let data_base = match settings.data_base_dir.as_deref() {
            Some(base) if !js_trim(base).is_empty() => js_trim(base),
            Some(_) => return Err(BlockedReason::SettingsInvalid),
            None => &observation.home,
        };
        if data_base.contains('\0')
            || !absolute_path(Path::new(data_base))
            || observation.credential_root != Path::new(data_base).join(".zcode/v2")
        {
            return Err(BlockedReason::RootUnverified);
        }
        if settings.provider_family_connection_selections.is_none()
            && (settings.model_provider_family_modes
                || settings.model_provider_family_selected_keys)
        {
            return Err(BlockedReason::LegacySelection);
        }
        let (family, name) = match settings.provider_family_domain.as_deref() {
            Some("zai") => (OAuthFamily::Zai, "zai"),
            Some("bigmodel") => (OAuthFamily::BigModel, "bigmodel"),
            _ => return Err(BlockedReason::SelectionMissing),
        };
        let selections = settings
            .provider_family_connection_selections
            .ok_or(BlockedReason::SelectionMissing)?;
        let selected = selections
            .0
            .get(name)
            .ok_or(BlockedReason::SelectionMissing)?;
        match selected.0.get("kind").and_then(serde_json::Value::as_str) {
            Some("team-coding-plan") => return Err(BlockedReason::TeamUnsupported),
            Some("start-plan" | "individual-coding-plan") if selected.0.len() == 1 => {}
            _ => return Err(BlockedReason::SelectionMissing),
        }
        // Length-framed JSON avoids ambiguous delimiters. Persistent context excludes
        // current family and settings revision, so both saved families retain identity.
        let identity = serde_json::to_vec(&(
            "zcode-credential-v1",
            "darwin",
            &observation.home,
            &observation.username,
            &observation.credential_root,
        ))
        .map_err(|_| BlockedReason::RootUnverified)?;
        let context_id = URL_SAFE_NO_PAD.encode(Sha256::digest(identity));
        let settings_revision = Sha256::digest(&*settings_bytes).into();
        Ok(Self {
            observation,
            family,
            context_id,
            settings_revision,
        })
    }
    pub(super) fn recheck(
        &self,
        observation: ContextObservation,
        contracts: &[ContractEntry],
    ) -> Result<(), BlockedReason> {
        let current = Self::assess(observation, contracts)?;
        if current.observation != self.observation
            || current.settings_revision != self.settings_revision
        {
            return Err(BlockedReason::ContextChanged);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn accept_target(&self, target: &AccountIdentity) -> Result<(), BlockedReason> {
        if !target.matches_scope(&self.context_id, self.family) {
            return Err(BlockedReason::TargetScopeMismatch);
        }
        Ok(())
    }

    pub(super) fn confirm_root(&self, actual: [u64; 2]) -> Result<(), BlockedReason> {
        if actual != self.observation.root_identity {
            return Err(BlockedReason::ContextChanged);
        }
        Ok(())
    }

    pub(super) fn native_root(&self) -> &Path {
        &self.observation.credential_root
    }
    pub(super) fn family(&self) -> OAuthFamily {
        self.family
    }
    pub(super) fn context_id(&self) -> &str {
        &self.context_id
    }
    /// Opaque click-time binding; no registry, secret or historical launch claim.
    pub(super) fn context_revision(&self) -> String {
        let observation = &self.observation;
        let binding = serde_json::to_vec(&(
            "zcode-context-revision-v1",
            &self.context_id,
            &observation.install.version,
            &observation.install.build,
            observation.install.artifact_sha256,
            &observation.credential_root,
            observation.root_identity,
            &observation.settings_file,
            observation.settings_identity,
            self.settings_revision,
        ))
        .expect("fixed serializable context fields");
        URL_SAFE_NO_PAD.encode(Sha256::digest(binding))
    }
    pub(super) fn cipher(&self) -> Result<NativeCipher, BlockedReason> {
        // Never read an environment variable or try another source after a failure.
        // Keep exact Node homedir/username text; macOS is "darwin" in Node's contract.
        let secret = Zeroizing::new(format!(
            "zcode-credential-fallback:darwin:{}:{}",
            self.observation.home, self.observation.username
        ));
        NativeCipher::new(&self.context_id, &secret).map_err(|_| BlockedReason::KeyContextUnknown)
    }
}
#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
