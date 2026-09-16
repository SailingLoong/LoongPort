//! 档位顺序配置档（2026-09-16）：命名的顺序快照，多份共存、可覆盖、可导入导出。
//!
//! 动机：实际路由的顺序只有一份活配置，覆盖即失；用户要多套方案（如「便宜的」
//! 「快的」）随时切换。配置档只是**顺序快照**——屏蔽名单与优先级开关是运行态，
//! 不进档。载入在前端表现为进草稿，仍走「应用/取消」。
//!
//! 自愈：档位 id 是本机 DB 的 provider_id，跨机导入必然带认不出的 id——
//! 读取/导入一律过滤到当前 app 的已知档位（坏值失败模式 = 该档不生效，
//! 不是整份拒绝）。

use crate::store::AppState;
use std::path::PathBuf;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OrderProfile {
    /// 用户自定义名称；同名单档覆盖。
    pub name: String,
    /// 档位顺序（provider_id 序列）。存储侧已过滤到已知档位。
    #[serde(rename = "providerIds")]
    pub provider_ids: Vec<String>,
}

fn profiles_key(app: &str) -> String {
    format!("application_order_profiles_{app}")
}

fn read_profiles(db: &crate::Database, app: &str) -> Vec<OrderProfile> {
    db.get_setting(&profiles_key(app))
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<OrderProfile>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|profile| !profile.name.trim().is_empty())
        .collect()
}

fn write_profiles(
    db: &crate::Database,
    app: &str,
    profiles: &[OrderProfile],
) -> Result<(), String> {
    db.set_setting(
        &profiles_key(app),
        &serde_json::to_string(profiles).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// 只保留当前 app 的已知档位并去重——顺序快照对不上现实（档位被删/跨机导入）
/// 时按「认得出的留下」自愈，不拒绝整份。
fn sanitize_ids(db: &crate::Database, app: &str, ids: &[String]) -> Vec<String> {
    let known: std::collections::HashSet<String> =
        crate::proxy::application_routing::ordered_providers(db, app)
            .map(|providers| providers.into_iter().map(|p| p.id).collect())
            .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .filter(|id| known.contains(*id) && seen.insert((*id).clone()))
        .cloned()
        .collect()
}

#[tauri::command]
pub fn get_order_profiles(
    state: State<'_, AppState>,
    app_type: String,
) -> Result<Vec<OrderProfile>, String> {
    Ok(read_profiles(&state.db, &app_type))
}

/// 保存（同名覆盖）当前顺序为命名配置档。
#[tauri::command]
pub fn save_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
    provider_ids: Vec<String>,
) -> Result<(), String> {
    upsert_profile(&state.db, &app_type, &name, &provider_ids)
}

#[tauri::command]
pub fn delete_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
) -> Result<(), String> {
    remove_profile(&state.db, &app_type, &name)
}

/// upsert 核心：命令与导入共用；测试直测这里（免 State 解包）。
fn upsert_profile(
    db: &crate::Database,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("profile name is empty".into());
    }
    let provider_ids = sanitize_ids(db, app, ids);
    let mut profiles = read_profiles(db, app);
    match profiles.iter().position(|p| p.name == name) {
        Some(index) => profiles[index] = OrderProfile { name, provider_ids },
        None => profiles.push(OrderProfile { name, provider_ids }),
    }
    write_profiles(db, app, &profiles)
}

fn remove_profile(db: &crate::Database, app: &str, name: &str) -> Result<(), String> {
    let mut profiles = read_profiles(db, app);
    profiles.retain(|p| p.name != name);
    write_profiles(db, app, &profiles)
}

/// 导出全部配置档为 JSON 文件（自带保存对话框）。返回写入路径；用户取消返回 None。
#[tauri::command]
pub async fn export_order_profiles<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    app_type: String,
) -> Result<Option<String>, String> {
    let profiles = read_profiles(&state.db, &app_type);
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(format!("{app_type}-order-profiles.json"))
        .blocking_save_file()
        .map(|p| p.to_string())
    else {
        return Ok(None);
    };
    let json = serde_json::to_string_pretty(&profiles).map_err(|e| e.to_string())?;
    std::fs::write(PathBuf::from(&path), json).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

/// 从 JSON 文件导入配置档（自带打开对话框，同名单档覆盖）。
/// 坏条目（缺名/非对象）跳过不阻断整份；返回导入条数；取消返回 None。
#[tauri::command]
pub async fn import_order_profiles<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    app_type: String,
) -> Result<Option<usize>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file()
        .map(|p| p.to_string())
    else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let parsed: Vec<OrderProfile> =
        serde_json::from_str(&raw).map_err(|e| format!("invalid profile file: {e}"))?;
    let mut imported = 0usize;
    for profile in parsed {
        if profile.name.trim().is_empty() {
            continue;
        }
        upsert_profile(&state.db, &app_type, &profile.name, &profile.provider_ids)?;
        imported += 1;
    }
    Ok(Some(imported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn profiles_round_trip_overwrite_and_delete() {
        let db = crate::Database::memory().unwrap();
        let a =
            crate::provider::Provider::with_id("a".into(), "A".into(), serde_json::json!({}), None);
        let b =
            crate::provider::Provider::with_id("b".into(), "B".into(), serde_json::json!({}), None);
        db.save_provider("claude", &a).unwrap();
        db.save_provider("claude", &b).unwrap();

        // 保存时过滤未知 id（跨机导入/档位已删自愈）。
        upsert_profile(
            &db,
            "claude",
            "便宜优先",
            &["b".to_string(), "a".into(), "ghost".into()],
        )
        .unwrap();
        let profiles = read_profiles(&db, "claude");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].provider_ids, vec!["b", "a"]);

        // 同名覆盖。
        upsert_profile(&db, "claude", "便宜优先", &["a".to_string(), "b".into()]).unwrap();
        assert_eq!(read_profiles(&db, "claude")[0].provider_ids, vec!["a", "b"]);

        // 空名拒绝。
        assert!(upsert_profile(&db, "claude", "  ", &["a".to_string()]).is_err());

        // 删除后读回空。
        remove_profile(&db, "claude", "便宜优先").unwrap();
        assert!(read_profiles(&db, "claude").is_empty());
    }
}
