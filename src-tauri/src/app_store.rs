use serde_json::Value;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::error::AppError;

/// Nonsecret scope path metadata; not a credential-protection policy or proof.
pub(crate) const ISOLATED_SCOPE_IDENTITY_FILE: &str = "scope-identity.json";

#[path = "app_store/isolated_scope.rs"]
pub(crate) mod isolated_scope;
static ACTIVE_SCOPE: isolated_scope::ScopeCache = isolated_scope::ScopeCache::new();

/// Must run before logger, settings, device, or database bootstrap. Both success
/// and failure are frozen for this process; publication only applies on restart.
pub(crate) fn initialize_active_scope() -> Result<(), AppError> {
    ACTIVE_SCOPE.initialize_optional_path(tauri_store_path().as_deref())
}

pub(crate) fn active_scope_root() -> Result<Option<PathBuf>, AppError> {
    initialize_active_scope()?;
    ACTIVE_SCOPE.root()
}

/// Compatibility boundary for old infallible path APIs. Runtime/bootstrap
/// entrypoints must check initialize_active_scope first and surface its error.
/// A missed guard must fail closed, never silently access the preserved root.
fn isolated_root_for_paths() -> Option<PathBuf> {
    active_scope_root()
        .expect("active configuration scope is unavailable; startup must retain the scope error")
}

pub(crate) fn device_config_root(legacy: PathBuf) -> PathBuf {
    isolated_root_for_paths().unwrap_or(legacy)
}

/// Store 中的键名
const STORE_KEY_APP_CONFIG_DIR: &str = "app_config_dir_override";

/// 缓存当前的 app_config_dir 覆盖路径，避免存储 AppHandle
static APP_CONFIG_DIR_OVERRIDE: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn override_cache() -> &'static RwLock<Option<PathBuf>> {
    APP_CONFIG_DIR_OVERRIDE.get_or_init(|| RwLock::new(None))
}

fn update_cached_override(value: Option<PathBuf>) {
    if let Ok(mut guard) = override_cache().write() {
        *guard = value;
    }
}

/// Isolated tests only: restore the process cache before their home is removed.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct TestAppConfigRoot(Option<PathBuf>);
#[cfg(any(test, feature = "test-hooks"))]
impl TestAppConfigRoot {
    pub(crate) fn enter(root: PathBuf) -> Self {
        let home = std::env::var_os("CC_SWITCH_TEST_HOME").expect("isolated test home");
        assert!(root.starts_with(PathBuf::from(home)));
        let previous = get_app_config_dir_override();
        update_cached_override(Some(root));
        Self(previous)
    }
}
#[cfg(any(test, feature = "test-hooks"))]
impl Drop for TestAppConfigRoot {
    fn drop(&mut self) {
        update_cached_override(self.0.take());
    }
}

/// 获取缓存中的 app_config_dir 覆盖路径
pub fn get_app_config_dir_override() -> Option<PathBuf> {
    if let Some(root) = isolated_root_for_paths() {
        return Some(root);
    }
    override_cache().read().ok()?.clone()
}

/// **不依赖 Tauri** 地读出用户设的数据目录覆盖。
///
/// ## 为什么需要它（review 抓出的一个静默失效）
///
/// [`get_app_config_dir_override`] 读的是**进程内的缓存**，而那个缓存只由
/// [`refresh_app_config_dir_override`] 填 —— 它要 `AppHandle`，也就是只在
/// `run()` 里能调。
///
/// 生图 MCP server 是同一个二进制的另一个入口（`--mcp-image-gen`），它**在 `run()`
/// 之前就分流走了**、没有 Tauri app ⇒ 那个缓存永远是空的 ⇒
/// [`crate::config::get_app_config_dir`] 回落到默认 `~/.loongport`。
///
/// 于是设过「LoongPort 配置目录」的用户会遇到两种**都不报错**的结果：
/// - 默认路径下没有库 ⇒ MCP 报「找不到数据库，请先启动 LoongPort 并登录」，
///   而他明明已经登录了 —— 一句他照做也没用的话；
/// - 默认路径下还留着**旧库** ⇒ 读到过期的档位与密钥，静默用错账号。
///
/// 所以这里绕过缓存，直接读 `app_paths.json` 那个 store 文件 —— 它就是覆盖值的落盘处，
/// 纯 JSON（`tauri-plugin-store` 不加密），位置由 bundle identifier 推出来。
///
/// 返回 `None` = 没设过覆盖（绝大多数用户），调用方用默认目录。
pub fn read_app_config_dir_override_without_tauri() -> Option<PathBuf> {
    if let Some(root) = isolated_root_for_paths() {
        return Some(root);
    }
    legacy_override_at(&tauri_store_path()?, false)
}

