//! 切回官方登录的恢复链路（含 codex auth 备份）。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;

/// 「切回官方登录」的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOfficialLoginResult {
    /// 备份文件的完整路径。`None` 表示本来就没有 `auth.json`（没登录过 ChatGPT）。
    ///
    /// 要送给前端显示：那里面是 OAuth refresh token，用户手滑点了确认时得知道去哪儿捞回来。
    pub backup_path: Option<String>,
    /// 我们把 ChatGPT 关掉了吗（关了才会去重开它）。
    pub chatgpt_was_running: bool,
    pub warnings: Vec<String>,
}

/// 一键「切回官方登录」：清 codex 的第三方路由与登录态，让用户自己重新登录 ChatGPT。
///
/// ## 为什么需要它
///
/// LoongPort 把 codex 配成 **provider auth 模式**（`experimental_bearer_token` 写在
/// `config.toml` 里），鉴权压根不看 `auth.json` ⇒ 用户在 ChatGPT / codex 里点「注销」
/// **没有任何反应**，请求照样带着中转站的 sk 打出去。这不是 bug，但对用户是困惑 ——
/// 他以为自己退出了，实际没有。这条命令是那个「退出」真正的开关。
///
/// ## 四步的顺序不能反
///
/// ```text
/// 1. 退 ChatGPT      它持有 ~/.codex，且**退出时会回写 auth.json**
/// 2. 备份 auth.json  里面是 OAuth refresh token，删了要重走浏览器登录
/// 3. 切 codex-official  它的空 config 会自然清掉 experimental_bearer_token
/// 4. 删 auth.json    让 codex 回到「未登录」，用户自己登
/// ```
///
/// **1 在 2 之前**：不先退它，我们删完它退出时又把 `auth.json` 写回来 —— 用户看到的是
/// 「点了切回官方，但 codex 还是登录着的」。
///
/// **3 在 4 之前**：反过来的话中间有个窗口既没 bearer token 也没登录态。只做一半的后果
/// 各不相同且都很糟：只删 `auth.json` ⇒ 仍走中转站（token 还在 `config.toml` 里）；
/// 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ codex 报 credentials incomplete。
///
/// **2 不可省**：`ProviderService::switch` 自己那套清理（`clear_stale_codex_live_auth_after_official_switch`）
/// **有意不删带 OAuth 的 auth.json**（见 `codex_config::codex_auth_has_credential_login_material`）——
/// 用户的 ChatGPT 登录正是它拒绝碰的那一类，所以第 4 步必须自己动手，而动手之前必须留后路。
#[tauri::command]
pub async fn relay_restore_official_login(
    app_handle: tauri::AppHandle,
) -> Result<RestoreOfficialLoginResult, String> {
    restore_official_login_impl(&app_handle)
        .await
        .map_err(|e| e.to_string())
}

async fn restore_official_login_impl(
    app_handle: &tauri::AppHandle,
) -> Result<RestoreOfficialLoginResult, AppError> {
    let auth_path = crate::codex_config::get_codex_auth_path();

    // 编排走 `chatgpt_app::around`（与切档位共用同一份，见那边的说明）。
    //
    // ⚠️ **`abort_on_unconfirmed_exit = true`，与切档位相反 —— 这个差异是有意的。**
    // 那条只写 `config.toml`（ChatGPT 不碰它）⇒ 退不掉也能照常切。而这条要删
    // `auth.json`，**macOS 上 ChatGPT 退出时会回写它** ⇒ 没确认它退出就动手等于白删：
    // 用户看到「已切回官方登录」，实际它一退出就把登录态写回来。
    //
    // 这个标志**只对 macOS 生效**（`around` 里那个 `cfg!`）：Windows 实测不回写
    // `~/.codex`（`auth.json` 整个启停周期 mtime 未变），那边理由不成立 ⇒ 不中止。
    // 见 `chatgpt_app::around` 的文档。
    let (outcome, chatgpt) = chatgpt_app::around(true, || {
        // ── 备份 auth.json ──
        //
        // 在切换之前做，且失败就**中止**（`?`）—— 这一步的全部意义是「删之前留后路」，
        // 留不下后路就不该往下走，那是拿用户的 OAuth 登录去赌。
        // 没有这个文件是正常状态（从没登录过 ChatGPT），不是错误。
        let backup_path = backup_codex_auth(&auth_path)?;

        // ── 切到 codex-official ──
        //
        // 走 cc-switch 既有链路，不另写落盘逻辑。失败时 `around` 会把 ChatGPT 开回去，
        // 而配置没动、`auth.json` 还在原处（备份是拷贝不是移动）⇒ 用户手上的状态与
        // 操作前完全一样。
        let switched = {
            let state = app_handle.state::<AppState>();
            ProviderService::switch(
                &state,
                AppType::Codex,
                crate::database::CODEX_OFFICIAL_PROVIDER_ID,
            )
            .map_err(|e| AppError::Config(format!("切回官方登录失败：{e}。配置未改动")))?
        };

        let mut warnings = switched.warnings;

        // ── 删 auth.json ──**必须在切 provider 之后** ──
        //
        // 反过来的话中间有个窗口既没 bearer token 也没登录态。只做一半的后果各不相同
        // 且都很糟：只删 `auth.json` ⇒ 仍走中转站（token 还在 `config.toml` 里）；
        // 只切 provider ⇒ 走 ChatGPT auth 模式但没登录态 ⇒ 报 credentials incomplete。
        //
        // 删失败**不回滚切换**：那时 codex 已经是官方 provider（没有 bearer token 了），
        // 回滚等于把用户送回中转站路由 —— 而他刚刚明确要求离开那里。如实报出来让他
        // 手动删，比擅自撤销他的决定好。
        if auth_path.exists() {
            if let Err(e) = crate::config::delete_file(&auth_path) {
                warnings.push(format!(
                    "已切到官方 provider，但删除登录态失败：{e}。请手动删除 {} 后重新登录。",
                    auth_path.display()
                ));
            }
        }

        Ok((backup_path, warnings))
    })?;

    let (backup_path, mut warnings) = outcome;
    warnings.extend(chatgpt.warnings);

    // 广播「当前供应商变了」—— 这条命令内部调了 `ProviderService::switch`（切到
    // `codex-official`），所以它跟 `relay_switch_tier` / `switch_provider` 一样
    // 是一条**切换路径**，必须发。
    //
    // 漏了它的症状（2026-08-04 review 抓出）：用户在设置页点「切回官方登录」成功后
    // 回到供应商页，中转站区里原来那个托管档位**仍高亮「当前使用中」**、
    // 中转站行的删除按钮仍是灰的、title 还写着「要先切走」—— 而他已经切走了。
    // 后端是对的，坏的只有「不重开窗口就看不到」这一段（静默的界面陈旧）。
    //
    // 补在后端而不是让 `RestoreOfficialLoginButton` 自己刷：那个按钮在设置页，
    // 与中转站区互不相识；发事件是上游已有的机制，一处发射喂到全部监听者
    // （`provider.rs::emit_provider_switched` 的文档写了完整论证）。
    crate::services::application_overview::record_successful_selection(
        &app_handle.state::<AppState>().db,
        &AppType::Codex,
        crate::database::CODEX_OFFICIAL_PROVIDER_ID,
    );
    emit_provider_switched(
        app_handle,
        &AppType::Codex,
        crate::database::CODEX_OFFICIAL_PROVIDER_ID,
    );

    Ok(RestoreOfficialLoginResult {
        backup_path,
        chatgpt_was_running: chatgpt.was_running,
        warnings,
    })
}

