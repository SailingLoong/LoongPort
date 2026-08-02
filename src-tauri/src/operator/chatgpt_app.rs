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
//! ## 平台边界：自动退出只有 macOS 有，但**切换在所有平台都能用**
//!
//! 非 macOS 平台走 [`QuitOutcome::NeedsManualRestart`]，调用方照常写配置、只是提示用户自己
//! 重启 ChatGPT。**这条路与「macOS 上权限被拒 / 命令出错」是同一条** —— 对用户都是「你自己
//! 关一下」，所以不分成两种处置。
//!
//! 关键是它**不返回错误**：把「没能替用户关掉那个 app」当失败会让这些平台上每次切换都失败，
//! 而配置本来是写得进去的。唯一中止切换的是用户在确认框里点了取消
//! （[`QuitOutcome::UserDeclined`]）。
//!
//! 要加 Windows 的自动退出时，改动局限在本文件：Windows 没有 bundle id，等价物是给主窗口发
//! `WM_CLOSE` 或 `taskkill /PID`（不带 `/F` 才是关闭请求），另需确认那边的 app 形态
//! （MSIX vs 传统 exe）。加之前先跑一次 `needs_user_attention` 那条注释里的判断 —— 那边现在
//! 恒为 true，加了实现之后才该按真实安装状态判。

// 只有 macOS 用得到：两个 QUIT_* 常量与轮询逻辑都在那边（Windows 上 gate 掉之后
// 这个 import 就成了 unused —— `-D warnings` 下同样会把 CI 判红）。
#[cfg(target_os = "macos")]
use std::time::Duration;

use crate::error::AppError;

/// ChatGPT 桌面版的 bundle id。**显示名是 ChatGPT，标识符仍是 codex**，别按名字找。
///
/// bundle id 是 macOS/Launch Services 的概念，所以这个常量与下面几个 AppleScript 参数一样
/// 只在 macOS 编译。Windows 上没有等价物（见模块文档末尾那段：那边要发 `WM_CLOSE`），
/// 无条件定义会让 `-D warnings` 下的 `dead_code` 把 Windows CI 判红。
#[cfg(target_os = "macos")]
pub const CHATGPT_BUNDLE_ID: &str = "com.openai.codex";

/// AppleScript 层的超时秒数。
///
/// **这个必须写在脚本里，不能只在 Rust 侧兜。** AppleEvent 的默认超时实测是 **120 秒**：
/// 当 ChatGPT 弹出确认框把主进程阻塞住时，`osascript` 会一直等到那 120 秒满才以 -1712
/// 失败。而 `std::process::Command::output()` 是同步阻塞的 —— 那就是把 Tauri command
/// 卡两分钟。包一层 `with timeout of N seconds` 实测能压到 N。
#[cfg(target_os = "macos")]
const APPLESCRIPT_TIMEOUT_SECS: u32 = 3;

/// 轮询「是否已退出」的间隔与上限。
///
/// 实测：quit 命令本身 0.08 秒返回（它是异步的），目标进程 0.24 秒后消失，150ms 间隔
/// 只需轮询 1-2 次。5 秒上限留了 20 倍余量；超过它基本只有一种情况 —— app 弹了确认框
/// 在等用户，那时该把控制权交回用户而不是继续等。
#[cfg(target_os = "macos")]
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(150);
#[cfg(target_os = "macos")]
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 退出结果。
///
/// ## 只有一种情况会中止切换
///
/// 这个枚举只分两类：**「用户明确说先别动」** 与 **「其余一切」**。
///
/// - [`UserDeclined`] 是唯一会中止切换的 —— 用户在 ChatGPT 的确认框里点了取消。
/// - 其余全部**照常切换**，需要时提示用户自己重启。理由：配置写进 `config.toml` 就已经
///   生效了，「能不能替用户关掉那个 app」是独立于「配置切没切」的一件事。把它当失败会让
///   没实现自动退出的平台、或者权限被拒的机器上**每次切换都失败**。
/// ## 为什么非 macOS 上要 `allow(dead_code)`
///
/// 这个枚举是**跨平台契约**：`commands/operator.rs` 在所有平台上都 match 全部四个分支。
/// 但当前只有 macOS 的 `quit_and_wait` 会构造 `Quit` / `NotRunning` / `UserDeclined`
/// （Windows 那条路恒返回 `NeedsManualRestart`），而 `dead_code` 只认「构造」不认「match」——
/// 于是 Windows 的 `-D warnings` 会把这三个变体判红。
///
/// 用 `cfg_attr` 而不是无条件 `allow`：macOS 上这三个确实在构造，那边的 dead_code 检查
/// 要留着 —— 哪天真没人构造了，该有人知道。加了 Windows 自动退出实现之后（模块文档末尾
/// 那段）这个 attribute 就该删掉。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// 已退出。切换后应重开。
    Quit,
    /// 没装、或本来没在跑。切换照常，之后**不重开**（用户没开着，我们不该替他开）。
    NotRunning,
    /// 自动退出没成功，需要用户自己重启。切换照常进行。
    ///
    /// 涵盖三种原因，对用户是同一件事（「你自己关一下」），所以不分开：
    /// - 本平台没有自动退出的实现（当前只有 macOS 有）
    /// - 系统拒绝了自动化权限（macOS 的 TCC）
    /// - 执行 `osascript` 本身出错
    NeedsManualRestart(&'static str),
    /// **用户在确认框里点了取消** —— 唯一会中止切换的情况。
    ///
    /// ChatGPT 在有进行中的对话时会弹阻塞式确认框。用户点取消就是明确表示「先别动」，
    /// 这时候硬写配置的后果是：它还活着、并且它自己会回写 `config.toml`，两边互相覆盖，
    /// 用户既没切成也不知道现在连的是哪个。
    UserDeclined,
}

