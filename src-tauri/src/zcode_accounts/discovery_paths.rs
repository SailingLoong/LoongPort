//! Bounded, pure candidate construction. Paths here are not evidence of trust.
//! The native adapter must still verify identity, canonical paths and artifacts.
use super::desktop_text::js_trim;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum InvalidSourcePath {
    Home,
    DataBase,
    Installation,
}

fn absolute_source(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|value| {
            !value.contains('\0') && !value.split('/').any(|part| part == "." || part == "..")
        })
}

/// `home` comes from the OS account; `bootstrap_base` is the allowlisted
/// desktop bootstrap setting, never LoongPort's inherited process environment.
/// A present but invalid base must not silently select a different account root.
pub(super) fn account_directory(
    home: &Path,
    bootstrap_base: Option<&str>,
) -> Result<PathBuf, InvalidSourcePath> {
    if !absolute_source(home) {
        return Err(InvalidSourcePath::Home);
    }
    let base = match bootstrap_base {
        Some(value) => {
            let base = Path::new(js_trim(value));
            if !absolute_source(base) {
                return Err(InvalidSourcePath::DataBase);
            }
            base
        }
        None => home,
    };
    Ok(base.join(".zcode").join("v2"))
}

/// A small explicit candidate list, with no filesystem scan or execution.
/// Selection is not automatic: multiple verified candidates require a choice.
pub(super) fn installation_candidates(
    home: &Path,
    selected: Option<&Path>,
) -> Result<Vec<PathBuf>, InvalidSourcePath> {
    if !absolute_source(home) {
        return Err(InvalidSourcePath::Home);
    }
    let mut paths = Vec::with_capacity(3);
    if let Some(path) = selected {
        if !absolute_source(path) {
            return Err(InvalidSourcePath::Installation);
        }
        paths.push(path.to_path_buf());
    }
    for path in [
        PathBuf::from("/Applications/ZCode.app"),
        home.join("Applications/ZCode.app"),
    ] {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}
