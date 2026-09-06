//! 「点 Star 领注册礼」的机制层：弹窗邀请 payload 的组装、领取标志落库与
//! gh CLI 自动点星。
//!
//! 策略（什么时候弹、弹给谁）在 `commands::onboarding` 与前端
//! `GitHubStarButton` / `StarRewardDialog`（红点入口与弹窗状态机）；
//! 本模块只提供两端共用的机制：
//! - 邀请 payload：纯本地组装（读远端配置缓存，不打任何网络）；
//! - `star_reward_claimed` 的窄命令 RMW 落库；
//! - gh CLI 自动点星：**后台尽力而为**，不参与任何校验。
//!
//! 发放语义是**荣誉制**：点击「领取」= 打开浏览器仓库页 + 当场发码，不做
//! 任何 Star 校验（校验要打 GitHub API，国内网络下是 20s 级无反馈等待，
//! 用户感知就是「卡死」，331b532f 拆的就是它）。自动点星是同一决策下的
//! 顺手便利：gh 装了且登录了就替用户点上，**成败都不影响领取** —— 见
//! [`star_reward_auto_star`] 的三道保险。

use serde::Serialize;

use crate::config::GITHUB_REPO;

/// gh 点星的整条通路预算（含进程启动与网络）。超时即杀：网络再差也只烧
/// 这么多**后台**时间，主流程本来就零等待，这道闸管的是不留孤儿进程与
/// 日志噪音。
const GH_STAR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Star 对话框的 payload：`star_reward_offer` 命令返回（顶栏红点是唯一入口，
/// 曾经的主动弹窗事件已删）；前端 `src/lib/api/starReward.ts` 的 `StarRewardOffer` 与之对应。
///
/// 序列化 camelCase（本仓 TS 侧惯例），与 `commands::onboarding` 的
/// `RegisterCompletedPayload` 同一形状。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StarRewardOffer {
    pub promo_code: String,
    pub amount_usd: u64,
}

/// 远端配置里的 star_reward 当前可用吗。空码 = 维护者撤销 = 活动下线，
/// 与 `remote_config::resolve_code` 的「空值 = 撤销」同一语义。
pub(crate) fn effective_star_reward() -> Option<crate::relay::remote_config::StarRewardConfig> {
    crate::relay::remote_config::load_cached()
        .and_then(|config| config.star_reward)
        .filter(|reward| !reward.promo_code.trim().is_empty())
}

/// 邀请成立的唯一判定：远端配置里有可用的 `star_reward`。读不到就 `None` ——
/// 调用方（新人引导事件、红点点击）一律静默回落到现状行为，不给用户看一个
/// 随时兑现不了的 offer。
pub(crate) fn build_offer() -> Option<StarRewardOffer> {
    let reward = effective_star_reward()?;
    Some(StarRewardOffer {
        promo_code: reward.promo_code.trim().to_string(),
        amount_usd: reward.amount_usd,
    })
}

/// 红点入口的弹窗邀请。`None` = 活动不在（远端配置无 `star_reward` / 空码），
/// 前端回落「直接开仓库」。
#[tauri::command]
pub fn star_reward_offer() -> Result<Option<StarRewardOffer>, String> {
    Ok(build_offer())
}

/// Star 领取落点（2026-08-16 起）：后端 RMW 写 `star_reward_claimed`，幂等。
/// 不走前端全量 save —— 这条路在 `merge_settings_for_save` 对后端专有字段
/// 无条件取现有值（旧快照回写曾把它抹掉，红点复活、码可重领），这个字段的
/// 事实 owner 本来就在后端。
#[tauri::command]
pub fn star_reward_mark_claimed() -> Result<(), String> {
    crate::settings::mutate_settings(|settings| {
        settings.star_reward_claimed = Some(true);
    })
    .map_err(|e| e.to_string())
}

/// gh CLI 自动点星（荣誉制下的顺手便利，不是校验）。命令**立即返回**，
/// 点星在后台任务里尽力而为，成败只进日志 —— 领取主流程（发码/开浏览器/
/// 开注册窗）零等待。
///
/// ## 三道保险（2026-08-15 版的教训，旧版让用户「点完卡死」的根因复盘）
///
/// 旧版 [`github_star_via_gh`] 的问题不在命令本身（它已带超时），而在
/// **主流程 await 它**：国内网络 gh 打 GitHub API 是 20s 级等待，配合
/// 当时的星数校验，用户点完领取要干等半分钟。现在的形状把三道保险焊死：
///
/// - **后台任务**：本命令 spawn 后立刻 `Ok(())`，前端 fire-and-forget，
///   任何结局都碰不到领取路径；
/// - **stdin = null**：gh 缺登录态时会交互式提示（等终端输入 = 真·永久
///   挂起），关掉 stdin 让它立即以非零退出；
/// - **3s 超时 + kill_on_drop**：网络黑洞也只烧 3 秒后台预算，不留孤儿进程。
///
/// 通路边界沿用 2026-08-15 的决定：**只用 gh 自己的鉴权**，不去碰 git
/// 凭据（keychain 里的 PAT scope 不可控，用 push 令牌点星信任面太难看）。
/// PUT starred 幂等，重复调用无害；没装/没登录 gh 就静默跳过，浏览器里
/// 开着的仓库页仍是荣誉制的最终落点。
#[tauri::command]
pub async fn star_reward_auto_star() -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = star_via_gh().await {
            log::info!("gh 自动点星未成（不影响领取，荣誉制不校验）: {error}");
        }
    });
    Ok(())
}

/// `GITHUB_REPO`（`https://github.com/{owner}/{repo}`）→ REST API 与 gh CLI 用的
/// `{owner}/{repo}`。从同一个常量派生而不是另写一份 —— 仓库搬家时只改一处。
fn repo_api_slug() -> &'static str {
    GITHUB_REPO.trim_start_matches("https://github.com/")
}

/// 逐个安装位试 `gh api --method PUT /user/starred/{owner}/{repo}`。
/// spawn 失败（NotFound）= 该位置没装，试下一个；跑起来但失败（没登录 /
/// scope 不够）= 换路径没意义（同一个 gh），判这条路不通。
async fn star_via_gh() -> Result<(), String> {
    // macOS 从 Finder/Dock 启动的 GUI 进程拿到的 PATH 很短，brew 装的 gh
    // 不在里面 —— 先探测常见安装位，最后才试 PATH（Windows 的 winget
    // 安装一般进 PATH，`gh` 那一项是给它用的）。
    let candidates = [
        "/opt/homebrew/bin/gh",
        "/usr/local/bin/gh",
        r"C:\Program Files\GitHub CLI\gh.exe",
        "gh",
    ];
    for candidate in candidates {
        let mut command = tokio::process::Command::new(candidate);
        command
            .args([
                "api",
                "--method",
                "PUT",
                &format!("/user/starred/{}", repo_api_slug()),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 超时后（下面 await 那层 drop）顺手杀掉，不留孤儿 gh 进程。
            .kill_on_drop(true);

        let output = match tokio::time::timeout(GH_STAR_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                // 这个位置没装，试下一个候选。
                continue;
            }
            Ok(Err(error)) => return Err(format!("gh 启动失败（{candidate}）: {error}")),
            Err(_) => return Err(format!("gh 超时（{candidate}，>{GH_STAR_TIMEOUT:?}）")),
        };

        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "gh 退出码非零（{candidate}，没登录/scope 不够都正常）: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Err("没找到 gh（四个安装位都没探测到）".to_string())
}
