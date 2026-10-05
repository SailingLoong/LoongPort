//! Lightweight behavioral tests: no filesystem, native processes or credentials.
#[path = "../../src-tauri/src/zcode_accounts/desktop_text.rs"]
mod desktop_text;
#[path = "../../src-tauri/src/zcode_accounts/discovery_paths.rs"]
mod discovery_paths;

#[cfg(test)]
mod tests {
    use super::discovery_paths::{account_directory, installation_candidates};
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
