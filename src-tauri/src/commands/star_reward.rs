//! 「点 Star 领注册礼」的机制层：星数取数 / gh CLI 代点 / 弹窗邀请 payload。
//!
//! 策略（什么时候弹、弹给谁、领过之后怎样）在 `commands::onboarding`（新人首启）
//! 与前端 `GitHubStarButton` / `StarRewardDialog`（红点入口与弹窗状态机）；
//! 本模块只提供三端共用的机制：
//! - 配置可用性（不碰网络，红点显隐用）；
//! - 带基线星数的完整邀请（新人引导事件与红点点击共用）；
//! - 两条「点星」通路：gh 直点（幂等，没装/没登录就返回不通）与手动
//!   （浏览器开仓库页，前端负责「我已点赞」后的二次取数比对）。
//!
//! 防作弊语义是**有意放开的**：不要求 GitHub 登录态授权，任何人点亮的 Star 都
//! 算数，校验只是文案差异（检测到 / 未确认也发）。低摩擦换转化，不做强校验。

use serde::Serialize;

use crate::config::GITHUB_REPO;

/// 弹窗邀请的 payload：`ONBOARDING_STAR_REWARD_OFFER` 事件与 `star_reward_offer`
/// 命令共用；前端 `src/lib/api/starReward.ts` 的 `StarRewardOffer` 与之对应。
///
/// 序列化 camelCase（本仓 TS 侧惯例），与 `commands::onboarding` 的
/// `RegisterCompletedPayload` 同一形状。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StarRewardOffer {
    pub promo_code: String,
    pub amount_usd: u64,
    /// 邀请成立那一刻的 star 数。前端只做一件事：用户点「我已点赞」后取一次
    /// 新数与它比对 —— 涨了说「已检测到」，没涨说「可能网络波动，照发」。
    pub baseline_stars: u64,
}

/// `GITHUB_REPO`（`https://github.com/{owner}/{repo}`）→ REST API 与 gh CLI 用的
/// `{owner}/{repo}`。从同一个常量派生而不是另写一份 —— 仓库搬家家时只改一处。
fn repo_api_slug() -> &'static str {
    GITHUB_REPO.trim_start_matches("https://github.com/")
}

/// 远端配置里的 star_reward 当前可用吗。空码 = 维护者撤销 = 活动下线，
/// 与 `remote_config::resolve_code` 的「空值 = 撤销」同一语义。
pub(crate) fn effective_star_reward() -> Option<crate::relay::remote_config::StarRewardConfig> {
    crate::relay::remote_config::load_cached()
        .and_then(|config| config.star_reward)
        .filter(|reward| !reward.promo_code.trim().is_empty())
}

/// 取当前 star 数。10s 超时、失败重试一次 —— 弹窗闸门与「我已点赞」校验共用。
/// 10s 是被实测校准的：国内直连 api.github.com 常态就要 5s+（2026-08-16 本机
/// 实测 5.2s，4s 超时把「慢但通」误判成「到不了」，新人引导弹窗整条被掐）。
/// 真到不了的网络两次也在 20s 内收敛，闸门跑在后台，不拖任何人干等。
async fn fetch_star_count() -> Result<u64, String> {
    let url = format!("https://api.github.com/repos/{}", repo_api_slug());
    let mut last_error = String::new();
    for _ in 0..2 {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| error.to_string())?;
        match client
            .get(&url)
            .header("User-Agent", "LoongPort")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(count) = json.get("stargazers_count").and_then(|v| v.as_u64()) {
                            return Ok(count);
                        }
                        last_error = "响应里没有 stargazers_count".into();
                    }
                    Err(error) => last_error = format!("GitHub API 响应解析失败: {error}"),
                }
            }
            Ok(response) => last_error = format!("GitHub API 返回 {}", response.status()),
            Err(error) => last_error = format!("GitHub API 请求失败: {error}"),
        }
    }
    Err(last_error)
}