/// 把 codex 的 `auth.json` 备份到 `~/.cc-switch/backups/codex-auth-<时间戳>.json`。
///
/// 返回 `None` 表示**源文件不存在**（用户从没登录过 ChatGPT）—— 那是正常状态，
/// 不是错误：没什么可备份，调用方那边也没什么可删。
///
/// 抽成独立函数是为了可测：它是这条链路上唯一碰用户文件的一步，
/// 而 `restore_official_login_impl` 需要 `AppHandle` 才能跑（测不了）。
pub(crate) fn backup_codex_auth(auth_path: &std::path::Path) -> Result<Option<String>, AppError> {
    if !auth_path.exists() {
        return Ok(None);
    }
    // 沿用仓库既有的 backups 目录惯例（`~/.cc-switch/backups/<用途>`），
    // 与 hermes / openclaw / codex-history 那几处同一个根。
    let dir = crate::config::get_app_config_dir().join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| AppError::io(&dir, e))?;
    let dest = dir.join(format!(
        "codex-auth-{}.json",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));
    crate::config::copy_file(auth_path, &dest)?;
    Ok(Some(dest.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 备份是「删 auth.json」之前的唯一后路，所以它必须真的把内容拷出来。
    ///
    /// ⚠️ **测试绝不能碰真实的 `~/.codex/auth.json`** —— 那里面是用户的 OAuth
    /// refresh token，跑一次测试把开发者自己的 ChatGPT 登录搞掉是不可接受的副作用
    /// （`chatgpt_app.rs:349` 那条注释钉的是同一件事）。所以这里不调
    /// `get_codex_auth_path()`，而是自己造一个临时文件喂给 `backup_codex_auth`，
    /// 并用 `CC_SWITCH_TEST_HOME` 把备份目标也关进临时目录。
    #[test]
    #[serial_test::serial]
    fn backup_copies_auth_json_before_it_gets_deleted() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let auth_path = temp.path().join("auth.json");
        let payload = r#"{"tokens":{"refresh_token":"secret"}}"#;
        std::fs::write(&auth_path, payload).expect("write fake auth.json");

        let backup = backup_codex_auth(&auth_path)
            .expect("备份不该失败")
            .expect("有源文件时必须返回备份路径");

        let backup_path = std::path::Path::new(&backup);
        assert_eq!(
            std::fs::read_to_string(backup_path).expect("read backup"),
            payload,
            "备份内容必须与原文件逐字节一致 —— 它是用户唯一的还原来源"
        );
        assert!(
            auth_path.exists(),
            "备份是**拷贝**不是移动：这一步失败时调用方要能原地中止，源文件必须还在"
        );
        assert!(
            backup_path.starts_with(temp.path()),
            "备份必须落在 CC_SWITCH_TEST_HOME 下，绝不能写到真实的 ~/.cc-switch"
        );
        let name = backup_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(
            name.starts_with("codex-auth-") && name.ends_with(".json"),
            "文件名要能让人一眼看出这是什么、什么时候备的，实际是 {name}"
        );

        match original_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }

    /// 没有 `auth.json` 是正常状态（从没登录过 ChatGPT），**不是错误**。
    ///
    /// 判成错误的后果：整条「切回官方登录」在这类用户身上直接失败，
    /// 而他们恰恰是最该能用它的人（想清掉 LoongPort 写的路由、自己去登录）。
    #[test]
    #[serial_test::serial]
    fn missing_auth_json_is_not_an_error() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let original_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        let absent = temp.path().join("auth.json");
        assert!(!absent.exists(), "前提：这个文件本来就不存在");

        assert!(
            backup_codex_auth(&absent)
                .expect("不存在不该报错")
                .is_none(),
            "没有源文件时返回 None（表示「没什么可备份」），而不是 Err"
        );
        assert!(
            !temp.path().join(".loongport").join("backups").exists(),
            "没东西要备份时不该顺手建出一个空的 backups 目录"
        );

        match original_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}
