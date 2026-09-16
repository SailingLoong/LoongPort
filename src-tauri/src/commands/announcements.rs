//! 远端公告：数据来自签名远端配置（改文案零发版），确认状态存设备本地 settings。

use crate::relay::remote_config::{self, Announcement};

/// 待展示的公告 = 远端配置里当前客户端认识的（type=dialog 且字段完整）减去已确认的。
/// 不返 `Result`：拿不到配置 = 今天没有公告，与 `relay_list_sponsors` 同一个语义。
#[tauri::command]
pub fn get_pending_announcements() -> Vec<Announcement> {
    let Some(cfg) = remote_config::load_cached() else {
        return Vec::new();
    };
    let acknowledged = crate::settings::get_settings().acknowledged_announcements;
    remote_config::effective_announcements(&cfg)
        .into_iter()
        .filter(|announcement| !acknowledged.iter().any(|id| id == &announcement.id))
        .cloned()
        .collect()
}

/// 确认公告（前端把任何关闭方式——点确认、Esc、点遮罩——都路由到这里：
/// 重弹比误关更烦人）。只登记远端当前存在的 id，防坏值把垃圾 id 灌进 settings。
#[tauri::command]
pub fn acknowledge_announcement(id: String) -> Result<(), String> {
    let known_ids: Vec<String> = remote_config::load_cached()
        .map(|cfg| {
            remote_config::effective_announcements(&cfg)
                .into_iter()
                .map(|announcement| announcement.id.clone())
                .collect()
        })
        .unwrap_or_default();
    if !known_ids.contains(&id) {
        return Ok(());
    }
    let mut settings = crate::settings::get_settings();
    if settings.acknowledged_announcements.contains(&id) {
        return Ok(());
    }
    settings.acknowledged_announcements.push(id);
    crate::settings::update_settings(settings).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_announcements_skip_unknown_kind_and_empty_fields() {
        let cfg = serde_json::from_str::<crate::relay::remote_config::RemoteConfig>(
            r#"{"announcements":[
                {"id":"a1","type":"dialog","title":"维护公告","body":"今晚维护"},
                {"id":"a2","type":"banner","title":"未来类型","body":"老客户端应跳过"},
                {"id":"","type":"dialog","title":"缺 id","body":"x"},
                {"id":"a4","type":"dialog","title":"","body":"缺标题"},
                {"id":"a5","type":"dialog","title":"缺正文","body":""}
            ]}"#,
        )
        .unwrap();
        let effective = remote_config::effective_announcements(&cfg);
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].id, "a1");
        // 缺 announcements 键的老配置照常解出（serde default）。
        let legacy =
            serde_json::from_str::<crate::relay::remote_config::RemoteConfig>(r#"{"sponsors":[]}"#)
                .unwrap();
        assert!(remote_config::effective_announcements(&legacy).is_empty());
    }
}