/// 邀请成立的全套判定：配置在 + 基线取到。任何一步不成都 `None` —— 调用方
/// （新人引导事件、红点点击）一律静默回落到现状行为，不给用户看一个
/// 随时兑现不了的 offer。
pub(crate) async fn build_offer() -> Option<StarRewardOffer> {
    let reward = effective_star_reward()?;
    let baseline_stars = fetch_star_count().await.ok()?;
    Some(StarRewardOffer {
        promo_code: reward.promo_code.trim().to_string(),
        amount_usd: reward.amount_usd,
        baseline_stars,
    })
}

/// 红点入口的弹窗邀请。`None` = 活动不在 / 基线取不到，前端回落「直接开仓库」。
#[tauri::command]
pub async fn star_reward_offer() -> Result<Option<StarRewardOffer>, String> {
    Ok(build_offer().await)
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

/// star_reward 活动还在吗（只读缓存配置，不碰网络）。红点显隐用它 ——
/// 每次启动都为了一颗红点去拉一次基线太浪费。
#[tauri::command]
pub fn star_reward_configured() -> Result<bool, String> {
    Ok(effective_star_reward().is_some())
}

/// 当前 star 数（「我已点赞」后的第二次取数）。失败由前端按
/// 「可能网络波动，照发」处理，不在这里重试更多次。
#[tauri::command]
pub async fn github_star_count() -> Result<u64, String> {
    fetch_star_count().await
}

/// 用本机 gh CLI 直接点星。
///
/// 装了且登录过的用户免去跑浏览器；`PUT /user/starred/...` 幂等，早就点过也
/// 返回成功。任何失败都只是「这条路不通」（返回 `Ok(false)`），前端回落到开
/// 浏览器 —— 没装 gh 是常态而非异常，不值得当错误冒出来。
///
/// ⚠️ 这是**代用户用他自己的 GitHub 账号执行动作**，前提是用户刚在弹窗里
/// 确认过「点 Star 领礼」—— 弹窗文案必须写明会用本机 gh 代点，consent 摆在
/// 明面上。有意**不**去碰 git 凭据（keychain 里的 PAT）：scope 不可控、
/// 要动钥匙串，用 push 令牌点星在信任面上太难看，只用 gh 自己的鉴权边界。
#[tauri::command]
pub async fn github_star_via_gh() -> Result<bool, String> {
    // macOS 从 Finder/Dock 启动的 GUI 进程拿到的 PATH 很短，brew 装的 gh 不在
    // 里面 —— 先探测常见安装位，最后才试 PATH（Windows 的 winget 安装一般
    // 进 PATH，`gh` 那一项就是给它用的）。
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
            // 超时（下面 await 那层）后顺手杀掉，不留孤儿 gh 进程。
            .kill_on_drop(true);

        let output =
            match tokio::time::timeout(std::time::Duration::from_secs(3), command.output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    // 这个位置没装，试下一个候选。
                    continue;
                }
                Ok(Err(error)) => {
                    log::info!("gh 点星启动失败（{candidate}）: {error}");
                    return Ok(false);
                }
                Err(_) => {
                    log::info!("gh 点星超时（{candidate}）");
                    return Ok(false);
                }
            };

        if output.status.success() {
            return Ok(true);
        }
        // gh 在但没登录 / 令牌 scope 不够等 —— 换安装路径没意义（同一个 gh），
        // 直接判这条路不通。
        log::info!(
            "gh 点星未成功（{candidate}）: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(false);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_api_slug_derives_from_the_single_repo_constant() {
        // 与 GITHUB_REPO 同源派生；仓库常量改了这里会跟着红，防止 slug 写死分叉。
        assert_eq!(repo_api_slug(), "SailingLoong/LoongPort");
        assert!(
            repo_api_slug().split('/').count() == 2,
            "slug 必须是 owner/repo 两段，GITHUB_REPO 改形时这里要跟着看"
        );
    }
}
