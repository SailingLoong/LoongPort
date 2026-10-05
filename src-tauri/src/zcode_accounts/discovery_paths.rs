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

#[cfg(test)]
mod tests {
    use super::{account_directory, installation_candidates};
    use std::path::{Path, PathBuf};

    #[test]
    fn absent_bootstrap_uses_os_account_home() {
        assert_eq!(
            account_directory(Path::new("/Users/synthetic"), None).unwrap(),
            PathBuf::from("/Users/synthetic/.zcode/v2")
        );
    }

    #[test]
    fn explicit_base_is_trimmed_and_expanded_once() {
        assert_eq!(
            account_directory(
                Path::new("/Users/synthetic"),
                Some("  /Volumes/Synthetic ZCode  ")
            )
            .unwrap(),
            PathBuf::from("/Volumes/Synthetic ZCode/.zcode/v2")
        );
    }

    #[test]
    fn bootstrap_trim_matches_desktop_javascript_semantics() {
        assert_eq!(
            account_directory(
                Path::new("/Users/synthetic"),
                Some("\u{feff}/Volumes/data\u{feff}")
            )
            .unwrap(),
            PathBuf::from("/Volumes/data/.zcode/v2")
        );
        assert_eq!(
            account_directory(Path::new("/Users/synthetic"), Some("/Volumes/data\u{85}")).unwrap(),
            PathBuf::from("/Volumes/data\u{85}/.zcode/v2")
        );
    }

    #[test]
    fn invalid_base_never_falls_back_to_home() {
        for invalid in [
            "",
            "  ",
            "relative",
            "/Volumes/../other",
            "/Volumes/./data",
            "/a\0b",
        ] {
            assert!(account_directory(Path::new("/Users/synthetic"), Some(invalid)).is_err());
        }
    }

    #[test]
    fn unverified_home_is_rejected_even_with_custom_base() {
        for invalid in ["", "relative", "/Users/../synthetic", "/Users/./synthetic"] {
            assert!(account_directory(Path::new(invalid), Some("/Volumes/data")).is_err());
            assert!(installation_candidates(Path::new(invalid), None).is_err());
        }
    }

    #[test]
    fn discovery_is_bounded_and_retains_explicit_choice_first() {
        assert_eq!(
            installation_candidates(
                Path::new("/Users/synthetic"),
                Some(Path::new("/Volumes/Tools/ZCode.app"))
            )
            .unwrap(),
            vec![
                PathBuf::from("/Volumes/Tools/ZCode.app"),
                PathBuf::from("/Applications/ZCode.app"),
                PathBuf::from("/Users/synthetic/Applications/ZCode.app"),
            ]
        );
    }

    #[test]
    fn saved_default_choice_is_not_duplicated() {
        assert_eq!(
            installation_candidates(
                Path::new("/Users/synthetic"),
                Some(Path::new("/Applications/ZCode.app"))
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn unsafe_saved_choice_is_not_silently_replaced() {
        assert!(installation_candidates(
            Path::new("/Users/synthetic"),
            Some(Path::new("relative/ZCode.app"))
        )
        .is_err());
    }
}
