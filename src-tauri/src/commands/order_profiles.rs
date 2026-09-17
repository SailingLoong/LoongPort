//! 档位顺序配置档：命名的顺序快照，多份共存、可覆盖、可导入导出，
//! 其中一份是「当前配置文件」——应用此顺序默认保存进它（2026-09-17 定调）。
//!
//! 动机：实际路由的顺序只有一份活配置，覆盖即失；用户要多套方案（如「便宜的」
//! 「快的」）随时切换。配置档只是**顺序快照**——屏蔽名单与优先级开关是运行态，
//! 不进档。载入在前端表现为进草稿 + 切换当前指针，仍走「应用/取消」。
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

/// 配置档列表 + 当前配置文件名（前端展示与「应用即保存」的落点）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderProfilesState {
    pub profiles: Vec<OrderProfile>,
    pub current: String,
}

/// 默认配置文件名：首次使用自动创建、内容取当时的切换链；随时可重命名。
const DEFAULT_PROFILE_NAME: &str = "default";

fn profiles_key(app: &str) -> String {
    format!("application_order_profiles_{app}")
}

fn current_key(app: &str) -> String {
    format!("application_order_profile_current_{app}")
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

fn read_current(db: &crate::Database, app: &str) -> Option<String> {
    db.get_setting(&current_key(app))
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<String>(&raw).ok())
        .filter(|name| !name.trim().is_empty())
}

fn write_current(db: &crate::Database, app: &str, name: &str) -> Result<(), String> {
    db.set_setting(
        &current_key(app),
        &serde_json::to_string(name).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// 保证列表非空（空则用当前切换链播种 default）且当前指针指向真实存在的档。
/// 幂等；读取与每次变更后都过一遍，指针悬空（手删 settings/导入覆盖）自愈回 default。
fn ensure_default_profile(db: &crate::Database, app: &str) -> Result<(), String> {
    let mut profiles = read_profiles(db, app);
    if profiles.is_empty() {
        let chain =
            crate::proxy::application_routing::chain_ids(db, app).map_err(|e| e.to_string())?;
        profiles.push(OrderProfile {
            name: DEFAULT_PROFILE_NAME.to_string(),
            provider_ids: chain,
        });
        write_profiles(db, app, &profiles)?;
    }
    let current = read_current(db, app);
    if !current.is_some_and(|name| profiles.iter().any(|p| p.name == name)) {
        write_current(db, app, DEFAULT_PROFILE_NAME)?;
    }
    Ok(())
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
) -> Result<OrderProfilesState, String> {
    ensure_default_profile(&state.db, &app_type)?;
    let current = read_current(&state.db, &app_type).unwrap_or_else(|| DEFAULT_PROFILE_NAME.into());
    Ok(OrderProfilesState {
        profiles: read_profiles(&state.db, &app_type),
        current,
    })
}

/// 保存（同名覆盖）顺序为命名配置档，并把它设为当前配置文件——
/// 「另存为/新建」与「应用此顺序落进当前档」共用这一个语义。
#[tauri::command]
pub fn save_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
    provider_ids: Vec<String>,
) -> Result<(), String> {
    let name = name.trim().to_string();
    upsert_profile(&state.db, &app_type, &name, &provider_ids)?;
    write_current(&state.db, &app_type, &name)
}

/// 切换当前配置文件（点选某个配置档时调用；内容载入是前端草稿，不在这）。
#[tauri::command]
pub fn set_current_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim().to_string();
    ensure_default_profile(&state.db, &app_type)?;
    if !read_profiles(&state.db, &app_type)
        .iter()
        .any(|p| p.name == name)
    {
        return Err(format!("profile not found: {name}"));
    }
    write_current(&state.db, &app_type, &name)
}

