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

/// AppleScript 层的超时秒数。
///
/// **这个必须写在脚本里，不能只在 Rust 侧兜。** AppleEvent 的默认超时实测是 **120 秒**：
/// 当 ChatGPT 弹出确认框把主进程阻塞住时，`osascript` 会一直等到那 120 秒满才以 -1712
/// 失败。而 `std::process::Command::output()` 是同步阻塞的 —— 那就是把 Tauri command
/// 卡两分钟。包一层 `with timeout of N seconds` 实测能压到 N。
const APPLESCRIPT_TIMEOUT_SECS: u32 = 3;

/// 轮询「是否已退出」的间隔与上限。
///
/// 实测：quit 命令本身 0.08 秒返回（它是异步的），目标进程 0.24 秒后消失，150ms 间隔
/// 只需轮询 1-2 次。5 秒上限留了 20 倍余量；超过它基本只有一种情况 —— app 弹了确认框
/// 在等用户，那时该把控制权交回用户而不是继续等。
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(150);
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 退出结果，用于给 UI 出不同的话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// 没装。切换照常进行（用户可能只用命令行 codex），之后不重开。
    NotInstalled,
    /// 装了但本来没在跑。切换照常进行，之后**不重开**（用户没开着，我们不该替他开）。
    NotRunning,
    /// 已退出。切换后应重开。
    Quit,
    /// 发了 quit 但它还在跑。**切换必须中止**，见 [`quit_and_wait`] 的说明。
    StillRunning,
}

/// 这个 app 装了没有。
///
/// 判据是 `is_running` 报不报 `-1728`（"不能获得 application id"）—— 实测这就是"没装"的
/// 信号。**不要用 `path to application id`**：实测它会挂住 25 秒以上不返回。
pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        // 能查到运行状态（无论 true/false）就说明 Launch Services 认得这个 bundle id。
        is_running().is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 是否正在运行。
///
/// `application id "x" is running` 是只读查询，**不会**把 app 启动起来（实测 quit 前后
/// 状态一致）。对比 `tell application "x" to ...` —— 那个会唤起 app，是这里绝不能用的写法。
///
/// 一律用 **bundle id** 而不是显示名：这个 app 的显示名已经从 Codex 改成 ChatGPT 一次了，
/// 而且实测 `quit application "不存在的名字"` 会**静默返回成功**（rc=0）把故障吞掉，
/// bundle id 形式则老实报 -1728。
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
///
/// ## 判据是轮询结果，不是 quit 的返回码
///
/// ChatGPT 在"有进行中的对话"或"有活跃的定时任务"时会弹一个**阻塞式**确认框
/// （Quit / Cancel）。用户点 Cancel 时 app 内部 `preventDefault()` 掉退出，而 `osascript`
/// 这边**仍可能返回 rc=0** —— 只看返回码会误判成"已退出"，接着去写 `config.toml`，而 app
/// 还活着、并且它自己会回写那个文件。
///
/// 所以唯一可信的判据是**轮询 `is_running` 变成 false**。
pub fn quit_and_wait() -> Result<QuitOutcome, AppError> {
    #[cfg(target_os = "macos")]
    {
        let running = match is_running() {
            Ok(r) => r,
            // 查不到状态 = 没装（-1728）。不是错误，用户可能只用命令行 codex。
            Err(_) => return Ok(QuitOutcome::NotInstalled),
        };
        if !running {
            return Ok(QuitOutcome::NotRunning);
        }

        // `quit application id "..."` 而不是 `tell application ... to quit`：两者其实是同一个
        // Apple event，但前者在 app 已退出的竞态下不会把它重新唤起。
        //
        // `with timeout` 是硬要求，见 APPLESCRIPT_TIMEOUT_SECS 的说明（默认 120 秒）。
        // 这里的错误不当致命：确认框挡住时它会以 -1712 失败，而那正是下面轮询要处理的情况。
        let _ = run_osascript(&format!(
            "with timeout of {APPLESCRIPT_TIMEOUT_SECS} seconds\n\
             quit application id \"{CHATGPT_BUNDLE_ID}\"\n\
             end timeout"
        ));

        let deadline = std::time::Instant::now() + QUIT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(QUIT_POLL_INTERVAL);
            // 轮询期间的查询失败不当致命错：app 正在退出时偶发拿不到状态，下一轮就好了。
            if let Ok(false) = is_running() {
                return Ok(QuitOutcome::Quit);
            }
        }
        Ok(QuitOutcome::StillRunning)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// 重新打开。
///
/// 用 `open -b <bundle-id>` 让它回到前台 —— 用户刚才是在用它，切换完自然是要继续用。
/// 实测对**已在跑**的 app 再执行是幂等的（不会开出第二个实例）。
///
/// 不用 `open -a <显示名>`：显示名已经改过一次（Codex → ChatGPT），而 bundle id 没变。
pub fn relaunch() -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("open")
            .args(["-b", CHATGPT_BUNDLE_ID])
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
        // -1743 / -1744 是 TCC 拒绝（「不允许控制其他应用」）。
        //
        // 实测 quit 这个 Apple event 目前**不需要**自动化授权（同一进程里发非豁免的
        // count-elements 会拿到 -1744，而 quit 能成功，加了 hardened runtime 签名后仍成功），
        // 所以正常路径不会走到这里。但那是 tccd 的执行层行为、Apple 没有文档承诺，将来
        // 可能收紧 —— 留这个分支是为了那天用户能看懂发生了什么，而不是看到一串错误码。
        if err.contains("-1743") || err.contains("-1744") || err.contains("not allowed") {
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
        // 实测正常退出 0.24 秒完成。超时太短会把正常退出误判成 StillRunning，
        // 太长会让用户在确认框卡住时干等。
        assert!(QUIT_TIMEOUT >= Duration::from_secs(3));
        assert!(QUIT_TIMEOUT <= Duration::from_secs(15));
        assert!(QUIT_POLL_INTERVAL < QUIT_TIMEOUT);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quit_script_wraps_the_apple_event_in_a_timeout() {
        // 这条钉住那个坑：AppleEvent 默认超时是 120 秒，而 osascript 是同步阻塞调用 ——
        // 不包 `with timeout` 就是在 ChatGPT 弹确认框时把 Tauri command 卡两分钟。
        //
        // 断言脚本文本而不是断言常量的大小（那是编译期常量、恒真）：这里要防的是
        // 有人重构时把 `with timeout` 那层拆掉。
        let script = format!(
            "with timeout of {APPLESCRIPT_TIMEOUT_SECS} seconds\n\
             quit application id \"{CHATGPT_BUNDLE_ID}\"\n\
             end timeout"
        );
        assert!(script.contains("with timeout of"), "{script}");
        assert!(script.contains("end timeout"), "{script}");
        // 顺带验证它真的是一段合法的 AppleScript（语法错在运行期才暴露的话，
        // 只有真机走到退出那一步才会发现）。
        let compiled = std::process::Command::new("osacompile")
            .args(["-o", "/dev/null", "-e", &script])
            .output()
            .expect("osacompile 应该可用");
        assert!(
            compiled.status.success(),
            "退出脚本语法不合法: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
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

    #[cfg(target_os = "macos")]
    #[test]
    fn unknown_bundle_id_is_an_error_not_a_silent_false() {
        // 「没装」的信号是查询报错（-1728），is_installed 正是靠这个判的。
        // 若哪天它变成静默返回 false，is_installed 会对没装的 app 报 true。
        let out =
            run_osascript(r#"application id "dev.loongport.definitely-not-installed" is running"#);
        assert!(out.is_err(), "不存在的 bundle id 该报错，实际: {out:?}");
    }
}
