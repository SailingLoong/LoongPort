//! ChatGPT 桌面版（曾名 Codex）的退出与重开。
//!
//! 切换分组会改写 `~/.codex/config.toml`，而 ChatGPT 桌面版在启动时读它 —— 不重启就仍连
//! 旧分组。所以编排是：**提示 → 用户确认 → 退出 → 切换 → 成功弹窗 → 重开**。
//!
//! ## 为什么按 bundle id，不按进程名
//!
//! 这个 app 显示名是 ChatGPT，**bundle id 仍是 `com.openai.codex`**（实测
//! `osascript -e 'id of app "ChatGPT"'`）。而它内部还带一份真的 codex 二进制
//! （`/Applications/ChatGPT.app/Contents/Resources/codex`，约 270MB），`ps -o ucomm` 就叫
//! `codex` —— 与命令行 codex CLI 完全同名。
//!
//! **实测踩过**：`pkill -9 -x codex` 会连 ChatGPT.app 内嵌的那个一起杀掉。所以本模块一律
//! 走 bundle id，不做任何进程名匹配。
//!
//! ## 为什么是 AppleScript quit 而不是发信号
//!
//! `quit` 走 app 自己的退出流程（保存状态、关窗），等价于用户按 ⌘Q；发 SIGTERM/SIGKILL 是
//! 从外面掐断。对一个有未保存对话的 GUI app，前者是唯一负责任的做法。
//!
//! ## 平台边界
//!
//! **只有 macOS 有实现**，其它平台返回 [`AppError::Config`] 让调用方降级成「请手动重启
//! ChatGPT」的提示。这不是预留抽象层（那会踩过度设计），是跟随 cc-switch 既有惯例
//! （`session_manager/terminal/mod.rs` 就是同样的 macOS-only 早退）。
//!
//! 要加 Windows 时改动局限在本文件：Windows 没有 bundle id，等价物是给主窗口发 `WM_CLOSE`
//! 或 `taskkill /PID`（不带 `/F` 才是关闭请求），另需确认那边的 app 形态（MSIX vs exe）。

use std::time::Duration;

use crate::error::AppError;

/// ChatGPT 桌面版的 bundle id。**显示名是 ChatGPT，标识符仍是 codex**，别按名字找。
pub const CHATGPT_BUNDLE_ID: &str = "com.openai.codex";

/// 轮询「是否已退出」的间隔与上限。
///
/// 8 秒是这样来的：正常 quit 在 1 秒内完成；超过 8 秒基本只有一种情况 —— app 弹了
/// 「要保存吗」之类的确认框在等用户。那时候我们不该无限等，而该把控制权交回用户。
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const QUIT_TIMEOUT: Duration = Duration::from_secs(8);

/// 退出结果，用于给 UI 出不同的话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// 本来就没在跑。切换照常进行，之后**不重开**（用户没开着，我们不该替他开）。
    NotRunning,
    /// 已退出。切换后应重开。
    Quit,
    /// 发了 quit 但超时仍在跑（通常是 app 弹了确认框）。
    TimedOut,
}

/// 这个 app 装了没有。
///
/// 用 `osascript -e 'id of app "..."'` 而不是查 `/Applications` 路径：用户可能装在
/// `~/Applications` 或别处，Launch Services 知道，硬编码路径不知道。
pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        run_osascript(&format!(
            r#"try
    get id of application id "{CHATGPT_BUNDLE_ID}"
    return "yes"
on error
    return "no"
end try"#
        ))
        .map(|out| out.trim() == "yes")
        .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 是否正在运行。
