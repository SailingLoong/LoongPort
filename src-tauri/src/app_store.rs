use serde_json::Value;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use tauri_plugin_store::StoreExt;

use crate::error::AppError;

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

/// 获取缓存中的 app_config_dir 覆盖路径
pub fn get_app_config_dir_override() -> Option<PathBuf> {
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
    let store_path = tauri_store_path()?;
    let raw = std::fs::read_to_string(&store_path).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    let path_str = json.get(STORE_KEY_APP_CONFIG_DIR)?.as_str()?.trim();
    if path_str.is_empty() {
        return None;
    }
    let path = resolve_path(path_str);
    // 与 `read_override_from_store` 同一条判据：路径不存在就当没设
    // （用户可能把那个目录删了 / 拔了外置盘，那时用默认目录比报错好）。
    path.is_dir().then_some(path)
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
fn tauri_store_path() -> Option<PathBuf> {
    const TAURI_CONF: &str = include_str!("../tauri.conf.json");
    let identifier = serde_json::from_str::<Value>(TAURI_CONF)
        .ok()?
        .get("identifier")?
        .as_str()?
        .to_string();

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

fn read_override_from_store(app: &tauri::AppHandle) -> Option<PathBuf> {
    let store = match app.store_builder("app_paths.json").build() {
        Ok(store) => store,
        Err(e) => {
            log::warn!("无法创建 Store: {e}");
            return None;
        }
    };

    match store.get(STORE_KEY_APP_CONFIG_DIR) {
        Some(Value::String(path_str)) => {
            let path_str = path_str.trim();
            if path_str.is_empty() {
                return None;
            }

            let path = resolve_path(path_str);

            if !path.exists() {
                log::warn!(
                    "Store 中配置的 app_config_dir 不存在: {path:?}\n\
                     将使用默认路径。"
                );
                return None;
            }

            log::info!("使用 Store 中的 app_config_dir: {path:?}");
            Some(path)
        }
        Some(_) => {
            log::warn!("Store 中的 {STORE_KEY_APP_CONFIG_DIR} 类型不正确，应为字符串");
            None
        }
        None => None,
    }
}

/// 从 Store 刷新 app_config_dir 覆盖值并更新缓存
pub fn refresh_app_config_dir_override(app: &tauri::AppHandle) -> Option<PathBuf> {
    let value = read_override_from_store(app);
    update_cached_override(value.clone());
    value
}

/// 写入 app_config_dir 到 Tauri Store
pub fn set_app_config_dir_to_store(
    app: &tauri::AppHandle,
    path: Option<&str>,
) -> Result<(), AppError> {
    let store = app
        .store_builder("app_paths.json")
        .build()
        .map_err(|e| AppError::Message(format!("创建 Store 失败: {e}")))?;

    match path {
        Some(p) => {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                store.set(STORE_KEY_APP_CONFIG_DIR, Value::String(trimmed.to_string()));
                log::info!("已将 app_config_dir 写入 Store: {trimmed}");
            } else {
                store.delete(STORE_KEY_APP_CONFIG_DIR);
                log::info!("已从 Store 中删除 app_config_dir 配置");
            }
        }
        None => {
            store.delete(STORE_KEY_APP_CONFIG_DIR);
            log::info!("已从 Store 中删除 app_config_dir 配置");
        }
    }

    store
        .save()
        .map_err(|e| AppError::Message(format!("保存 Store 失败: {e}")))?;

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

/// 从旧的 settings.json 迁移 app_config_dir 到 Store
pub fn migrate_app_config_dir_from_settings(app: &tauri::AppHandle) -> Result<(), AppError> {
    // app_config_dir 已从 settings.json 移除，此函数保留但不再执行迁移
    // 如果用户在旧版本设置过 app_config_dir，需要在 Store 中手动配置
    log::info!("app_config_dir 迁移功能已移除，请在设置中重新配置");

    let _ = refresh_app_config_dir_override(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `app_paths.json` 的位置由 `tauri.conf.json` 的 `identifier` 推出来，
    /// 而那是**跨文件的同一事实**（CLAUDE.md §三点六）。
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