/// 重命名配置档；改的是当前档时指针跟着走。目标名已存在则拒绝（防静默合并）。
#[tauri::command]
pub fn rename_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    from: String,
    to: String,
) -> Result<(), String> {
    rename_profile(&state.db, &app_type, &from, &to)
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

fn rename_profile(db: &crate::Database, app: &str, from: &str, to: &str) -> Result<(), String> {
    let from = from.trim().to_string();
    let to = to.trim().to_string();
    if to.is_empty() {
        return Err("profile name is empty".into());
    }
    let mut profiles = read_profiles(db, app);
    let Some(index) = profiles.iter().position(|p| p.name == from) else {
        return Err(format!("profile not found: {from}"));
    };
    if from != to && profiles.iter().any(|p| p.name == to) {
        return Err(format!("profile already exists: {to}"));
    }
    profiles[index].name = to.clone();
    write_profiles(db, app, &profiles)?;
    if read_current(db, app).as_deref() == Some(from.as_str()) {
        write_current(db, app, &to)?;
    }
    Ok(())
}

fn remove_profile(db: &crate::Database, app: &str, name: &str) -> Result<(), String> {
    let mut profiles = read_profiles(db, app);
    let removed_current = read_current(db, app).is_some_and(|current| current == name.trim());
    profiles.retain(|p| p.name != name.trim());
    write_profiles(db, app, &profiles)?;
    // 删的是当前档 → 指针回退到剩下第一份；全删空则清指针（下次读取重新播种 default）。
    if removed_current {
        match profiles.first() {
            Some(first) => write_current(db, app, &first.name.clone())?,
            None => db
                .set_setting(&current_key(app), "")
                .map_err(|e| e.to_string())?,
        }
    }
    Ok(())
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

    /// 首次读取播种 default 配置档（内容 = 当时切换链），当前指针自愈到有效档。
    #[test]
    #[serial_test::serial]
    fn ensure_default_seeds_from_chain_and_heals_pointer() {
        let db = crate::Database::memory().unwrap();
        let a =
            crate::provider::Provider::with_id("a".into(), "A".into(), serde_json::json!({}), None);
        let b =
            crate::provider::Provider::with_id("b".into(), "B".into(), serde_json::json!({}), None);
        db.save_provider("claude", &a).unwrap();
        db.save_provider("claude", &b).unwrap();

        ensure_default_profile(&db, "claude").unwrap();
        let profiles = read_profiles(&db, "claude");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "default");
        // 链未初始化 → chain_ids 回落全量显示序，default 播种即全量。
        assert_eq!(profiles[0].provider_ids, vec!["a", "b"]);
        assert_eq!(
            read_current(&db, "claude").as_deref(),
            Some("default"),
            "空指针自愈到 default"
        );

        // 指针悬空（手删 settings / 异常状态）→ 下次 ensure 拉回 default。
        write_current(&db, "claude", "不存在的档").unwrap();
        ensure_default_profile(&db, "claude").unwrap();
        assert_eq!(read_current(&db, "claude").as_deref(), Some("default"));

        // 已有档时不追加 default（幂等）。
        ensure_default_profile(&db, "claude").unwrap();
        assert_eq!(read_profiles(&db, "claude").len(), 1);
    }

    /// 重命名：目标撞名拒绝；改当前档时指针跟着走；删当前档时指针回退到剩下第一份。
    #[test]
    #[serial_test::serial]
    fn rename_moves_pointer_and_delete_falls_back() {
        let db = crate::Database::memory().unwrap();
        for id in ["a", "b"] {
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    id.into(),
                    id.into(),
                    serde_json::json!({}),
                    None,
                ),
            )
            .unwrap();
        }
        ensure_default_profile(&db, "claude").unwrap();
        upsert_profile(&db, "claude", "快", &["b".into()]).unwrap();
        write_current(&db, "claude", "default").unwrap();

        // 撞名拒绝。
        assert!(rename_profile(&db, "claude", "快", "default").is_err());
        // 改当前档 → 指针跟随。
        rename_profile(&db, "claude", "default", "日常").unwrap();
        assert_eq!(
            read_profiles(&db, "claude")
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            vec!["日常", "快"]
        );
        assert_eq!(read_current(&db, "claude").as_deref(), Some("日常"));

        // 改非当前档 → 指针不动。
        rename_profile(&db, "claude", "快", "更快").unwrap();
        assert_eq!(read_current(&db, "claude").as_deref(), Some("日常"));

        // 删当前档 → 指针回退到剩下第一份。
        remove_profile(&db, "claude", "日常").unwrap();
        assert_eq!(read_current(&db, "claude").as_deref(), Some("更快"));

        // 删空 → 指针清空，下次 ensure 重新播种 default。
        remove_profile(&db, "claude", "更快").unwrap();
        assert!(read_current(&db, "claude").is_none());
        ensure_default_profile(&db, "claude").unwrap();
        assert_eq!(read_current(&db, "claude").as_deref(), Some("default"));
    }
}
