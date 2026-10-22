//! Bounded desktop path discovery; never opens storage.json or generates device IDs.
//! Path facts adapted from dingminhua/dsh-connect-trae@7a9553eadb7081c6c1f27652a95f97104e8136ed
//! src/paths.ts and src/identity.ts (MIT; licenses/dsh-connect-trae-MIT.txt).

use super::{
    import::{storage_directory_matches, Edition},
    store::Realm,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Installation {
    storage_path: PathBuf,
    product_path: PathBuf,
    version: String,
}
fn mac_candidates(home: &Path, cn: bool) -> Vec<(PathBuf, PathBuf)> {
    let name = if cn { "Trae CN" } else { "Trae" };
    let storage = home
        .join("Library/Application Support")
        .join(name)
        .join("User/globalStorage/storage.json");
    [PathBuf::from("/Applications"), home.join("Applications")]
        .into_iter()
        .map(|root| {
            (
                storage.clone(),
                root.join(format!("{name}.app"))
                    .join("Contents/Resources/app/product.json"),
            )
        })
        .collect()
}
fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}
fn windows_roots(default: PathBuf, configured: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in configured
        .into_iter()
        .map(Path::to_path_buf)
        .chain([default])
    {
        if root.is_absolute()
            && !roots
                .iter()
                .any(|p: &PathBuf| path_key(p) == path_key(&root))
        {
            roots.push(root);
        }
    }
    roots
}
fn windows_candidates(
    home: &Path,
    roaming: Option<&Path>,
    local: Option<&Path>,
    cn: bool,
) -> Vec<(PathBuf, PathBuf)> {
    let names: &[&str] = if cn {
        &["Trae CN", "trae-cn"]
    } else {
        &["Trae"]
    };
    let roaming = windows_roots(home.join("AppData/Roaming"), roaming);
    let local = windows_roots(home.join("AppData/Local"), local);
    let mut result = Vec::new();
    for data_root in &roaming {
        for data_name in names {
            for install_root in &local {
                for install_name in names {
                    result.push((
                        data_root
                            .join(data_name)
                            .join("User/globalStorage/storage.json"),
                        install_root
                            .join("Programs")
                            .join(install_name)
                            .join("resources/app/product.json"),
                    ));
                }
            }
        }
    }
    result
}
fn regular(path: &Path) -> bool {
    // Do not follow links in candidate components. Missing/denied paths are not candidates.
    if !path.is_absolute() {
        return false;
    }
    for ancestor in path.ancestors() {
        let Ok(meta) = std::fs::symlink_metadata(ancestor) else {
            return false;
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return false;
            }
        }
        if meta.file_type().is_symlink() {
            return false;
        }
        if ancestor == path {
            if !meta.is_file() {
                return false;
            }
        } else if !meta.is_dir() {
            return false;
        }
    }
    true
}
fn inspect(storage: &Path, product: &Path, cn: bool) -> Option<Installation> {
    inspect_metadata(storage, product, cn, false)
}
fn inspect_windows(storage: &Path, product: &Path, cn: bool) -> Option<Installation> {
    inspect_metadata(storage, product, cn, true)
}
fn inspect_metadata(
    storage: &Path,
    product: &Path,
    cn: bool,
    windows: bool,
) -> Option<Installation> {
    // Metadata only for the credential file, including its existence and size.
    if !regular(storage)
        || !regular(product)
        || std::fs::symlink_metadata(storage).ok()?.len() > 4 * 1024 * 1024
    {
        return None;
    }
    let raw = crate::config_file_io::read_regular_file(product, 1024 * 1024).ok()??;
    let product_info: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let expected = if cn { "TRAE_CN" } else { "TRAE_I18N" };
    if product_info.get("packageType")?.as_str()? != expected {
        return None;
    }
    if cn
        && (product_info.get("quality")?.as_str()? != "stable"
            || product_info
                .pointer("/iCubeApp/authConfig/TRAE/stable")?
                .as_str()?
                != "ono9krqynydwx5")
    {
        return None;
    }
    if windows {
        let edition = if cn { Edition::Cn } else { Edition::Global };
        let data_name = storage.ancestors().nth(3)?.file_name()?.to_str()?;
        let install_name = product.ancestors().nth(3)?.file_name()?.to_str()?;
        let declared_name = product_info.get("win32DirName")?.as_str()?;
        if !storage_directory_matches(data_name, edition, true)
            || !storage_directory_matches(install_name, edition, true)
            || !storage_directory_matches(declared_name, edition, true)
            || !data_name.eq_ignore_ascii_case(declared_name)
        {
            return None;
        }
    }
    let version = product_info.get("appVersion")?.as_str()?;
    if version.trim().is_empty() || version.len() > 128 || version.chars().any(char::is_control) {
        return None;
    }
    Some(Installation {
        storage_path: storage.into(),
        product_path: product.into(),
        version: version.into(),
    })
}
/// No recursion, native credential access, CLI/SOLO fallback, or network request.
/// Other platforms retain explicit selected-file import until their pairs are verified.
pub(crate) fn discover(realm: Realm) -> Vec<Installation> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let cn = realm == Realm::Cn;
    match std::env::consts::OS {
        "macos" => mac_candidates(&home, cn)
            .into_iter()
            .filter_map(|(s, p)| inspect(&s, &p, cn))
            .collect(),
        "windows" => {
            let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
            let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
            let mut seen = std::collections::HashSet::new();
            windows_candidates(&home, roaming.as_deref(), local.as_deref(), cn)
                .into_iter()
                .filter_map(|(s, p)| inspect_windows(&s, &p, cn))
                .filter(|item| {
                    seen.insert((path_key(&item.storage_path), path_key(&item.product_path)))
                })
                .collect()
        }
        // Reliable public product layout has not been established for Linux.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let storage =
            root.join("Library/Application Support/Trae CN/User/globalStorage/storage.json");
        let product = root.join("Applications/Trae CN.app/Contents/Resources/app/product.json");
        std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
        std::fs::create_dir_all(product.parent().unwrap()).unwrap();
        std::fs::write(&storage, "NOT JSON: SYNTHETIC SECRET MUST NOT BE READ").unwrap();
        std::fs::write(&product, r#"{"packageType":"TRAE_CN","quality":"stable","appVersion":"3.6.2","iCubeApp":{"authConfig":{"TRAE":{"stable":"ono9krqynydwx5"}}}}"#).unwrap();
        (tmp, storage, product)
    }
    #[test]
    fn metadata_discovery_accepts_unreadable_content_without_exposing_it() {
        let (_tmp, storage, product) = fixture();
        let candidate = inspect(&storage, &product, true).unwrap();
        let safe = serde_json::to_string(&candidate).unwrap();
        assert!(!safe.contains("SECRET"));
        assert!(safe.contains("3.6.2"));
        assert!(inspect(&storage, &product, false).is_none());
    }
    #[test]
    fn discovery_rejects_symlinks_and_unsupported_product() {
        let (_tmp, storage, product) = fixture();
        std::fs::write(
            &product,
            r#"{"packageType":"TRAE_SOLO_CN","appVersion":"3.6.2"}"#,
        )
        .unwrap();
        assert!(inspect(&storage, &product, true).is_none());
        #[cfg(unix)]
        {
            std::fs::remove_file(&storage).unwrap();
            std::os::unix::fs::symlink(&product, &storage).unwrap();
            assert!(!regular(&storage));
        }
    }
    #[test]
    fn mac_candidates_are_finite_and_pair_only_matching_edition() {
        let paths = mac_candidates(std::path::Path::new("/synthetic-home"), true);
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(
            |(s, p)| s.ends_with("Trae CN/User/globalStorage/storage.json")
                && p.ends_with("Trae CN.app/Contents/Resources/app/product.json")
        ));
    }
    #[test]
    fn windows_candidates_are_bounded_deduplicated_and_do_not_mix_editions() {
        let home = Path::new(if cfg!(windows) {
            "C:/synthetic-home"
        } else {
            "/synthetic-home"
        });
        assert!(home.is_absolute());
        let roaming = home.join("AppData/Roaming");
        let local = home.join("AppData/Local");
        let cn = windows_candidates(home, Some(&roaming), Some(&local), true);
        assert_eq!(cn.len(), 4);
        assert!(cn
            .iter()
            .all(|(s, p)| !s.to_string_lossy().contains("SOLO")
                && !p.to_string_lossy().contains("SOLO")));
        let global = windows_candidates(home, Some(&roaming), Some(&local), false);
        assert_eq!(global.len(), 1);
        assert!(global[0]
            .0
            .ends_with("Trae/User/globalStorage/storage.json"));
        assert!(global[0]
            .1
            .ends_with("Programs/Trae/resources/app/product.json"));
    }
    #[test]
    fn windows_pairing_requires_public_installation_metadata_and_never_reads_storage() {
        let (_tmp, storage, original_product) = fixture();
        let product = _tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("Programs/Trae CN/resources/app/product.json");
        std::fs::create_dir_all(product.parent().unwrap()).unwrap();
        std::fs::copy(original_product, &product).unwrap();
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&product).unwrap()).unwrap();
        assert!(inspect_windows(&storage, &product, true).is_none());
        metadata["win32DirName"] = serde_json::json!("Trae CN");
        std::fs::write(&product, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(inspect_windows(&storage, &product, true).is_some());
        metadata["win32DirName"] = serde_json::json!("Trae");
        std::fs::write(&product, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(inspect_windows(&storage, &product, true).is_none());
        assert!(inspect_windows(&storage, &product, false).is_none());
    }
    #[test]
    fn windows_aliases_and_case_never_admit_solo_or_cross_edition() {
        use super::super::import::{storage_directory_matches, Edition};
        assert!(storage_directory_matches("TRAE-CN", Edition::Cn, true));
        assert!(storage_directory_matches("trae cn", Edition::Cn, true));
        assert!(storage_directory_matches("TRAE", Edition::Global, true));
        assert!(!storage_directory_matches(
            "TRAE SOLO CN",
            Edition::Cn,
            true
        ));
        assert!(!storage_directory_matches("Trae", Edition::Cn, true));
        assert!(!storage_directory_matches("trae-cn", Edition::Global, true));
        assert!(!storage_directory_matches("trae-cn", Edition::Cn, false));
    }
    #[test]
    fn windows_fixture_multiple_installs_missing_files_and_metadata_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let paths = windows_candidates(&home, None, None, true);
        assert!(paths
            .iter()
            .all(|(s, p)| inspect_windows(s, p, true).is_none()));
        let metadata = serde_json::json!({"packageType":"TRAE_CN","quality":"stable","appVersion":"3.6.2","win32DirName":"TRAE CN","iCubeApp":{"authConfig":{"TRAE":{"stable":"ono9krqynydwx5"}}}});
        for (storage, product) in &paths {
            if storage.ends_with("Trae CN/User/globalStorage/storage.json") {
                std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
                std::fs::write(storage, "NOT JSON: SYNTHETIC SECRET").unwrap();
            }
            std::fs::create_dir_all(product.parent().unwrap()).unwrap();
            std::fs::write(product, serde_json::to_vec(&metadata).unwrap()).unwrap();
        }
        let found = paths
            .iter()
            .filter_map(|(s, p)| inspect_windows(s, p, true))
            .collect::<Vec<_>>();
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|v| !serde_json::to_string(v).unwrap().contains("SECRET")));
        std::fs::remove_file(&found[0].product_path).unwrap();
        assert_eq!(
            paths
                .iter()
                .filter_map(|(s, p)| inspect_windows(s, p, true))
                .count(),
            1
        );
    }
    #[test]
    fn windows_global_fixture_never_becomes_cn_or_solo() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let (storage, product) = windows_candidates(&home, None, None, false).remove(0);
        std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
        std::fs::create_dir_all(product.parent().unwrap()).unwrap();
        std::fs::write(&storage, "SYNTHETIC NOT JSON").unwrap();
        let mut metadata = serde_json::json!({"packageType":"TRAE_I18N","appVersion":"3.6.2","win32DirName":"trae"});
        std::fs::write(&product, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(inspect_windows(&storage, &product, false).is_some());
        assert!(inspect_windows(&storage, &product, true).is_none());
        metadata["packageType"] = serde_json::json!("TRAE_SOLO");
        std::fs::write(&product, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(inspect_windows(&storage, &product, false).is_none());
    }
}