///
/// `application id "x" is running` 是只读查询，不会把 app 启动起来（对比
/// `tell application "x" to ...` —— 那个会**唤起** app，是这里绝不能用的写法）。
pub fn is_running() -> Result<bool, AppError> {
    #[cfg(target_os = "macos")]
    {
        let out = run_osascript(&format!(
            r#"application id "{CHATGPT_BUNDLE_ID}" is running"#
        ))?;
        Ok(out.trim() == "true")
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// 优雅退出并等它真的退出。
pub fn quit_and_wait() -> Result<QuitOutcome, AppError> {
    #[cfg(target_os = "macos")]
    {
        if !is_running()? {
            return Ok(QuitOutcome::NotRunning);
        }

        // `quit app id "..."` 而不是 `tell application ... to quit`：前者不会在 app 已经
        // 退出的竞态下把它重新唤起。
        run_osascript(&format!(r#"quit app id "{CHATGPT_BUNDLE_ID}""#))?;

        let deadline = std::time::Instant::now() + QUIT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(QUIT_POLL_INTERVAL);
            // 轮询期间的查询失败不当致命错：app 正在退出时 osascript 偶发拿不到状态，
            // 下一轮就好了。真出不来由 TimedOut 兜。
            if let Ok(false) = is_running() {
                return Ok(QuitOutcome::Quit);
            }
        }
        Ok(QuitOutcome::TimedOut)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// 重新打开（不抢焦点）。
///
/// `open -g -b <bundle-id>`：`-g` 是不把它带到前台 —— 用户是在 LoongPort 里点的按钮，
/// 焦点该留在 LoongPort。app 自己启动完会不会抢焦点由它决定，我们至少不主动要求。
pub fn relaunch() -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("open")
            .args(["-g", "-b", CHATGPT_BUNDLE_ID])
            .output()
            .map_err(|e| AppError::Config(format!("启动 ChatGPT 失败: {e}")))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(AppError::Config(format!(
                "启动 ChatGPT 失败: {}",
                if err.is_empty() {
                    format!("open 退出码 {:?}", out.status.code())
                } else {
                    err
                }
            )));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> AppError {
    AppError::Config("当前平台暂不支持自动重启 ChatGPT，请手动重启它".into())
}

/// 跑一段 AppleScript，回传 stdout。
///
/// 失败时把 stderr 带出来 —— 这里最常见的失败是 macOS 的自动化授权被拒
/// （用户在系统弹窗里点了「不允许」），那条 stderr 是唯一能让用户看懂发生了什么的信息。
#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<String, AppError> {
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| AppError::Config(format!("执行 osascript 失败: {e}")))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // -1743 是 TCC 拒绝（「不允许控制其他应用」）。单独给话，否则用户看到的是一串
        // AppleScript 错误码，不知道要去哪儿开权限。
        if err.contains("-1743") || err.contains("not allowed") {
            return Err(AppError::Config(
                "没有控制 ChatGPT 的权限：请到「系统设置 → 隐私与安全性 → 自动化」\
                 里允许 LoongPort 控制 ChatGPT，然后重试"
                    .into(),
            ));
        }
        return Err(AppError::Config(format!("osascript 执行失败: {err}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_id_is_the_codex_one_not_a_chatgpt_lookalike() {
        // 这条钉死一个反直觉的事实：app 显示名叫 ChatGPT，bundle id 却是 com.openai.codex。
        // 有人「顺手改成 com.openai.chatgpt」时这条会红。
        assert_eq!(CHATGPT_BUNDLE_ID, "com.openai.codex");
    }

    #[test]
    fn quit_timeout_leaves_room_for_a_normal_quit() {
        // 正常 quit 亚秒级完成。超时太短会把正常退出误判成 TimedOut，
        // 太长会让用户在确认框卡住时干等。
        assert!(QUIT_TIMEOUT >= Duration::from_secs(5));
        assert!(QUIT_TIMEOUT <= Duration::from_secs(15));
        assert!(QUIT_POLL_INTERVAL < QUIT_TIMEOUT);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn is_running_query_does_not_launch_the_app() {
        // 真机行为：无论 app 在不在跑，这个查询都必须成功返回而不是报错，
        // 且不得把 app 启动起来（`is running` 是只读的，`tell ... to` 才会唤起）。
        let before = is_running();
        assert!(before.is_ok(), "查询状态不该失败: {before:?}");
        // 连查两次结果一致 —— 若第一次把 app 唤起了，第二次就会变 true。
        assert_eq!(is_running().unwrap(), before.unwrap());
    }
}