fn legacy_override_at(store_path: &std::path::Path, accept_existing_file: bool) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(store_path).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    let path_str = json.get(STORE_KEY_APP_CONFIG_DIR)?.as_str()?.trim();
    if path_str.is_empty() {
        return None;
    }
    let path = resolve_path(path_str);
    isolated_scope::validate_legacy_root_at(store_path, &path)
        .expect("managed configuration root requires a committed isolated selector");
    // Preserve both original legacy contracts: GUI accepts an existing path
    // (including a file, which must retain its original later error), while the
    // headless reader requires a directory. Explicit isolated scopes never use
    // either fallback; their validation errors are retained above.
    (if accept_existing_file {
        path.exists()
    } else {
        path.is_dir()
    })
    .then_some(path)
}

/// `app_paths.json` 在磁盘上的位置。
///
/// **与 `tauri-plugin-store` 的默认落盘位置必须一致** —— 它把 store 放在
/// `app_config_dir()` 下，而那是 OS 约定 + `tauri.conf.json` 的 `identifier` 推出来的：
///
/// | 平台 | 位置 |
/// |---|---|
/// | macOS | `~/Library/Application Support/<identifier>/` |
/// | Windows | `%APPDATA%\<identifier>\` |
/// | Linux | `~/.config/<identifier>/` |
///
/// ⚠️ identifier 从 `tauri.conf.json` 编译期读进来（`include_str!` + 解析），
/// **不写字面量** —— 那个值改了这里不跟着改就会静默读不到覆盖，而症状是
/// 「设了数据目录但生图还是用默认库」。
pub(crate) fn tauri_store_path() -> Option<PathBuf> {
    const TAURI_CONF: &str = include_str!("../tauri.conf.json");
    let identifier = serde_json::from_str::<Value>(TAURI_CONF)
        .ok()?
        .get("identifier")?
        .as_str()?
        .to_string();

    // Same explicit test-home convention as config::get_home_dir. No tests may
    // inspect or publish a real user's selector when their data root is isolated.
    if let Some(home) = std::env::var_os("CC_SWITCH_TEST_HOME") {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join(identifier)
                .join("app_paths.json"),
        );
    }

    #[cfg(target_os = "macos")]
    let base = dirs::home_dir()?
        .join("Library")
        .join("Application Support");
    #[cfg(target_os = "windows")]
    let base = dirs::config_dir()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = dirs::config_dir()?;

    Some(base.join(identifier).join("app_paths.json"))
}

#[cfg(feature = "gui")]
fn read_override_from_store(_app: &tauri::AppHandle) -> Option<PathBuf> {
    // Do not retain a tauri-plugin-store snapshot of this control document: a
    // delayed save could erase an independently published isolated selection.
    if let Some(root) = isolated_root_for_paths() {
        return Some(root);
    }
    legacy_override_at(&tauri_store_path()?, true)
}

/// 从 Store 刷新 app_config_dir 覆盖值并更新缓存
#[cfg(feature = "gui")]
pub fn refresh_app_config_dir_override(app: &tauri::AppHandle) -> Option<PathBuf> {
    let value = read_override_from_store(app);
    update_cached_override(value.clone());
    value
}