/// 切换分组前要不要先提示用户处理 ChatGPT。
///
/// 语义是**「这台机器上切换分组需要管 ChatGPT 吗」**，不是「装了没有」—— 后者在非 macOS
/// 上答不出来（没有 Launch Services 那种一句话查得到的东西）。
///
/// - macOS：判据是 `is_running` 报不报 `-1728`（"不能获得 application id"，实测就是"没装"
///   的信号）。没装就不必打扰用户。**不要用 `path to application id`** —— 实测它会挂住 25
///   秒以上不返回。
/// - 其它平台：**恒为 true**。我们查不到装没装，但如果用户装了、又不提示他重启，他会拿着
///   旧分组跑而完全不知道。宁可对没装的用户多问一句（他点「只切换」就好），也不能让装了的
///   用户静默用错分组。
pub fn needs_user_attention() -> bool {
    #[cfg(target_os = "macos")]
    {
        // 能查到运行状态（无论 true/false）就说明 Launch Services 认得这个 bundle id。
        is_running().is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
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
///
/// **只在 macOS 存在**：它问的是 Launch Services，其它平台没有等价的一句话查询（这正是
/// [`needs_user_attention`] 在那些平台上恒为 true 的原因）。以前这里留了一个返回
/// `Err(unsupported())` 的非 macOS 分支，但没有任何非 macOS 调用点 —— 于是 Windows 上
/// `-D warnings` 把它判成 dead_code。
#[cfg(target_os = "macos")]
pub fn is_running() -> Result<bool, AppError> {
    let out = run_osascript(&format!(
        r#"application id "{CHATGPT_BUNDLE_ID}" is running"#
    ))?;
    Ok(out.trim() == "true")
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
///
/// ## 返回 `QuitOutcome` 而不是 `Result`
///
/// 这个函数**不失败**。所有「没能替用户关掉」的原因（平台没实现、权限被拒、命令出错）
/// 都归到 [`QuitOutcome::NeedsManualRestart`]，让调用方照常切换并提示用户自己重启。
/// 把它们当错误会让没实现自动退出的平台上每次切换都失败 —— 而配置本来是能写的。
pub fn quit_and_wait() -> QuitOutcome {
    #[cfg(target_os = "macos")]
    {
        let running = match is_running() {
            Ok(r) => r,
            // 查不到状态：可能没装（-1728），也可能是自动化权限被拒。前者不需要重启、
            // 后者需要用户自己来，但我们分不清 —— 统一按「没在跑」处理最不惹事：
            // 切换照常，不提示用户去关一个可能根本没装的 app。
            //
            // 真的是权限问题时用户会在下一步「重开」那里看到提示（relaunch 也会失败）。
            Err(e) => {
                log::debug!("查 ChatGPT 运行状态失败（可能没装）: {e}");
                return QuitOutcome::NotRunning;
            }
        };
        if !running {
            return QuitOutcome::NotRunning;
        }

        // `quit application id "..."` 而不是 `tell application ... to quit`：两者其实是同一个
        // Apple event，但前者在 app 已退出的竞态下不会把它重新唤起。
        //
        // `with timeout` 是硬要求，见 APPLESCRIPT_TIMEOUT_SECS 的说明（默认 120 秒）。
        if let Err(e) = run_osascript(&format!(
            "with timeout of {APPLESCRIPT_TIMEOUT_SECS} seconds\n\
             quit application id \"{CHATGPT_BUNDLE_ID}\"\n\
             end timeout"
        )) {
            // 权限被拒之类的硬失败：切换照常，让用户自己关。
            //
            // **超时（-1712）不算这一类** —— 那通常是确认框挡住了，还要靠下面的轮询区分
            // 「用户点了取消」与「只是慢了一点」，所以不在这里提前返回。
            let msg = e.to_string();
            if !msg.contains("-1712") {
                log::warn!("退出 ChatGPT 失败: {msg}");
                return QuitOutcome::NeedsManualRestart("退出 ChatGPT 时出错");
            }
        }

        let deadline = std::time::Instant::now() + QUIT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(QUIT_POLL_INTERVAL);
            // 轮询期间的查询失败不当致命错：app 正在退出时偶发拿不到状态，下一轮就好了。
            if let Ok(false) = is_running() {
                return QuitOutcome::Quit;
            }
        }
        // 等满了它还活着 —— 几乎只有一种解释：确认框弹出来了，用户点了取消（或还没理它）。
        QuitOutcome::UserDeclined
    }
    #[cfg(not(target_os = "macos"))]
    {
        QuitOutcome::NeedsManualRestart("当前系统暂不支持自动重启 ChatGPT")
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

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_id_is_the_codex_one_not_a_chatgpt_lookalike() {
        // 这条钉死一个反直觉的事实：app 显示名叫 ChatGPT，bundle id 却是 com.openai.codex。
        // 有人「顺手改成 com.openai.chatgpt」时这条会红。
        //
        // 2026-08-02 复核：/Applications/ChatGPT.app 的 CFBundleIdentifier 实测就是
        // com.openai.codex（CFBundleName 才是 ChatGPT），而 com.openai.chat 在系统里
        // 根本解析不出来（-1728）。
        assert_eq!(CHATGPT_BUNDLE_ID, "com.openai.codex");
    }

    #[test]
    fn only_user_declined_aborts_the_switch() {
        // 这条钉住整个模块的取舍：**只有「用户明确说先别动」才中止切换**，其余一切
        // （平台没实现、权限被拒、命令出错）都照常切换 + 提示手动重启。
        //
        // 会红的改法：给 NeedsManualRestart 加上「中止」语义，或者把它拆回一堆 Err ——
        // 那会让没实现自动退出的平台上每次切换都失败，而配置本来是能写的。
        fn aborts(o: QuitOutcome) -> bool {
            matches!(o, QuitOutcome::UserDeclined)
        }
        assert!(aborts(QuitOutcome::UserDeclined));
        assert!(!aborts(QuitOutcome::Quit));
        assert!(!aborts(QuitOutcome::NotRunning));
        assert!(!aborts(QuitOutcome::NeedsManualRestart("任何原因")));
    }

    #[test]
    fn quit_never_reports_failure_as_an_error() {
        // quit_and_wait 的签名是 QuitOutcome 而不是 Result —— 这本身就是那条取舍的载体。
        // 真机上跑一次：无论 ChatGPT 装没装、在不在跑，它都得给出一个 outcome。
        let outcome = quit_and_wait_is_infallible();
        assert!(
            matches!(
                outcome,
                QuitOutcome::Quit
                    | QuitOutcome::NotRunning
                    | QuitOutcome::NeedsManualRestart(_)
                    | QuitOutcome::UserDeclined
            ),
            "outcome 必须是四者之一: {outcome:?}"
        );
    }

    /// 只是给上面那条测试一个不真的去退出 ChatGPT 的替身。
    ///
    /// 直接调 `quit_and_wait()` 会**真的把用户的 ChatGPT 关掉** —— 测试不该有这种副作用。
    /// 这里只验「非 macOS 分支返回的是 outcome 而不是错误」这个类型层面的事实；macOS 上
    /// 那条路径由 `is_running_query_does_not_launch_the_app` 与手工验证覆盖。
    fn quit_and_wait_is_infallible() -> QuitOutcome {
        #[cfg(target_os = "macos")]
        {
            // 不真的退出：只走到「查状态」这一步，它是只读的。
            match is_running() {
                Ok(false) => QuitOutcome::NotRunning,
                Ok(true) => QuitOutcome::UserDeclined, // 装了且在跑，不去动它
                Err(_) => QuitOutcome::NotRunning,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            quit_and_wait()
        }
    }

    #[cfg(target_os = "macos")]
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
        // 这条要钉的是**只读性**：`is running` 不得把 app 唤起（`tell ... to` 才会）。
        // 判据是连查两次结果一致 —— 若第一次唤起了它，第二次就会翻成 true。
        //
        // ⚠️ **不能断言 `is_ok()`**：那要求这台机器装了 ChatGPT.app。CI 的 macOS runner
        // 是干净环境，没装，于是查询如实报 -1728（"不能获得 application id"）——
        // 而 `unknown_bundle_id_is_an_error_not_a_silent_false` 那条测试正是把 -1728
        // 钉成「没装」的信号。两条一起要求「没装时既要报错、又要不报错」，自相矛盾，
        // 实测让 CI 的 Backend Checks (macos-latest) 红在这里。
        //
        // 所以按「装了 / 没装」分开断言，两种环境下都验到该验的东西。
        let before = is_running();
        match before {
            // 装了：两次查询必须一致（真正的只读性检查）。
            Ok(first) => assert_eq!(
                is_running().expect("第一次查得到，第二次也该查得到"),
                first,
                "两次查询结果不一致 —— 说明第一次把 app 唤起了"
            ),
            // 没装：错误必须是稳定可复现的，而不是时好时坏。
            Err(_) => assert!(
                is_running().is_err(),
                "没装 ChatGPT 时两次查询都该报错（-1728），不该一次成一次败"
            ),
        }
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
