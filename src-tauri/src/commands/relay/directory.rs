//! 中转站广场/榜单（transit 与 VeriDrop）命令与目录事件。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;

pub(crate) const RELAY_DIRECTORY_UPDATED_EVENT: &str = "relay-directory-updated";

// `DEFAULT_MODEL` 住在 `provision` 里 —— `pick_model` 要在「问不出模型列表」时
// 回落到它。这里只 `use`，避免在命令层另写一份。

/// 匿名统计的上报端点配好了没。
///
/// ## 为什么前端需要这个事实
///
/// 首启告知弹窗（`StatsNoticeDialog`）在问用户「同不同意上传」。而端点还是占位
/// （`stats::ENDPOINT` 含 `.invalid`）时，**同意与不同意的实际后果完全相同** ——
/// 一个字节都不会发出去（`lib.rs` 那个上报任务第一道闸就是 `is_configured`）。
///
/// 那时弹这一屏是**向用户征求一个没有意义的同意**：它消耗用户对弹窗的信任，
/// 却换不到任何数据。所以前端拿这个值当弹窗的前置条件。
///
/// ⚠️ **有意不把它并进 [`RelayStatus`]**：那条命令是**首屏渲染要等的东西**
/// （它的文档为此删掉过一个有遍历开销的字段），而这个事实只有统计告知那一屏要用。
/// 单独一条命令让它不参与首屏的关键路径。
///
/// ⇒ **端点配好那天这里自动放行**，不需要有人记得回来撤掉什么开关 ——
/// 判据就是端点本身，不是一个另行维护的标记。
#[tauri::command]
pub fn relay_stats_endpoint_configured() -> bool {
    crate::relay::stats::is_configured()
}

/// 推荐中转站（首启屏那几个按钮）。
///
/// ## 为什么读缓存而不是现拉
///
/// 与 [`relay_login`] 里取 aff 码同一个理由：拉取由启动时那个后台任务做
/// （`lib.rs`，延迟 5 秒），这里只同步读一份磁盘文件（含重新验签）——
/// **不让用户对着一个转圈的弹窗等一次网络往返**。
///
/// ⇒ **首启第一次打开时这里通常是空的**（那 5 秒还没到，或者根本没网）。
/// 那不是错误：UI 拿到空数组就只显示手动输入框，与这个功能上线前的样子一致。
/// 下次启动就有了（缓存已落盘）。
///
/// 返回空数组的三种情形都正常：没网 / 还没拉到 / 维护者临时撤空了列表。
#[tauri::command]
pub fn relay_list_sponsors() -> Vec<crate::relay::remote_config::Sponsor> {
    // 不返 `Result` —— 拿不到推荐不是错误，是「今天没有推荐」。
    // 返 Err 会让前端不得不写一个 catch 去把错误咽掉，那是把非错误伪装成错误。
    crate::relay::remote_config::load_cached()
        .map(|cfg| cfg.sponsors)
        .unwrap_or_default()
}