/// 写入 app_config_dir 到 Tauri Store
#[cfg(feature = "gui")]
pub fn set_app_config_dir_to_store(
    app: &tauri::AppHandle,
    path: Option<&str>,
) -> Result<(), AppError> {
    initialize_active_scope()?;
    let store = tauri_store_path()
        .ok_or_else(|| AppError::Config("config.store_path_unavailable".into()))?;
    isolated_scope::write_legacy_override_at(&store, path)?;
    refresh_app_config_dir_override(app);
    Ok(())
}

/// 解析路径，支持 ~ 开头的相对路径
fn resolve_path(raw: &str) -> PathBuf {
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if let Some(stripped) = raw.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }

    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `app_paths.json` 的位置由 `tauri.conf.json` 的 `identifier` 推出来，
    /// 而那是**跨文件的同一事实**（AGENTS.md §三点六）。
    ///
    /// 这道闸盯住三件事，任一条破了 `read_app_config_dir_override_without_tauri`
    /// 就会静默读不到覆盖 —— 症状是「用户设了数据目录，但生图 MCP 还是用默认库」，
    /// 没有任何东西会报错：
    ///
    /// 1. `tauri.conf.json` 里仍有 `identifier`（不是被挪进平台专属的 conf 文件了）
    /// 2. 它非空
    /// 3. 拼出来的路径确实以 `app_paths.json` 结尾（`tauri-plugin-store` 的默认落盘名，
    ///    与 `read_override_from_store` 里那个 `store_builder("app_paths.json")` 同一个）
    #[test]
    fn legacy_direct_reader_cannot_bypass_managed_namespace_after_cache_init() {
        let temp = crate::secrets::testing::tempdir().unwrap();
        let store = temp.path().join("app_paths.json");
        let candidate = temp
            .path()
            .join("isolated-configs")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(
            &store,
            serde_json::to_vec(&serde_json::json!({STORE_KEY_APP_CONFIG_DIR:candidate})).unwrap(),
        )
        .unwrap();
        assert!(std::panic::catch_unwind(|| legacy_override_at(&store, true)).is_err());
        assert!(std::panic::catch_unwind(|| legacy_override_at(&store, false)).is_err());
    }

    #[test]
    fn legacy_gui_file_override_keeps_its_original_error_path_not_home_fallback() {
        let temp = crate::secrets::testing::tempdir().unwrap();
        let file = temp.path().join("file-not-directory");
        std::fs::write(&file, b"preserved").unwrap();
        let store = temp.path().join("app_paths.json");
        std::fs::write(
            &store,
            serde_json::to_vec(&serde_json::json!({STORE_KEY_APP_CONFIG_DIR: file})).unwrap(),
        )
        .unwrap();
        assert_eq!(legacy_override_at(&store, true), Some(file));
        assert_eq!(legacy_override_at(&store, false), None);
    }

    #[test]
    fn the_store_path_tracks_the_bundle_identifier() {
        const TAURI_CONF: &str = include_str!("../tauri.conf.json");
        let identifier = serde_json::from_str::<Value>(TAURI_CONF)
            .expect("tauri.conf.json 必须是合法 JSON")
            .get("identifier")
            .and_then(Value::as_str)
            .map(str::to_string)
            .expect("tauri.conf.json 里必须有 identifier —— 没有它就推不出 store 位置");
        assert!(!identifier.is_empty(), "identifier 是空的");

        // ⚠️ **`None` 是合法返回值，不能 `expect`。** 这个函数依赖 OS 的用户目录探测
        // （Windows 走 `dirs::config_dir()` → `FOLDERID_RoamingAppData`），探不到时它
        // 有意返回 `None` 让上层回落到默认目录 —— 那是**环境事实，不是不变量**。
        //
        // 2026-08-05 在 CI 的 windows-latest 上实测到了：2596 个测试全过、只这一条
        // `panicked at ...: 推不出 store 路径`。原来的 `expect` 把「这台机器能否探到
        // 用户目录」当成了断言对象，而这条测试真正要守的是**路径的形状**
        // （以 `app_paths.json` 结尾、含 identifier）—— 那才是「改了 identifier 却忘了
        // 同步」会破坏的东西。
        //
        // ⇒ 探不到就跳过形状检查。这不是放宽标准：拿不到 base 的机器上根本没有
        // 「形状」可言，而 identifier 那两条前置断言（存在、非空）仍然无条件生效。
        let Some(path) = tauri_store_path() else {
            eprintln!("○ 这台机器探不到用户配置目录，跳过路径形状检查（identifier 已验）");
            return;
        };
        assert!(
            path.ends_with("app_paths.json"),
            "store 文件名与 `store_builder(\"app_paths.json\")` 那处不一致：{}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains(&identifier),
            "store 路径里没有 identifier（{identifier}）：{}",
            path.display()
        );
    }
}

