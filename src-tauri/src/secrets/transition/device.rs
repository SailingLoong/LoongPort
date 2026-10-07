//! Device-specific preconditions on the existing generation transaction.
use super::{destination, hash, invalid, Destination, Manifest, FORMAT, LEGACY_FORMAT};
use crate::{
    error::AppError,
    secrets::{files, owned_file::DeviceFile, VaultContext},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(super) struct Roots<'a> {
    pub data: &'a Path,
    pub device: &'a Path,
}

/// Root paths live only in the encrypted manifest, never the public intent.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RootBindings {
    data: PathBuf,
    device: PathBuf,
}

impl Roots<'_> {
    pub fn bindings(self) -> RootBindings {
        RootBindings {
            data: self.data.to_owned(),
            device: self.device.to_owned(),
        }
    }
    pub fn validate(self) -> Result<(), AppError> {
        validate_root(self.data)?;
        validate_root(self.device)
    }
}

fn conflict() -> AppError {
    AppError::Config("secret.device_transition_conflict".into())
}

/// Reject aliases before any canonicalization, including the home resolver's
/// relative fallback. Missing device directories remain a read-only empty set.
fn validate_root(path: &Path) -> Result<(), AppError> {
    if !root_spelling_is_pinned(path) {
        return Err(invalid());
    }
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(AppError::io(ancestor, e)),
        }
    }
    Ok(())
}

pub(super) fn validate_bindings(
    roots: Roots<'_>,
    version: u32,
    manifest: &Manifest,
) -> Result<(), AppError> {
    roots.validate()?;
    match (version, &manifest.roots) {
        (FORMAT, Some(binding))
            if binding.data.as_os_str() == roots.data.as_os_str()
                && binding.device.as_os_str() == roots.device.as_os_str() =>
        {
            Ok(())
        }
        (LEGACY_FORMAT, None)
            if !manifest
                .artifacts
                .iter()
                .any(|a| matches!(a.destination, Destination::Device { .. }))
                && files::device_file_paths(roots.device)?.is_empty() =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}

pub(super) fn validate_digest(digest: &str) -> Result<(), AppError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_membership(roots: Roots<'_>, manifest: &Manifest) -> Result<(), AppError> {
    roots.validate()?;
    let expected = manifest
        .artifacts
        .iter()
        .filter_map(|a| match &a.destination {
            Destination::Device { relative, .. } => {
                Some(DeviceFile::registered(relative).map(|f| roots.device.join(f.relative_path())))
            }
            _ => None,
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let actual = files::device_file_paths(roots.device)?
        .into_iter()
        .map(|(_, p)| p)
        .collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(conflict());
    }
    Ok(())
}

/// Mixed old/next ciphertext is expected on interrupted replay, but a third
/// value or membership change is never authority to overwrite external data.
pub(super) fn validate_sources(roots: Roots<'_>, manifest: &Manifest) -> Result<(), AppError> {
    validate_membership(roots, manifest)?;
    for artifact in &manifest.artifacts {
        if let Destination::Device { source_digest, .. } = &artifact.destination {
            validate_digest(source_digest)?;
            let path = destination(roots, &artifact.destination)?.ok_or_else(invalid)?;
            let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
            let digest = hash(&bytes);
            if digest != *source_digest && digest != artifact.digest {
                return Err(conflict());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_installed_device_files(
    roots: Roots<'_>,
    version: u32,
    manifest: &Manifest,
    next: &VaultContext,
) -> Result<(), AppError> {
    validate_bindings(roots, version, manifest)?;
    validate_membership(roots, manifest)?;
    for artifact in &manifest.artifacts {
        if let Destination::Device { relative, .. } = &artifact.destination {
            let file = DeviceFile::registered(relative)?;
            let path = roots.device.join(file.relative_path());
            let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
            if hash(&bytes) != artifact.digest {
                return Err(conflict());
            }
            file.decode(next, &bytes)?;
        }
    }
    Ok(())
}

fn root_spelling_is_pinned(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_spelling_requires_an_absolute_unaliased_native_path() {
        let base = std::env::temp_dir().canonicalize().unwrap();
        let root = base.join("device-root");
        assert!(root_spelling_is_pinned(&base));
        assert!(root_spelling_is_pinned(&root));
        for relative in ["", ".", "device-root", "../device-root"] {
            assert!(!root_spelling_is_pinned(Path::new(relative)));
        }
        let separator = std::path::MAIN_SEPARATOR;
        for tail in [
            format!("{separator}..{separator}other"),
            format!("{separator}.{separator}child"),
            format!("{separator}{separator}child"),
        ] {
            let mut alias = root.as_os_str().to_os_string();
            alias.push(tail);
            assert!(!root_spelling_is_pinned(Path::new(&alias)));
        }
    }

    #[test]
    #[cfg(windows)]
    fn root_spelling_accepts_windows_drive_unc_and_verbatim_roots() {
        for root in [
            r"C:\",
            r"C:\Users\fixture\.loongport",
            r"\\server\share\",
            r"\\server\share\.loongport",
            r"\\?\C:\",
            r"\\?\C:\Users\fixture\.loongport",
            r"\\?\UNC\server\share\",
            r"\\?\UNC\server\share\.loongport",
        ] {
            assert!(root_spelling_is_pinned(Path::new(root)), "{root}");
        }
        for alias in [
            r"C:fixture",
            r"\fixture",
            r"C:\Users\fixture\.\.loongport",
            r"\\?\C:\Users\fixture\..\.loongport",
        ] {
            assert!(!root_spelling_is_pinned(Path::new(alias)), "{alias}");
        }
    }
}
