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
    acknowledge_known_announcement(id)
}

// Only called after the signed-config admission above. The read-only projection
// is sufficient for the no-write fast path, never for a full settings save.
fn acknowledge_known_announcement(id: String) -> Result<(), String> {
    if crate::settings::get_settings()
        .acknowledged_announcements
        .contains(&id)
    {
        return Ok(());
    }
    crate::settings::mutate_settings(|settings| {
        // Another confirmation may have committed after the fast-path read.
        if !settings.acknowledged_announcements.contains(&id) {
            settings.acknowledged_announcements.push(id);
        }
    })
    .map_err(|e| e.to_string())
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

    fn settings_fixture() -> (
        crate::secrets::testing::TestHome,
        std::sync::Arc<crate::secrets::session::SecretSession>,
    ) {
        use crate::secrets::{session::SecretSession, VaultContext};
        let home = crate::secrets::testing::TestHome::new().unwrap();
        let session = SecretSession::from_context(
            crate::config::get_app_config_dir(),
            VaultContext::generate().unwrap(),
        );
        std::fs::create_dir_all(crate::settings::settings_path().parent().unwrap()).unwrap();
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
        crate::settings::mutate_settings(|settings| {
            settings.webdav_sync = Some(crate::settings::WebDavSyncSettings {
                base_url: "https://synthetic.invalid".into(),
                password: "synthetic-webdav".into(),
                ..Default::default()
            });
            settings.s3_sync = Some(crate::settings::S3SyncSettings {
                bucket: "synthetic-bucket".into(),
                secret_access_key: "synthetic-s3".into(),
                ..Default::default()
            });
            settings.webdav_backup = Some(serde_json::json!({"password":"synthetic-backup"}));
            settings.upgrade_completion = Some(crate::settings::UpgradeCompletion {
                checkpoint_id: "synthetic-completion".into(),
                sync_paused: false,
            });
            settings.current_provider_codex = Some("synthetic-current".into());
        })
        .unwrap();
        (home, session)
    }

    fn persisted_settings(session: &crate::secrets::session::SecretSession) -> serde_json::Value {
        serde_json::to_value(
            crate::settings::decode_settings_with_storage(
                &std::fs::read(crate::settings::settings_path()).unwrap(),
                &session.read().unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    #[serial_test::serial]
    fn acknowledgement_preserves_protected_bundles_and_foreign_fields() {
        let (_home, session) = settings_fixture();
        let path = crate::settings::settings_path();
        let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let foreign = r#"{"number":900719925474099312345,"keep":true}"#;
        fields.insert(
            "futureAnnouncementPeer".into(),
            serde_json::value::RawValue::from_string(foreign.into()).unwrap(),
        );
        std::fs::write(&path, serde_json::to_vec(&fields).unwrap()).unwrap();
        let mut expected = persisted_settings(&session);
        expected["acknowledgedAnnouncements"] = serde_json::json!(["synthetic-known"]);
        acknowledge_known_announcement("synthetic-known".into()).unwrap();
        assert_eq!(persisted_settings(&session), expected);
        let fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(fields["futureAnnouncementPeer"].get(), foreign);
    }

    #[test]
    #[serial_test::serial]
    fn duplicate_and_unknown_acknowledgements_do_not_rewrite_settings() {
        let (_home, _session) = settings_fixture();
        acknowledge_known_announcement("synthetic-known".into()).unwrap();
        let path = crate::settings::settings_path();
        let before = std::fs::read(&path).unwrap();
        acknowledge_known_announcement("synthetic-known".into()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // No signed announcement with this synthetic ID: original public admission rejects it.
        acknowledge_announcement("synthetic-unknown-owner-regression-20261027".into()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    #[serial_test::serial]
    fn failed_acknowledgement_does_not_publish_an_id() {
        let (_home, _session) = settings_fixture();
        let path = crate::settings::settings_path();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(acknowledge_known_announcement("synthetic-known".into()).is_err());
        assert!(crate::settings::get_settings()
            .acknowledged_announcements
            .is_empty());
        assert!(path.is_dir());
    }

    #[test]
    #[serial_test::serial]
    fn concurrent_acknowledgements_preserve_each_admitted_id() {
        let (_home, session) = settings_fixture();
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for index in 0..8 {
                let start = start.clone();
                scope.spawn(move || {
                    start.wait();
                    acknowledge_known_announcement(format!("synthetic-{index}")).unwrap();
                });
            }
        });
        let persisted = persisted_settings(&session);
        let ids = persisted["acknowledgedAnnouncements"].as_array().unwrap();
        assert_eq!(ids.len(), 8);
        for index in 0..8 {
            assert!(ids.contains(&serde_json::json!(format!("synthetic-{index}"))));
        }
        assert!(persisted.get("webdavSync").is_some());
        assert!(persisted.get("s3Sync").is_some());
        assert!(persisted.get("webdavBackup").is_some());
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let start = start.clone();
                scope.spawn(move || {
                    start.wait();
                    acknowledge_known_announcement("synthetic-same-id".into()).unwrap();
                });
            }
        });
        let persisted = persisted_settings(&session);
        let ids = persisted["acknowledgedAnnouncements"].as_array().unwrap();
        assert_eq!(ids.len(), 9);
        assert_eq!(
            ids.iter()
                .filter(|id| **id == serde_json::json!("synthetic-same-id"))
                .count(),
            1
        );
    }
}