#[cfg(test)]
mod scope_integration_tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    // Each case has a fresh process selection cache and temp-only store/home.
    // Never reset production globals in a running process to simulate restart.
    #[test]
    fn explicit_selection_controls_all_path_accessors() {
        const CHILD: &str = "LOONGPORT_SCOPE_TEST_CHILD";
        let Some(mode) = std::env::var_os(CHILD) else {
            for mode in ["legacy", "isolated", "broken"] {
                let output = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "app_store::scope_integration_tests::explicit_selection_controls_all_path_accessors", "--nocapture"])
                    .env(CHILD,mode).output().unwrap();
                assert!(
                    output.status.success(),
                    "{mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        };
        let temp = crate::secrets::testing::tempdir().unwrap();
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        let store = tauri_store_path().unwrap();
        assert!(
            store.starts_with(temp.path()),
            "tests must never resolve real home"
        );
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        let old = temp.path().join("old");
        let device = temp.path().join("device");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&device).unwrap();
        fs::write(
            &store,
            serde_json::to_vec(&serde_json::json!({STORE_KEY_APP_CONFIG_DIR:old})).unwrap(),
        )
        .unwrap();
        update_cached_override(Some(old.clone()));
        let selected = if mode != "legacy" {
            let pending = isolated_scope::prepare_scope_at(
                &store,
                &old,
                &device,
                &"a".repeat(64),
                &isolated_scope::capture_legacy_selection_at(&store, &old).unwrap(),
            )
            .unwrap();
            fs::write(pending.root().join("vault.json"), br#"{"metadata":{"vaultId":"4f281075-5976-4bba-aa7e-a3b24abc5d49","keyId":"445863c5-7aa4-4f88-817a-7532fc72f6f6"},"automaticUnlock":true}"#).unwrap();
            Some(
                pending
                    .publish(
                        "4f281075-5976-4bba-aa7e-a3b24abc5d49",
                        "445863c5-7aa4-4f88-817a-7532fc72f6f6",
                    )
                    .unwrap(),
            )
        } else {
            None
        };
        if mode == "broken" {
            fs::remove_dir_all(&selected.unwrap().root).unwrap();
            assert!(initialize_active_scope().is_err());
            assert!(
                std::panic::catch_unwind(get_app_config_dir_override).is_err(),
                "infallible API must not expose old root"
            );
            assert!(
                std::panic::catch_unwind(read_app_config_dir_override_without_tauri).is_err(),
                "headless path must not expose old root"
            );
            return;
        }
        initialize_active_scope().unwrap();
        let expected = selected.map(|s| s.root);
        assert_eq!(
            get_app_config_dir_override(),
            expected.clone().or(Some(old.clone()))
        );
        assert_eq!(
            read_app_config_dir_override_without_tauri(),
            expected.clone().or(Some(old))
        );
        assert_eq!(
            device_config_root(device.clone()),
            expected.unwrap_or(device)
        );
    }
}