#[tauri::command]
pub async fn relay_list_directory(
    app_handle: tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, String> {
    if let Some(cached) =
        crate::relay::leaderboard::read_cached(kind).map_err(|error| error.to_string())?
    {
        if !crate::relay::leaderboard::is_cache_fresh(kind, chrono::Utc::now().timestamp()) {
            tauri::async_runtime::spawn(async move {
                if let Err(error) = refresh_stale_directory_and_emit(&app_handle, kind).await {
                    log::warn!("background VeriDrop refresh for {kind:?} failed: {error}");
                }
            });
        }
        return Ok(cached);
    }

    refresh_stale_directory_and_emit(&app_handle, kind)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn relay_refresh_directory(
    app_handle: tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, String> {
    force_refresh_directory_and_emit(&app_handle, kind)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayDirectoryUpdated {
    kind: crate::relay::leaderboard::LeaderboardKind,
}

async fn force_refresh_directory_and_emit(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    // 手动刷新按钮：榜单同步刷（返回值立刻要给 UI），transit 摘要异步刷
    // ——不能让几十个站的快照抓取把按钮卡住十几秒，刷完走事件广播。
    spawn_transit_refresh_and_emit(app_handle.clone());
    let outcome = crate::relay::leaderboard::refresh(kind).await?;
    emit_directory_update(app_handle, kind, outcome)
}

/// 异步刷一轮 transit 摘要，完成后广播全部榜单的更新事件。
///
/// maintenance 周期任务与手动刷新共用这一条：榜单与 transit 是两份数据、
/// 各刷各的；前端对 `relay-directory-updated` 的反应是重拉列表，届时
/// 读取路径会把新摘要合并进去（见 `leaderboard::decorate_transit`）。
pub(crate) fn spawn_transit_refresh_and_emit(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let config = crate::relay::remote_config::load_cached().unwrap_or_default();
        let hosts = crate::relay::leaderboard::managed_site_hosts(&config);
        if hosts.is_empty() {
            return;
        }
        crate::relay::transit::refresh_for_hosts(&hosts).await;
        emit_all_directory_updates(&app_handle);
    });
}

/// 广场数据在「命令层之外」被更新（transit 后台刷新）后的广播：
/// 4 个榜单各发一次同名事件，前端作废重拉。与 [`emit_directory_update`]
/// 共用事件契约，前端不区分来源。
pub(crate) fn emit_all_directory_updates(app_handle: &tauri::AppHandle) {
    for kind in crate::relay::leaderboard::LeaderboardKind::ALL {
        if let Err(error) = app_handle.emit(
            RELAY_DIRECTORY_UPDATED_EVENT,
            RelayDirectoryUpdated { kind },
        ) {
            log::warn!("发送广场更新事件失败（{kind:?}）: {error}");
        }
    }
}

async fn refresh_stale_directory_and_emit(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    let outcome = crate::relay::leaderboard::refresh_if_stale(kind).await?;
    emit_directory_update(app_handle, kind, outcome)
}

fn emit_directory_update(
    app_handle: &tauri::AppHandle,
    kind: crate::relay::leaderboard::LeaderboardKind,
    outcome: crate::relay::leaderboard::RefreshOutcome,
) -> Result<crate::relay::leaderboard::RelayLeaderboard, AppError> {
    if outcome.updated {
        app_handle
            .emit(
                RELAY_DIRECTORY_UPDATED_EVENT,
                RelayDirectoryUpdated { kind },
            )
            .map_err(|error| AppError::Message(format!("发送 VeriDrop 更新事件失败: {error}")))?;
    }
    Ok(outcome.leaderboard)
}

pub(crate) async fn refresh_stale_directories(
    app_handle: tauri::AppHandle,
) -> Result<(), AppError> {
    use futures::StreamExt;

    let now = chrono::Utc::now().timestamp();
    let stale = crate::relay::leaderboard::LeaderboardKind::ALL
        .into_iter()
        .filter(|kind| !crate::relay::leaderboard::is_cache_fresh(*kind, now));
    let mut refreshes = futures::stream::iter(stale.map(|kind| {
        let app_handle = app_handle.clone();
        async move { refresh_stale_directory_and_emit(&app_handle, kind).await }
    }))
    .buffer_unordered(2);

    let mut failures = Vec::new();
    while let Some(result) = refreshes.next().await {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "部分 VeriDrop 榜单刷新失败: {}",
            failures.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_update_event_matches_the_frontend_constant() {
        let frontend = include_str!("../../../../src/config/constants.ts");

        assert!(frontend.contains(RELAY_DIRECTORY_UPDATED_EVENT));
    }

    /// ⭐ `relay_list_sponsors` 发给前端的**键名**必须是 camelCase。
    ///
    /// 这条守的是一个跨语言的静默失效：`Sponsor` 的 `Deserialize` 用 snake_case
    /// （签名覆盖的配置契约，动不了），`Serialize` 用 camelCase（TS 侧惯例）。
    /// 两者不一致看起来像疏漏，**很可能被人顺手统一** —— 而统一到 snake_case 时
    /// 编译器一声不响，前端拿到的每个字段都是 `undefined` ⇒
    /// **首启屏卡片全是空白按钮**（`displayName` 为 undefined、React 什么都不渲染）。
    ///
    /// 断言的是序列化后的键，不是结构体字段名 —— 后者与前端无关。
    /// （`remote_config` 那边也有一条同向的闸，两处各守一端：
    /// 那条管「结构体的两个方向」，这条管「命令实际吐出去的东西」。）
    #[test]
    fn list_sponsors_emits_camel_case_keys_for_the_frontend() {
        let sponsor = crate::relay::remote_config::Sponsor {
            site_origin: "https://x.com".into(),
            display_name: "X".into(),
            tagline: "T".into(),
        };
        // 命令的返回类型是 `Vec<Sponsor>`，所以按它实际的序列化形态断言。
        let json = serde_json::to_value(vec![sponsor]).expect("要能序列化");
        let first = json[0].as_object().expect("是个对象");

        for key in ["siteOrigin", "displayName", "tagline"] {
            assert!(
                first.contains_key(key),
                "前端要的键 {key} 不在返回里，实际：{:?}",
                first.keys().collect::<Vec<_>>()
            );
        }
        assert!(
            !first.contains_key("site_origin") && !first.contains_key("display_name"),
            "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
        );
    }

    /// `directoryState.ts` 的 `defaultDirectoryKind` 是「app → 榜单分组」的 TS 镜像，
    /// 分组键是后端 `LeaderboardKind` 的 serde 线上名。跨语言编译器管不到 `.ts`：
    /// 后端改分键（或前端改默认分组）时两边各自漂移，症状是 tab 切过去查了一个
    /// 不存在的榜单（空列表，不报错）。这道闸把映射钉住，形状照 events.rs 的跨语言闸。
    #[test]
    fn frontend_default_directory_kind_matches_the_backend_leaderboard_kinds() {
        let ts = include_str!("../../../../src/components/relay/directory/directoryState.ts");
        for line in [
            r#"if (appId === "claude" || appId === "claude-desktop") return "claude";"#,
            r#"if (appId === "codex" || appId === "codex-image") return "openai";"#,
            r#"if (appId === "gemini") return "gemini";"#,
            r#"return "overall";"#,
        ] {
            assert!(
                ts.contains(line),
                "directoryState.ts 的默认分组映射变了，与后端 LeaderboardKind 的线上名 \
                 （overall/claude/openai/gemini）对不上一行：{line}"
            );
        }
    }
}
