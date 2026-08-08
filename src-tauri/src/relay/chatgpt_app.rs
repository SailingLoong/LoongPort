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
//! ## macOS 为什么是 AppleScript quit 而不是发信号
//!
//! `quit` 走 app 自己的退出流程（保存状态、关窗），等价于用户按 ⌘Q；发 SIGTERM/SIGKILL 是
//! 从外面掐断。对一个有未保存对话的 GUI app，前者是唯一负责任的做法 ——
//! **而这条路在 macOS 上走得通**（那边它响应 Apple event）。Windows 走不通，见下面那节。
//!
//! ## 平台边界：两边都能自动退出，但手段与语义不同
//!
//! | | 手段 | 用户能否拒绝 | 退出失败时 |
//! |---|---|---|---|
//! | macOS | AppleScript `quit`（协作式） | **能** —— 它有进行中对话会弹自己的确认框 | 权限被拒 / 出错 → [`QuitOutcome::NeedsManualRestart`] |
//! | Windows | `taskkill /F`（强制） | **不能** —— 所以事前弹窗告知是必需的 | 杀不掉（罕见）→ 同上 |
//! | Linux | 无（ChatGPT 不发 Linux 版） | — | 恒 [`QuitOutcome::NotRunning`] |
//!
//! 关键是 [`quit_and_wait`] **不返回错误**：把「没能替用户关掉那个 app」当失败会让
//! 权限被拒的机器上每次切换都失败，而配置本来是写得进去的。
//! 唯一中止切换的是 [`QuitOutcome::UserDeclined`]（**只有 macOS 会给出**——
//! 强制终止不给应用拒绝的机会）。
//!
//! ## Windows 的自动退出：`WM_CLOSE` 这条路**已被实测证伪**（2026-08-03）
//!
//! 本段原来写着「等价物是给主窗口发 `WM_CLOSE` 或 `taskkill /PID`（不带 `/F` 才是关闭
//! 请求）」。那只是**设想，从未验证**；实测下来它不成立，照着写会得到一个静默错误的实现。
//!
//! 在维护者的 Windows 机器上实测（与 ChatGPT 同一个桌面会话，主窗口句柄正常）：
//!
//! ```text
//! taskkill /PID <主进程>          # 不带 /F，即请求关闭而非强杀
//!   SUCCESS: Sent termination signal to the process with PID 22396.
//!   exit code: 0                  # ← 报成功
//! 10 秒后：9 个进程一个没退        # ← 但什么都没发生
//! 用户在屏幕前看到的：窗口只是最小化了
//! ```
//!
//! 原因：ChatGPT 桌面版是 **Electron 应用，且开了 minimize-to-tray** —— 它把 `WM_CLOSE`
//! 当「关窗口」处理（`event.preventDefault()` + 隐藏），进程继续跑。**跟本 app 自己的
//! `minimize_to_tray_on_close` 是同一套行为**，所以这不是它特立独行，而是这类 app 的常态。
//!
//! ⚠️ **危险之处不是「失败」，是「假成功」**：命令返回 0，只看返回码的实现会判定「已退出」，
//! 接着去改 `~/.codex` —— 而 app 还活着、还持着那些文件。这正是
//! [`quit_and_wait`] 坚持「判据是轮询结果，不是命令返回码」的原因（macOS 那边同理，
//! 见它的文档）。**任何 Windows 实现都必须沿用这条：轮询进程真的没了才算退出。**
//!
//! ### 实测到的其它事实（将来实现时直接用，不必重新摸索）
//!
//! - **它是 MSIX/AppX 包**，不是传统 exe：包名 `OpenAI.Codex`，进程名 `ChatGPT.exe`。
//!   查装没装要用 `Get-AppxPackage OpenAI.Codex`，**卸载表里查不到**。
//! - **绝不要写死安装路径**：`PackageFullName` 与 `InstallLocation` 都含版本号
//!   （`OpenAI.Codex_26.727.6591.0_x64__…`），每次更新就变；而 `C:\Program Files\WindowsApps\`
//!   默认拒绝访问。稳定标识只有包名与 `PackageFamilyName`。
//! - **重开有效**：`explorer.exe shell:AppsFolder\<PackageFamilyName>!App`（AUMID 运行时算，
//!   别写死那串 publisher hash —— 换签名证书会变）。
//! - **一个 app 有 9 个进程**（Electron 多进程）。主进程判据是「命令行里没有 `--type=`
//!   且父进程不是 `ChatGPT.exe`」。**不能靠 `MainWindowHandle != 0` 判** —— 跨会话看它恒为 0。
//! - 托盘菜单里那一项叫 **`Exit`**（不是 `Quit`），且会随系统语言变 ⇒ 靠文案匹配的
//!   UI 自动化很脆，不是好路子。
//!
//! ## Windows 的实测事实：**它不持有文件，但只在启动时读配置**
//!
//! | 实测项 | 结果 |
//! |---|---|
//! | 运行中独占打开 `config.toml`（`FileShare.None`） | **成功** ⇒ 没有任何进程持着它 |
//! | 运行中往 `config.toml` 加一行标记，观察 60 秒 | 标记始终在 ⇒ **运行期不回写** |
//! | 用户从托盘 `Exit` 退出后再看那行标记 | **仍在** ⇒ **退出时也不覆盖** |
//! | 完整启动 + 运行 + 退出 后看 `auth.json` mtime | **纹丝未动**（还是一周前）|
//! | 运行中把 provider 显示名改掉，看它界面 | **不变** ⇒ **只在启动时读** |
//!
//! 两条结论方向相反，缺一不可：
//!
//! 1. **它不回写 `~/.codex`** ⇒ 「改配置前必须先退出它」这个前提在 Windows 上**不成立**
//!    （macOS 上成立，那边确实会回写）。所以改配置本身**永远安全**，不该因为退不掉就中止。
//! 2. **它只在启动时读** ⇒ 不重启它，新配置**对桌面版永远不生效**。
//!
//! 所以 Windows 的处置是：**配置照改（不受退出成败影响），但要帮用户重启它**。
//!
//! ## Windows 的退出手段：`taskkill /F` + 事前警告，别再找「优雅退出」
//!
//! 上面已实测：`WM_CLOSE` 被它吃掉（只最小化）。而 Windows **没有 SIGTERM**，
//! 对 GUI 应用只有两种手段 —— WM_CLOSE（无效）与强制终止。逐一排除过的：
//!
//! | 手段 | 为什么不用 |
//! |---|---|
//! | 官方 reload / restart 接口 | **不存在**（翻遍内嵌 `codex.exe` 的 22 个子命令，只有 `app` 启动器）|
//! | 让自己当父进程再优雅关子进程 | MSIX 必须经 AUMID 激活，父进程恒为激活器（实测 `explorer.exe`）；且 Job Object 那套**也是强杀** |
//! | 提管理员权限 | 解决的是「权限不足」，而我们从没遇到 —— `taskkill` 返回的是 `SUCCESS`、exit 0，是消息被应用吃掉，提权改不了应用逻辑；代价是每次启动弹 UAC |
//! | UI 自动化点托盘的 `Exit` | 文案随系统语言变，靠文案匹配太脆 |
//!
//! 所以只剩强制终止。**而强制终止是可以做的，前提是事前告知** ——
//! 这正是 Windows 安装程序（Restart Manager）的标准模式，也与本模块 macOS 侧
//! 「提示 → 用户确认 → 退出」的编排同构（见文件开头那行流程）。用户点了「退出并切换」
//! 就是知情同意；他要保存对话，可以点「只切换，我自己重启」。
//!
//! 风险比看上去小：ChatGPT 的会话状态落在 `~/.codex` 的 SQLite 里
//! （`logs_*.sqlite` / `state_*.sqlite` / `memories_*.sqlite`，都带 `-wal`）——
//! SQLite 本身是抗崩溃设计的，不是「进程一死就丢一整段对话」那种形态。
//! 但**仍要在弹窗里说清「会强制关闭、未保存内容可能丢失」**：不确定的风险要告知用户，
//! 由他决定，而不是我们替他判断「应该没事」。

// macOS 与 Windows 都要轮询「真的退了吗」，所以 QUIT_* 那两个常量两边共用。
// Linux 上没有实现 ⇒ 那里 gate 掉，否则 `-D warnings` 会把它判成 dead_code。
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::Duration;

use crate::error::AppError;

/// ChatGPT 桌面版的 bundle id。**显示名是 ChatGPT，标识符仍是 codex**，别按名字找。
///
/// bundle id 是 macOS/Launch Services 的概念，所以这个常量与下面几个 AppleScript 参数一样
/// 只在 macOS 编译。Windows 的等价物是 [`CHATGPT_PROCESS_NAME`] 与
/// [`CHATGPT_PACKAGE_NAME`]（那边没有 bundle id 这个概念）。
#[cfg(target_os = "macos")]
pub const CHATGPT_BUNDLE_ID: &str = "com.openai.codex";

/// Windows 上 ChatGPT 桌面版的**进程名**。
///
/// ⚠️ 显示名是 ChatGPT、MSIX 包名却是 `OpenAI.Codex`，而**可执行文件叫 `ChatGPT.exe`**
/// （包内 `app/ChatGPT.exe`）—— 三个名字不一致，别互相替换。实测确认。
///
/// 与 macOS 那边的处境相反：那边进程名 `codex` 会与命令行 codex CLI 撞名（所以只认
/// bundle id），Windows 这边 `ChatGPT.exe` 反而是干净的 —— 内嵌的 CLI 叫 `codex.exe`，
/// 不会被误伤。
#[cfg(target_os = "windows")]
const CHATGPT_PROCESS_NAME: &str = "ChatGPT.exe";

/// Windows 上的 MSIX 包名。用来算 AUMID（重开）与判「装了没有」。
///
/// **只用这个稳定名，绝不碰 `PackageFullName` / `InstallLocation`** —— 那两个含版本号
/// （`OpenAI.Codex_26.727.6591.0_x64__…`），每次更新就变，而
/// `C:\Program Files\WindowsApps\` 默认还拒绝访问。
#[cfg(target_os = "windows")]
const CHATGPT_PACKAGE_NAME: &str = "OpenAI.Codex";

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
/// 实测（macOS）：quit 命令本身 0.08 秒返回（它是异步的），目标进程 0.24 秒后消失，
/// 150ms 间隔只需轮询 1-2 次。5 秒上限留了 20 倍余量；超过它基本只有一种情况 ——
/// app 弹了确认框在等用户，那时该把控制权交回用户而不是继续等。
///
/// Windows 上共用这两个值：那边是 `taskkill /F`（内核直接终止，比 AppleScript 的
/// 协作式退出更快），5 秒同样是宽裕的上限。**但那边超时的含义不同** ——
/// 强制终止不会被用户挡住，所以超时只可能是「杀不掉」（权限/句柄问题），
/// 不该像 macOS 那样解读成 `UserDeclined`。见 [`quit_and_wait`]。
#[cfg(any(target_os = "macos", target_os = "windows"))]
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(150);
#[cfg(any(target_os = "macos", target_os = "windows"))]
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
///   权限被拒的机器上**每次切换都失败**。
///
/// ## 为什么非 macOS 上要 `allow(dead_code)`
///
/// 这个枚举是**跨平台契约**：`commands/relay.rs` 在所有平台上都 match 全部四个分支。
/// 但 [`Self::UserDeclined`] **只有 macOS 会构造** —— 那边的 AppleScript `quit` 是协作式的，
/// 用户能在 app 自己的确认框里拒绝；Windows 用 `taskkill /F`，不给拒绝的机会，
/// Linux 压根没实现。而 `dead_code` 只认「构造」不认「match」⇒ 那两个平台会把它判红。
///
/// 用 `cfg_attr` 而不是无条件 `allow`：macOS 上它确实在构造，那边的 dead_code 检查
/// 要留着 —— 哪天真没人构造了，该有人知道。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// 已退出。切换后应重开。
    Quit,
    /// 没装、或本来没在跑。切换照常，之后**不重开**（用户没开着，我们不该替他开）。
    NotRunning,
    /// 试过了但没能关掉，需要用户自己重启。切换照常进行（除非调用方要求严格）。
    ///
    /// 涵盖三种原因，对用户是同一件事（「你自己关一下」），所以不分开：
    /// - macOS：系统拒绝了自动化权限（TCC）
    /// - macOS：执行 `osascript` 本身出错
    /// - Windows：`taskkill /F` 之后进程仍在（罕见；权限或句柄问题）
    ///
    NeedsManualRestart(&'static str),
    /// **用户在确认框里点了取消** —— 唯一会中止切换的情况。
    ///
    /// ChatGPT 在有进行中的对话时会弹阻塞式确认框。用户点取消就是明确表示「先别动」，
    /// 这时候硬写配置的后果是：它还活着、并且它自己会回写 `config.toml`，两边互相覆盖，
    /// 用户既没切成也不知道现在连的是哪个。
    UserDeclined,
}

/// 「退 ChatGPT → 做事 → 重开」这套编排的结果。
#[derive(Debug, Default)]
pub struct AroundOutcome {
    /// 我们把它关掉了吗（关了才会去重开）。
    pub was_running: bool,
    /// 有没有重新打开它。
    pub relaunched: bool,
    /// 非致命问题（平台没实现自动退出、重开失败）。如实带给用户。
    pub warnings: Vec<String>,
}

/// 在「退掉 ChatGPT」的窗口里执行 `action`，做完再把它开回去。
///
/// ## 为什么要有这个函数
///
/// **macOS 上**凡是会改动 `~/.codex` 的操作都得先退 ChatGPT —— 那边它持有那个目录、
/// 且**退出时会回写 `config.toml` 与 `auth.json`**。不先退的后果不是报错，
/// 是两边互相覆盖：用户以为切成了，实际 ChatGPT 一退出就把旧配置写回来，
/// 而没有任何东西会提示他。
///
/// **Windows 上这个前提不成立**（实测：运行中不持句柄、退出后我们的改动仍在、
/// `auth.json` 整个启停周期 mtime 未变 —— 见模块文档那张表）。那边退出 ChatGPT
/// 的理由是另一个：**它只在启动时读配置**，不重启新配置就不生效。
///
/// 这套编排（四个 quit 分支 × 失败要不要回滚 × 只在关过时才重开）原来只在
/// `switch_tier_impl` 里有一份。要让**上游那条通用 provider 切换**也享受同样的保护，
/// 只有两条路：把那段复制第二遍，或者抽成这个函数。复制的必然结局是两份慢慢分叉 ——
/// 而分叉的表现是「从 LoongPort 页切没问题、从 provider 页切就静默用错配置」，
/// 那种 bug 没人会想到去对比两处实现。
///
/// ## `action` 失败时会把 ChatGPT 开回去
///
/// 我们关的，就得负责开回来。否则用户手上是「ChatGPT 被关了、事情没办成、
/// 也没人告诉他现在是什么状态」。开回去的是**原样** —— action 失败意味着配置没动。
///
/// ## 四个 quit 分支的处理由 `abort_on_unconfirmed_exit` 决定
///
/// 这是唯一需要调用方拍板的地方，因为**它取决于 action 会不会碰 ChatGPT 会回写的文件**：
///
/// - `false`（切 provider）：只写 `config.toml`。退不掉也照常做 + 提示手动重启 ——
///   配置写进去就已经生效了，硬拦住用户没有收益。
/// - `true`（删 `auth.json`）：ChatGPT **退出时会重写它** ⇒ 没确认它退出就动手等于白删。
///
/// ## ⚠️ 中止只在 macOS 发生（2026-08-03 实测后改）
///
/// 中止的**唯一理由**是「ChatGPT 退出时会重写 `~/.codex`，没退干净就白删」——
/// 而那是 **macOS 上验证过**的行为。Windows 上实测**不成立**（见模块文档那张表：
/// 运行中不持句柄、退出后 `config.toml` 的改动仍在、`auth.json` mtime 纹丝未动）。
///
/// 理由不成立就不该有那个后果：那边即使没关掉 ChatGPT，删 `auth.json` 也是真的删掉了。
/// 中止只会把用户挡在一件**本来已经做成**的事前面。
///
/// 所以判据不是「调用方要不要严格」，而是「**这个平台上那个前提成立吗**」：
/// 成立（macOS）才中止，不成立（Windows / Linux）就照常做 + 提示。
///
/// （历史：这里原先写「那边 `quit_and_wait` 恒返回 `NeedsManualRestart` ⇒ 用户陷入死循环」。
/// 那在 Windows 还没实现自动退出时是对的；现在那边会真的去杀、给出真实结果，
/// 所以理由换成上面这条 —— 结论没变。）
/// 那边本来就不需要先退出，中止只是把一个不存在的风险变成一个真实的死锁。
///
/// `UserDeclined` **两种情况都中止** —— 用户在确认框点了取消，那是明确的「先别动」。
///
/// 编排本身在 [`around_with`]，这里只是把真实的「退」与「开」喂给它。
pub fn around<T>(
    abort_on_unconfirmed_exit: bool,
    action: impl FnOnce() -> Result<T, AppError>,
) -> Result<(T, AroundOutcome), AppError> {
    around_with(abort_on_unconfirmed_exit, quit_and_wait, relaunch, action)
}

/// [`around`] 的实现，把「退」与「开」两个副作用作为参数收进来。
///
/// ## 为什么要有这个接缝（2026-08-04 加，review 抓出）
///
/// 原来编排直接写死调 `quit_and_wait()`，于是**唯一一条测它的单元测试会真的去退用户的
/// ChatGPT**。实测：跑一次 `cargo test`，ChatGPT 的 pid 从 29330 变成 36016 —— 真关真开。
///
/// 那条测试的注释当时写「`was_running == false` 时 `around` 压根不调 `relaunch()`，
/// 所以能在真机上安全地跑」。**那句话只覆盖了一半**：`was_running` 是 `quit_and_wait()`
/// 的**返回结果**，不是入口条件 —— 机器上 ChatGPT 在跑时它就是 `true`，于是照样退、
/// 照样重开。而它「碰巧能过」还取决于退出时没弹确认框（弹了就是 `UserDeclined` ⇒
/// 主错误变成「ChatGPT 还在运行」⇒ 断言落空、测试红）。所以那条测试同时是
/// **有真实副作用**且**结果依赖开发者当下的桌面状态**。
///
/// 有了这个接缝，四个 quit 分支与「失败要不要重开」都能用假的效果函数测到位，
/// 一个 Apple event 都不发。**生产路径一个字没改** —— `around` 喂的就是真实那两个。
pub fn around_with<T>(
    abort_on_unconfirmed_exit: bool,
    quit: impl FnOnce() -> QuitOutcome,
    relaunch_app: impl Fn() -> Result<(), AppError>,
    action: impl FnOnce() -> Result<T, AppError>,
) -> Result<(T, AroundOutcome), AppError> {
    let mut outcome = AroundOutcome::default();

    match quit() {
        // 没装 / 本来没在跑：不需要重开（用户没开着，我们不该替他开）。
        QuitOutcome::NotRunning => {}
        QuitOutcome::Quit => outcome.was_running = true,
        // 本平台不需要先退出 ⇒ **无论调用方多严格都照常做**。
        // 中止它等于把用户锁在一个他做什么都过不去的错误里（见本函数文档）。
        QuitOutcome::NeedsManualRestart(why) => {
            // 只有 macOS 才中止：那边 ChatGPT 退出时**真的会**回写 `~/.codex`，
            // 没确认它退出就删 `auth.json` 等于白删。
            //
            // Windows 不中止（实测那边不回写：运行中不持句柄、退出后我们的改动仍在、
            // `auth.json` 整个启停周期 mtime 未变）—— 而且那边 `quit_and_wait` 走到
            // 这一支意味着 `taskkill /F` 都没杀掉，用户再手动关一次也未必成，
            // 中止只会把他锁在一个过不去的错误里。
            if abort_on_unconfirmed_exit && cfg!(target_os = "macos") {
                return Err(AppError::Config(format!(
                    "{why}。请先手动退出 ChatGPT，然后重试 —— 它退出时会重写 \
                     ~/.codex 里的文件，没退干净就白做了。配置未改动。"
                )));
            }
            outcome
                .warnings
                .push(format!("{why}，请手动重启 ChatGPT 让新配置生效。"));
        }
        QuitOutcome::UserDeclined => {
            return Err(AppError::Config(
                "ChatGPT 还在运行（它可能弹出了确认退出的对话框，或有进行中的对话）。\
                 请先手动退出它，然后重试。配置未改动。"
                    .into(),
            ));
        }
    }

    let value = match action() {
        Ok(v) => v,
        Err(e) => {
            // 我们关的就得开回来。恢复也失败时要说出来，但**主错误是 action 那条** ——
            // 别让恢复的错盖住用户真正需要知道的原因。
            if outcome.was_running {
                if let Err(re) = relaunch_app() {
                    return Err(AppError::Config(format!(
                        "{e}（重新打开 ChatGPT 也失败了：{re}，请手动打开它）"
                    )));
                }
                return Err(AppError::Config(format!("{e}（已重新打开 ChatGPT）")));
            }
            return Err(e);
        }
    };

    if outcome.was_running {
        match relaunch_app() {
            Ok(()) => outcome.relaunched = true,
            // 重开失败**不回滚**：事情已经办成了，用户手动打开 ChatGPT 就能用上。
            Err(e) => outcome.warnings.push(format!("重新打开 ChatGPT 失败：{e}")),
        }
    }

    Ok((value, outcome))
}

/// 切换分组前要不要先提示用户处理 ChatGPT。
///
/// 语义是**「这台机器上切换分组需要管 ChatGPT 吗」**。
///
/// - macOS：判据是 `is_running` 报不报 `-1728`（"不能获得 application id"，实测就是"没装"
///   的信号）。没装就不必打扰用户。**不要用 `path to application id`** —— 实测它会挂住 25
///   秒以上不返回。
/// - Windows：判据是 `Get-AppxPackage` 查不查得到那个包（结果有缓存，
///   见 [`chatgpt_aumid`] —— 那条查询单次 ~730ms，而本函数每次面板刷新都会被调到）。
/// - 其它平台：**恒为 true**。查不到装没装，但如果用户装了、又不提示他重启，他会拿着
///   旧分组跑而完全不知道。宁可对没装的用户多问一句（他点「只切换」就好），也不能让装了的
///   用户静默用错分组。
pub fn needs_user_attention() -> bool {
    #[cfg(target_os = "macos")]
    {
        // 能查到运行状态（无论 true/false）就说明 Launch Services 认得这个 bundle id。
        is_running().is_ok()
    }
    // Windows：查得到真实安装状态了（`Get-AppxPackage`），不再恒为 true。
    //
    // 判据用「装了没有」而不是「在不在跑」：没在跑也要问 —— 用户可能切换完才去开它，
    // 那时若不提示，他开起来的仍是旧配置（它只在启动时读）。
    #[cfg(target_os = "windows")]
    {
        chatgpt_aumid().is_ok()
    }
    // 其它平台查不到，宁可多问一句：装了却不提示，用户会拿着旧分组跑而完全不知道。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
    // Windows：**强制终止全部进程**，因为那边没有能让它优雅退出的手段
    // （`WM_CLOSE` 被吃掉、官方无 reload 接口、当不了父进程 —— 逐条见模块文档那张表）。
    //
    // ⚠️ **走到这里意味着用户已经在弹窗里点过「退出并切换」** —— `around` 的调用链上游
    // （`RelaySection` 的 `confirmSwitch`）先弹 `loongport.quitConfirm`，
    // 用户点「只切换，我自己重启」时压根不会调本函数。所以这里直接杀是知情同意的，
    // 不是我们替他决定。**别把这个前置弹窗去掉** —— 那会让强杀变成偷袭。
    #[cfg(target_os = "windows")]
    {
        let pids = match chatgpt_root_pids() {
            Ok(p) => p,
            // **查不到 ≠ 没在跑**：拿不到事实时不能假装是「没有」，那会让
            // `around` 认为本来就没开 ⇒ 切换后不重开、也不提示（见 `chatgpt_pids`）。
            // 归到「你自己关一下」是诚实的：配置照改，且用户会收到提示。
            Err(e) => {
                log::warn!("查 ChatGPT 进程失败: {e}");
                return QuitOutcome::NeedsManualRestart("没能确认 ChatGPT 的运行状态");
            }
        };
        if pids.is_empty() {
            return QuitOutcome::NotRunning;
        }

        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(force_kill_args(&pids));
        match cmd.output() {
            Ok(out) if !out.status.success() => {
                // 不当致命错：**判据仍是轮询**，与 macOS 同理 ——
                // 命令返回码在这条路上已经骗过我们一次了（见模块文档「假成功」那段）。
                log::debug!(
                    "taskkill 返回非零（继续轮询确认）: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Err(e) => {
                log::warn!("执行 taskkill 失败: {e}");
                return QuitOutcome::NeedsManualRestart("没能关闭 ChatGPT");
            }
            _ => {}
        }

        // **判据是进程真的没了，不是 taskkill 的返回码。**
        let deadline = std::time::Instant::now() + QUIT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(QUIT_POLL_INTERVAL);
            // ⚠️ 只认**明确查到空**才算退出 —— `Ok(p) if p.is_empty()`，不是
            // `chatgpt_pids().is_empty()`（那会把 `Err` 也当成「空 ⇒ 已退出」，
            // 正是上面刚修掉的那个坑换个地方复发）。
            // 轮询期间偶发查询失败不致命：下一轮再查，等满了自然落到超时那支。
            if matches!(chatgpt_pids(), Ok(ref p) if p.is_empty()) {
                return QuitOutcome::Quit;
            }
        }

        // 强制终止杀不掉，且**不可能是用户挡的**（`/F` 不给应用拒绝的机会）
        // ⇒ 不能像 macOS 那样解读成 `UserDeclined`。归到「你自己关一下」：
        // 配置照改（那边不回写，改了就是改了），只是得他自己重启。
        log::warn!("taskkill /F 之后 ChatGPT 进程仍在，放弃自动重启");
        QuitOutcome::NeedsManualRestart("没能关闭 ChatGPT")
    }

    // Linux：**`NotRunning` 是语义正确的**，不是占位 ——
    // ChatGPT 桌面版不发 Linux 版，那边它必然没装、必然没在跑。
    // 于是「不重开」（`NotRunning` 的语义）也正好对：没有东西可开。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        QuitOutcome::NotRunning
    }
}

/// Windows：当前全部 `ChatGPT.exe` 的 pid。空 = 没在跑。
///
/// 用 `tasklist /FO CSV /NH` 而不是 WMI/`windows-sys`：省一层 FFI，且 CSV 输出好解。
/// **不做本地化文本判断** —— 只取每行第二个 CSV 字段（pid），无匹配时 `tasklist`
/// 输出的是提示文本、解不出数字，自然得到空 vec（实测那台中文系统上 `chcp` 是 65001
/// 但 UI 语言 en-US，靠文案判断本来就不可靠）。
#[cfg(target_os = "windows")]
/// Windows：只取**进程树的根**（父进程不是 `ChatGPT.exe` 的那些）。
///
/// ## ⚠️ 为什么不能把全部 pid 都传给 `taskkill`（实测出来的）
///
/// `/T` 会先杀掉目标的子进程。所以若把 9 个 pid 全传进去，等 `taskkill` 走到
/// 那些子进程自己的 `/PID` 项时它们**已经被杀了**，于是：
///
/// ```text
/// taskkill /F /T /PID <主> /PID <子1> …
///   SUCCESS: … (child process of …) has been terminated.   ← 树被杀干净了
///   ERROR: The process "17040" not found.                  ← 但子进程那几项报错
///   exit code = 128                                        ← 整条命令算失败
/// ```
///
/// 这不是偶发竞态，而是**必然发生**（Electron 8 个子进程都会撞上）。后果是
/// 用户在一次**完全成功**的切换后看到「没能关闭 ChatGPT，请手动重启」这种假警告。
///
/// 只传根 pid 则 exit code 干净（实测 0），子进程照样由 `/T` 全部带走。
///
/// 判据是「父进程不在本批 `ChatGPT.exe` 里」而不是「命令行没有 `--type=`」——
/// 后者依赖 Electron 的内部约定（它改了参数名我们就认不出），
/// 前者只依赖进程树形状，更稳。
#[cfg(target_os = "windows")]
fn chatgpt_root_pids() -> Result<Vec<u32>, AppError> {
    let pids = chatgpt_pids()?;
    if pids.is_empty() {
        return Ok(pids);
    }
    // `tasklist` 不给父 pid，得用 WMI。拿不到就回落到「全传」——
    // 那只会让返回码变脏（我们本来就不信它），不影响杀进程的效果。
    let Ok(out) = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "Get-CimInstance Win32_Process -Filter \"Name='{CHATGPT_PROCESS_NAME}'\" | \
                 ForEach-Object {{ \"$($_.ProcessId),$($_.ParentProcessId)\" }}"
            ),
        ])
        .output()
    else {
        log::debug!("查父进程失败，回落到把全部 pid 传给 taskkill");
        return Ok(pids);
    };
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, ppid) = line.trim().split_once(',')?;
            Some((pid.parse().ok()?, ppid.parse().ok()?))
        })
        .collect();
    if pairs.is_empty() {
        log::debug!("父进程信息解析不出，回落到把全部 pid 传给 taskkill");
        return Ok(pids);
    }
    let roots = tree_roots(&pairs);
    // 一个根都认不出（比如进程树形状意外）⇒ 别返回空（那会被判成「没在跑」），
    // 回落到全传。返回码会脏，但我们本来就不信它。
    Ok(if roots.is_empty() { pids } else { roots })
}

/// `(pid, ppid)` 里哪些是**树根** —— 父进程不在这批里的那些。
///
/// **抽成纯函数只为了可测**：真实进程表里 ChatGPT 有 9 个进程，而测试能起的替身
/// 只有 2 层 —— 那种规模下「全传」恰好也不报错，于是**测不出退化**
/// （变异测试实证过）。喂构造数据才守得住。
#[cfg(target_os = "windows")]
fn tree_roots(pairs: &[(u32, u32)]) -> Vec<u32> {
    let own: std::collections::HashSet<u32> = pairs.iter().map(|(pid, _)| *pid).collect();
    pairs
        .iter()
        .filter(|(_, ppid)| !own.contains(ppid))
        .map(|(pid, _)| *pid)
        .collect()
}

/// Windows：`taskkill` 的完整参数。
///
/// **抽成函数只为了可测**：原来这几个 flag 直接拼在 `quit_and_wait` 里，
/// 而测试是自己另拼一遍参数去验证 —— 于是**改实现的 flag 测试不会红**
/// （变异测试实证：去掉 `/F` 和去掉 `/T` 两条变异都存活了）。
/// 那是假闸，跟本仓踩过的其它假闸同一个形态：测了「命令行长什么样」这件事的
/// **复制品**，而不是实现本身。
///
/// 两个 flag 都不可省：
/// - `/F` —— 不带它只发 `WM_CLOSE`，被 minimize-to-tray 吃掉且返回 exit 0 假成功
///   （见模块文档那段实测）。
/// - `/T` —— 连带子进程。Electron 有 9 个进程，只杀主进程会留下一堆孤儿 renderer。
///
/// 一次把全部 pid 传给同一条命令：逐个起进程会慢一个数量级，而且中途某个已随主进程
/// 消失时会报「找不到」噪声。
#[cfg(target_os = "windows")]
fn force_kill_args(pids: &[u32]) -> Vec<String> {
    let mut args = vec!["/F".to_string(), "/T".to_string()];
    for pid in pids {
        args.push("/PID".to_string());
        args.push(pid.to_string());
    }
    args
}

#[cfg(target_os = "windows")]
fn chatgpt_pids() -> Result<Vec<u32>, AppError> {
    // ⚠️ **「查不到」与「确实没有」必须分开**（review 抓出）。
    //
    // 原来这里把执行失败静默折叠成空 vec，后果不是「少杀一个进程」而是**静默错误**：
    // `quit_and_wait` 会据此返回 `NotRunning` ⇒ `around` 认为「本来就没开」⇒
    // **切换后不重开、也不给任何提示** ⇒ 用户看到「切换成功」，而 ChatGPT 还开着跑旧配置。
    //
    // 那正是本模块反复强调的那类坑（见模块文档「假成功」那段）：拿不到事实时，
    // 不能假装事实是「没有」。
    let out = std::process::Command::new("tasklist")
        .args([
            "/FI",
            &format!("IMAGENAME eq {CHATGPT_PROCESS_NAME}"),
            "/NH",
            "/FO",
            "CSV",
        ])
        .output()
        .map_err(|e| AppError::Config(format!("执行 tasklist 失败: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Config(format!(
            "tasklist 返回 {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // `"ChatGPT.exe","22396","Console","2","338,396 K"` —— 取第 2 个字段。
            //
            // 裸按逗号切是安全的：带逗号的只有末列的内存数（`338,396 K`），
            // 而 pid 在第 2 列；进程名含空格也无妨（空格不是分隔符）。已实测确认。
            //
            // **无匹配时 `tasklist` 输出的是一行提示文本**（且会被系统语言本地化），
            // 它解不出数字 ⇒ 自然被 `filter_map` 丢掉。所以这里不做任何文案判断。
            let pid = line.split(',').nth(1)?.trim().trim_matches('"');
            pid.parse::<u32>().ok()
        })
        .collect())
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
    // Windows：MSIX 应用**必须经 AUMID 激活**，不能直接跑包内的 exe
    // （`C:\Program Files\WindowsApps\` 默认拒绝访问，且那样起来的进程拿不到包身份）。
    //
    // `explorer.exe shell:AppsFolder\<AUMID>` 是标准做法，实测有效。
    // 副作用：真正 spawn 应用的是系统的激活器，所以**我们不会成为它的父进程**
    // （实测父进程是 `explorer.exe`）—— 这也意味着我们退出时不会连带杀掉它，
    // 那是好事（用户关 LoongPort 不该顺手关掉 ChatGPT）。
    #[cfg(target_os = "windows")]
    {
        let aumid = chatgpt_aumid()?;
        let out = std::process::Command::new("explorer.exe")
            .arg(format!("shell:AppsFolder\\{aumid}"))
            .output()
            .map_err(|e| AppError::Config(format!("启动 ChatGPT 失败: {e}")))?;
        // ⚠️ **`explorer.exe` 的退出码不可信**：它把请求转交激活器后就返回，
        // 实测常见返回 1 而应用照样起来了。所以这里不判返回码 ——
        // 判了会把成功的启动报成失败（`around` 会据此给用户一条假警告）。
        log::debug!(
            "已请求启动 ChatGPT（AUMID={aumid}，explorer 退出码 {:?}，该码不可信）",
            out.status.code()
        );
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(unsupported())
    }
}

/// Windows：算出 ChatGPT 的 AUMID（`<PackageFamilyName>!App`）。
///
/// **运行时用 `Get-AppxPackage` 查，绝不写死** —— `PackageFamilyName` 里那串
/// publisher hash（实测 `OpenAI.Codex_2p2nqsd0c76g0`）虽然不含版本号，
/// 但它是从签名证书派生的：OpenAI 换证书就会变。写死等于给自己埋一颗定时炸弹。
///
/// `!App` 这个 Application Id 取自包的 `AppxManifest.xml`（实测 `Id=App`）。
/// 它属于「进了对外契约的常量」，改的可能性极低，且真变了会立刻表现为「起不来」
/// （不是静默失效），所以不值得为它再多打一次 PowerShell。
/// ## ⚠️ 结果要缓存：这条查询很贵，而它每次面板刷新都会被调到
///
/// 实测 `powershell -Command (Get-AppxPackage …)` 单次 **~730ms**（三次取样
/// 767/706/724）。而 [`needs_user_attention`] 也用它判「装了没有」，
/// 那个又被 `relay_status` 调用 —— 后者是**同步** Tauri command，
/// 且前端在三处（面板、分组页、切换守卫）每次刷新都打一次
/// ⇒ 不缓存就是每次刷新阻塞 IPC 0.7 秒。
///
/// 用 `OnceLock` 缓存**成功**的结果：安装状态在一次会话里几乎不会变，
/// 而万一变了（用户中途装上 ChatGPT），重启 app 即可 —— 那比每次刷新卡 0.7 秒划算。
/// **失败不缓存**：那可能是 PowerShell 一次抖动，缓存下来会让「装了却一直说没装」。
#[cfg(target_os = "windows")]
fn chatgpt_aumid() -> Result<String, AppError> {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(hit) = CACHED.get() {
        return Ok(hit.clone());
    }

    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-AppxPackage {CHATGPT_PACKAGE_NAME}).PackageFamilyName"),
        ])
        .output()
        .map_err(|e| AppError::Config(format!("查 ChatGPT 安装信息失败: {e}")))?;

    let family = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if family.is_empty() {
        // 查不到 = 没装（或不是 MSIX 安装形态）。这不该是 panic，调用方会把它
        // 当成一条警告带给用户。**不进缓存** —— 用户可能随后才装上。
        return Err(AppError::Config(
            "没找到已安装的 ChatGPT 桌面版，请手动打开它".into(),
        ));
    }
    let aumid = format!("{family}!App");
    let _ = CACHED.set(aumid.clone());
    Ok(aumid)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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

    /// ⚠️ **`abort_on_unconfirmed_exit` 的两种取值必须给出不同行为** ——
    /// 这是 `around` 唯一让调用方拍板的参数，也是两个调用点的关键差异。
    ///
    /// `false`（切 provider，只写 `config.toml`）：`NeedsManualRestart` 照常做事 + 提示。
    /// `true`（删 `auth.json`）：必须中止 —— **在 macOS 上** ChatGPT 退出时会回写那个文件，
    /// 没确认它退出就动手等于白删。
    ///
    /// ⚠️ **中止只发生在 macOS**：那边 ChatGPT 退出时**真的会**回写 `~/.codex`。
    /// Windows 实测不回写（运行中不持句柄、退出后我们的改动仍在、`auth.json` mtime
    /// 整个启停周期未变）⇒ 那边即使没关掉也照常做，中止只会锁死用户。
    /// 所以实现里那个条件是 `abort_on_unconfirmed_exit && cfg!(target_os = "macos")`。
    ///
    /// 会红的改法：为了「两个调用点一致」把这个参数删掉、统一成其中一种行为；
    /// 或者把那个 `cfg!` 去掉，让 Windows 也跟着中止。
    #[test]
    fn abort_flag_decides_what_needs_manual_restart_means() {
        // 判据本身（`around` 里那个 match 的形状）——两种取值下同一个 outcome 的去向不同。
        // **必须连 `cfg!` 一起镜像**，否则这条测试与实现分叉后就成了假闸。
        fn aborts(abort_on_unconfirmed_exit: bool, o: &QuitOutcome) -> bool {
            match o {
                QuitOutcome::Quit | QuitOutcome::NotRunning => false,
                QuitOutcome::NeedsManualRestart(_) => {
                    abort_on_unconfirmed_exit && cfg!(target_os = "macos")
                }
                // 用户明确说「先别动」——**两种取值都中止**。
                QuitOutcome::UserDeclined => true,
            }
        }

        let needs_manual = QuitOutcome::NeedsManualRestart("权限被拒");

        assert!(
            !aborts(false, &needs_manual),
            "切 provider 时退不掉也该照常切 —— 配置写进 config.toml 就已经生效"
        );
        assert_eq!(
            aborts(true, &needs_manual),
            cfg!(target_os = "macos"),
            "要删 auth.json 时：macOS 必须中止（它退出会回写那个文件），\
             其它平台不能中止（实测不回写，中止只会锁死用户）"
        );

        // 这两条与 abort 标志无关：确认退出了就往下走。
        for flag in [true, false] {
            assert!(!aborts(flag, &QuitOutcome::Quit));
            assert!(!aborts(flag, &QuitOutcome::NotRunning));
            assert!(
                aborts(flag, &QuitOutcome::UserDeclined),
                "用户点了取消，两种情况都得中止"
            );
        }
    }

    /// 本来没在跑 ⇒ `around` 不碰 ChatGPT，action 的错误原样传出。
    ///
    /// ## 为什么这几条测 `around_with` 而不是 `around`
    ///
    /// 这条测试原来调的是 `around`，而那个函数直接调 `quit_and_wait()` ⇒
    /// **它会真的去退用户的 ChatGPT**。实测跑一次 `cargo test`，ChatGPT 的 pid
    /// 从 29330 变成 36016（真关真开）。当时的注释写「`was_running == false` 时
    /// 不调 `relaunch()`，所以真机上安全」—— 那句话把 `was_running` 当成了入口条件，
    /// 而它其实是 `quit_and_wait()` 的**返回结果**：机器上 ChatGPT 在跑时它就是 true。
    ///
    /// 而且结果依赖开发者当下的桌面状态：退出时弹了确认框就是 `UserDeclined`
    /// ⇒ 主错误变成「ChatGPT 还在运行」⇒ 断言落空。**单元测试不该有真实副作用，
    /// 更不该看运行环境的脸色。**
    ///
    /// 现在喂假的效果函数（见 [`around_with`] 的接缝说明），一个 Apple event 都不发。
    #[test]
    fn action_failure_propagates_and_leaves_a_never_running_app_alone() {
        let relaunched = std::cell::Cell::new(0);
        let result = around_with(
            false,
            || QuitOutcome::NotRunning,
            || {
                relaunched.set(relaunched.get() + 1);
                Ok(())
            },
            || -> Result<(), AppError> { Err(AppError::Config("action 自己失败了".into())) },
        );

        let err = result.expect_err("action 的错误必须原样传出去").to_string();
        assert!(
            err.contains("action 自己失败了"),
            "主错误必须是 action 那条，不能被恢复逻辑的话术盖住，实际：{err}"
        );
        assert_eq!(
            relaunched.get(),
            0,
            "本来没在跑就**不该替用户打开它** —— 我们没关，就不负责开"
        );
    }

    /// ⭐ 我们关掉了它 ⇒ action 失败时**必须开回去**，且主错误仍是 action 那条。
    ///
    /// 这一半原来测不到（要真的退掉 ChatGPT 才走得到），被列为「实机验证的范围」——
    /// 有了接缝就是一条普通单测。它守的是「关了不开回去」这个最难被发现的缺陷：
    /// 用户手上会是「ChatGPT 被关了、事情没办成、也没人告诉他现在是什么状态」。
    #[test]
    fn a_failed_action_reopens_the_app_we_closed() {
        let relaunched = std::cell::Cell::new(0);
        let result = around_with(
            false,
            || QuitOutcome::Quit, // 它在跑，我们关掉了
            || {
                relaunched.set(relaunched.get() + 1);
                Ok(())
            },
            || -> Result<(), AppError> { Err(AppError::Config("action 自己失败了".into())) },
        );

        let err = result.expect_err("action 失败要传出去").to_string();
        assert!(
            err.contains("action 自己失败了"),
            "主错误必须是 action 那条，实际：{err}"
        );
        assert!(
            err.contains("已重新打开"),
            "要告诉用户我们把它开回去了，实际：{err}"
        );
        assert_eq!(relaunched.get(), 1, "⭐ 我们关的就必须开回来");
    }

    /// 重开也失败时：两条原因都要说，但**主错误仍是 action 那条**。
    #[test]
    fn a_failed_relaunch_does_not_bury_the_actions_error() {
        let result = around_with(
            false,
            || QuitOutcome::Quit,
            || Err(AppError::Config("开不起来".into())),
            || -> Result<(), AppError> { Err(AppError::Config("action 自己失败了".into())) },
        );

        let err = result.expect_err("要报错").to_string();
        assert!(
            err.contains("action 自己失败了"),
            "用户真正需要知道的是 action 为什么失败，实际：{err}"
        );
        assert!(
            err.contains("开不起来"),
            "恢复失败也得说 —— 否则用户不知道 ChatGPT 还关着，实际：{err}"
        );
    }

    /// 成功路径：关过就要重开，并如实报告 `was_running` / `relaunched`。
    #[test]
    fn a_successful_action_reopens_and_reports_what_happened() {
        let (value, outcome) = around_with(
            false,
            || QuitOutcome::Quit,
            || Ok(()),
            || Ok::<_, AppError>(42),
        )
        .expect("成功路径不该报错");

        assert_eq!(value, 42, "action 的返回值要原样带出");
        assert!(outcome.was_running, "它本来在跑");
        assert!(outcome.relaunched, "关过就该开回去");
        assert!(
            outcome.warnings.is_empty(),
            "一切顺利时不该有 warning：{:?}",
            outcome.warnings
        );
    }

    /// 重开失败**不回滚**：事情已经办成了，只降级成 warning。
    #[test]
    fn a_failed_relaunch_after_success_is_only_a_warning() {
        let (value, outcome) = around_with(
            false,
            || QuitOutcome::Quit,
            || Err(AppError::Config("开不起来".into())),
            || Ok::<_, AppError>(7),
        )
        .expect("配置已经写进去了，不该因为重开失败而报错");

        assert_eq!(value, 7);
        assert!(
            outcome.warnings.iter().any(|w| w.contains("开不起来")),
            "要如实告诉用户「手动打开它」，实际：{:?}",
            outcome.warnings
        );
        assert!(!outcome.relaunched, "确实没开成");
    }

    /// 用户在 ChatGPT 的确认框点了取消 ⇒ **action 压根不执行**，配置不能动。
    ///
    /// 这条最重要：`UserDeclined` 意味着那个 app 还活着、并且它自己会回写
    /// `config.toml` —— 此时写配置的结果是两边互相覆盖，而用户既没切成也不知道。
    #[test]
    fn a_declined_quit_never_runs_the_action() {
        let ran = std::cell::Cell::new(false);
        let result = around_with(
            false,
            || QuitOutcome::UserDeclined,
            || Ok(()),
            || {
                ran.set(true);
                Ok::<_, AppError>(())
            },
        );

        let err = result.expect_err("用户点了取消，必须中止").to_string();
        assert!(
            err.contains("配置未改动"),
            "文案要明确告诉用户什么都没动，实际：{err}"
        );
        assert!(!ran.get(), "⭐ action 一次都不能执行");
    }

    /// `NeedsManualRestart`：照常做事 + 提示手动重启，**不中止**（`abort` 为 false 时）。
    ///
    /// 「退不掉」不该拦住一件本来做得成的事 —— 配置写进文件就已经生效了。
    #[test]
    fn an_unconfirmed_exit_still_does_the_work_and_warns() {
        let (value, outcome) = around_with(
            false,
            || QuitOutcome::NeedsManualRestart("退出 ChatGPT 时出错"),
            || Ok(()),
            || Ok::<_, AppError>(1),
        )
        .expect("退不掉也该照常切 —— 配置写进去就生效了");

        assert_eq!(value, 1);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("请手动重启 ChatGPT")),
            "必须提示用户重启，否则桌面版一直用旧配置：{:?}",
            outcome.warnings
        );
        assert!(
            !outcome.was_running,
            "没确认它退出 ⇒ 不算「我们关掉了」⇒ 不该去重开（那会变成替用户开一个他没开的 app）"
        );
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

    /// Windows：**没在跑时必须是 `NotRunning`，不能去杀也不能报错。**
    ///
    /// 这条能在 CI / 开发机上安全跑（那些机器上没有 ChatGPT ⇒ 走的就是这一支），
    /// 而且它顺带验证了 `chatgpt_pids()` 的解析：若那个函数把 `tasklist`
    /// 「无匹配」时的提示文本误解析成一个 pid，这里就会走到杀进程那一支、
    /// 拿到 `NeedsManualRestart` 而红。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reports_not_running_when_chatgpt_is_absent() {
        // 与端到端那条互斥：它会起一个名叫 ChatGPT.exe 的替身（见锁的说明）。
        let _guard = PROCESS_TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let pids = chatgpt_pids().expect("查进程不该失败");
        if !pids.is_empty() {
            // 真机上 ChatGPT 正开着 —— 这条测试**不能**跑（会杀掉用户的 app）。
            eprintln!("跳过：本机 ChatGPT 正在运行，不去动它");
            return;
        }
        assert_eq!(
            quit_and_wait(),
            QuitOutcome::NotRunning,
            "没在跑时必须直接 NotRunning"
        );
    }

    /// Windows 上这几条测试共享一份**全局状态**：系统进程表里有没有叫
    /// `ChatGPT.exe` 的进程。`cargo test` 默认并发跑 ⇒ 端到端那条起的替身会被
    /// 「没在跑时应当 NotRunning」那条看见，后者于是走了杀进程路径拿到 `Quit` 而红
    /// （实测撞过一次）。用一把锁串行化它们。
    ///
    /// 不给整个模块加锁，只给**真的会碰进程表**的那两条 —— 其余（参数拼装、
    /// 源码扫描）互不干扰，串行化它们只是白等。
    #[cfg(target_os = "windows")]
    static PROCESS_TABLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// ⭐ **端到端：让 `quit_and_wait` 真的去杀一个替身**，走完整实现路径。
    ///
    /// 这条是本模块最有价值的一道闸。review 用变异测试实证：在它之前，
    /// 每一条 Windows 测试都是「自己另拼一遍逻辑」或「扫源码字符串」，
    /// **没有一条真的调用生产代码路径** ⇒ 四条变异同时存活
    /// （去 `/F`、去 `/T`、只看返回码不轮询、`chatgpt_pids` 折叠错误为空）。
    ///
    /// 做法：把 `cmd.exe` 复制成 `ChatGPT.exe` 放进临时目录再起它 ——
    /// 于是 `tasklist /FI "IMAGENAME eq ChatGPT.exe"` 认得它，
    /// `quit_and_wait` 会把它当成真的 ChatGPT 杀掉。
    ///
    /// ⚠️ **本机真有 ChatGPT 在跑时必须跳过** —— 否则会杀掉维护者正在用的 app。
    #[cfg(target_os = "windows")]
    #[test]
    fn quit_and_wait_really_kills_a_decoy_named_like_chatgpt() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // 与 `windows_reports_not_running_when_chatgpt_is_absent` 互斥（见锁的说明）。
        let _guard = PROCESS_TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        if !chatgpt_pids().expect("查进程不该失败").is_empty() {
            eprintln!("跳过：本机 ChatGPT 正在运行，不去动它");
            return;
        }

        // 替身放在自己的临时目录里，名字必须**逐字**是 CHATGPT_PROCESS_NAME。
        let dir = std::env::temp_dir().join("loongport-chatgpt-decoy");
        let _ = std::fs::create_dir_all(&dir);
        let decoy = dir.join(CHATGPT_PROCESS_NAME);
        let cmd_exe = std::path::Path::new(&std::env::var("SystemRoot").unwrap_or_default())
            .join("System32")
            .join("cmd.exe");
        if std::fs::copy(&cmd_exe, &decoy).is_err() {
            eprintln!("跳过：复制替身失败（可能是杀软锁了临时目录）");
            return;
        }

        // 起两层：替身自己 + 它的 ping 子进程，这样也覆盖到 `/T`。
        let mut child = std::process::Command::new(&decoy)
            .args(["/C", "ping -n 60 127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("起替身失败");
        std::thread::sleep(Duration::from_millis(600));

        let seen = chatgpt_pids().expect("查进程不该失败");
        assert!(
            seen.contains(&child.id()),
            "替身应当被 tasklist 认成 {CHATGPT_PROCESS_NAME}（否则这条测试测不到东西），\
             查到的是 {seen:?}"
        );

        // ⭐ 走真实实现。
        let outcome = quit_and_wait();

        let _ = child.wait();
        let _ = std::fs::remove_file(&decoy);
        let _ = std::fs::remove_dir(&dir);

        assert_eq!(
            outcome,
            QuitOutcome::Quit,
            "quit_and_wait 应当确认替身已退出。拿到 {outcome:?} 说明实现有问题：\
             少 /F 杀不掉、少 /T 留孤儿、或者判据退化成看返回码"
        );
        assert!(
            chatgpt_pids().expect("查进程不该失败").is_empty(),
            "替身进程树应当被杀干净"
        );
    }

    /// **退出判据必须是「查到空」，不能是「查得动」** —— 守轮询那一行的形状。
    ///
    /// 变异测试实证：把判据改成 `chatgpt_pids().is_ok()`（即「进程表查得动就算退出」），
    /// 端到端那条测试**照样绿** —— 因为替身死得快，第一次轮询时它确实已经没了。
    /// 那个退化在真机上的后果是：ChatGPT **杀不掉**时（权限/句柄问题）也报
    /// `Quit` ⇒ `around` 认为关掉了 ⇒ 切换后去「重开」一个还开着的 app，
    /// 而用户以为一切正常，实际它还在跑旧配置。
    ///
    /// 时序类退化没法靠「起个进程看结果」稳定复现（那要求替身死得刚好够慢），
    /// 所以这条直接钉住源码里那一行的形状 —— 与本仓其它 include_str! 闸同一套路。
    #[cfg(target_os = "windows")]
    #[test]
    fn the_exit_check_requires_an_empty_list_not_merely_a_successful_query() {
        let src = include_str!("chatgpt_app.rs");
        let impl_region = src
            .split_once("\nmod tests {")
            .map(|(before, _)| before)
            .unwrap_or(src);
        let code: String = impl_region
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        assert!(
            code.contains("matches!(chatgpt_pids(), Ok(ref p) if p.is_empty())"),
            "轮询判据必须是「明确查到空」。写成 `.is_ok()` / `.is_empty()` 都会让\
             「杀不掉」被误报成「已退出」—— 那正是本模块反复强调的假成功。"
        );
    }

    /// **只传树根，不传全部 pid** —— 这条守的是 review 抓出的那个假警告缺陷。
    ///
    /// `/T` 会先杀子进程，所以把 9 个 pid 全传进去时，`taskkill` 走到子进程那几项时
    /// 它们**已经没了** ⇒ `ERROR: not found` + **exit 128**（实测），于是一次
    /// **完全成功**的切换会给用户报「没能关闭 ChatGPT，请手动重启」。
    /// 只传根则 exit 0（实测），子进程照样由 `/T` 带走。
    ///
    /// 这条用**构造出来的进程表**验证筛选逻辑，而不是起真进程 ——
    /// 端到端那条测试的替身只有 2 层，全传时 `taskkill` 恰好不报错、测不出这个退化
    /// （变异测试实证：把 `chatgpt_root_pids` 改成返回全部 pid，端到端那条照样绿）。
    #[cfg(target_os = "windows")]
    #[test]
    fn only_tree_roots_are_passed_to_taskkill() {
        // 模拟 ChatGPT 的真实形状：主进程 100 的父是 explorer（不在本批里），
        // 其余 8 个的父都是 100。
        let pairs: Vec<(u32, u32)> = vec![
            (100, 9), // 主进程，父是 explorer
            (101, 100),
            (102, 100),
            (103, 100),
            (104, 100),
            (105, 100),
            (106, 100),
            (107, 101), // 孙进程（crashpad 那种）
            (108, 101),
        ];
        let roots = tree_roots(&pairs);
        assert_eq!(
            roots,
            vec![100],
            "只该传主进程 —— 传全部会让 taskkill 因「子进程已被 /T 杀掉」报 exit 128，\
             进而给用户一条假警告"
        );

        // 多个独立树（用户开了两个实例）都要认出来。
        let two = tree_roots(&[(200, 9), (201, 200), (300, 9), (301, 300)]);
        assert_eq!(two, vec![200, 300], "多个根都要保留");

        // 认不出根时**不能返回空** —— 那会被上层判成「没在跑」。
        assert!(
            tree_roots(&[(400, 401), (401, 400)]).is_empty(),
            "环形（不该出现）时返回空，由调用方回落到全传"
        );
    }

    /// **`taskkill` 的两个 flag 都不可省** —— 这条守的是实现产出的参数本身。
    ///
    /// 变异测试实证：去掉 `/F` 或 `/T` 之前**一条测试都不会红**，因为那时
    /// `force_kill_args` 还没抽出来、测试是自己另拼一遍参数（假闸）。
    ///
    /// - 少 `/F`：只发 `WM_CLOSE` ⇒ 被 minimize-to-tray 吃掉、还返回 exit 0 假成功，
    ///   于是「切换成功但 ChatGPT 还开着跑旧配置」。
    /// - 少 `/T`：主进程死了、8 个子进程变孤儿，`chatgpt_pids` 仍查得到它们
    ///   ⇒ 轮询等满 5 秒 ⇒ 误报「没能关闭」。
    #[cfg(target_os = "windows")]
    #[test]
    fn force_kill_args_always_carry_both_flags() {
        let args = force_kill_args(&[123, 456]);
        assert!(
            args.contains(&"/F".to_string()),
            "少了 /F 就只是发 WM_CLOSE —— 实测对 ChatGPT 无效且返回 exit 0 假成功。实际：{args:?}"
        );
        assert!(
            args.contains(&"/T".to_string()),
            "少了 /T 会留下 8 个孤儿子进程，轮询永远等不到干净。实际：{args:?}"
        );
        // 每个 pid 都要带自己的 `/PID`。
        assert_eq!(
            args,
            vec!["/F", "/T", "/PID", "123", "/PID", "456"],
            "参数形状变了 —— taskkill 要求每个 pid 前都有一个 /PID"
        );
    }

    /// **`chatgpt_pids` 必须用 `Result` 区分「查不到」与「确实没有」**（review 抓出的缺陷）。
    ///
    /// 原来它返回 `Vec<u32>`，执行失败静默变成空 vec ⇒ `quit_and_wait` 返回
    /// `NotRunning` ⇒ `around` 认为「本来就没开」⇒ **切换后不重开、也不给任何提示**
    /// ⇒ 用户看到「切换成功」，而 ChatGPT 还开着跑旧配置。典型的静默错误。
    ///
    /// 这条闸守两件事：签名是 `Result`（类型层面就不允许再折叠），
    /// 以及**轮询处不许写 `chatgpt_pids().is_empty()`** —— 那会让 `Err` 又被当成
    /// 「空 ⇒ 已退出」，同一个坑换个地方复发。
    #[cfg(target_os = "windows")]
    #[test]
    fn a_failed_process_query_is_not_the_same_as_no_process() {
        // 类型层面：必须是 Result，不能是裸 Vec。
        let _: Result<Vec<u32>, AppError> = chatgpt_pids();

        // 源码层面：轮询那处必须只认 `Ok(空)`。
        let src = include_str!("chatgpt_app.rs");
        let impl_region = src
            .split_once("\nmod tests {")
            .map(|(before, _)| before)
            .unwrap_or(src);
        let code: String = impl_region
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        assert!(
            !code.contains("chatgpt_pids().is_empty()"),
            "别写 `chatgpt_pids().is_empty()` —— 那把 Err 也当成「已退出」。\
             要用 `matches!(chatgpt_pids(), Ok(ref p) if p.is_empty())`"
        );
    }

    /// Windows：`chatgpt_pids()` **不能把 `tasklist` 的「无匹配」提示当成 pid**。
    ///
    /// `tasklist /FI` 查不到时不是空输出，而是一行提示文本（且**会被系统语言本地化**）。
    /// 靠文案判断必然在别的语言环境下碎掉，所以实现是「只取第 2 个 CSV 字段并 parse 成
    /// 数字，解不出就丢掉」。这条用一个必然不存在的进程名验证那个策略。
    #[cfg(target_os = "windows")]
    #[test]
    fn tasklist_parsing_ignores_the_localized_no_match_message() {
        let out = std::process::Command::new("tasklist")
            .args([
                "/FI",
                "IMAGENAME eq loongport-no-such-process.exe",
                "/NH",
                "/FO",
                "CSV",
            ])
            .output()
            .expect("tasklist 必须可执行");
        let parsed: Vec<u32> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                line.split(',')
                    .nth(1)?
                    .trim()
                    .trim_matches('"')
                    .parse()
                    .ok()
            })
            .collect();
        assert!(
            parsed.is_empty(),
            "「无匹配」的输出不该解析出任何 pid，实际：{parsed:?}（\
             解析策略若退化成按文案判断，会在非英文系统上碎掉）"
        );
    }

    /// Windows：AUMID 必须**运行时算**，实现里不许出现写死的 publisher hash 或安装路径。
    ///
    /// 那串 hash（形如 `OpenAI.Codex_<13位>`）是从**签名证书**派生的 —— OpenAI 换证书就变；
    /// `PackageFullName` / `InstallLocation` 更是含版本号，每次更新就变，
    /// 而 `C:\Program Files\WindowsApps\` 默认还拒绝访问。
    ///
    /// ## 只扫 `mod tests` 之前的部分
    ///
    /// 「扫自己的源码」有自指问题：注释里要讲清这个坑就得引用那些字面量，
    /// 断言自己也带着它们 —— 第一版按「行首是 `//`」过滤注释，结果被
    /// **断言字符串自身**触发而误报（Windows 上实际红过一次）。
    ///
    /// 切在 `mod tests` 处最省事且判据清晰：**实现区域**里出现这些字面量才是真问题，
    /// 测试与文档里出现是必要的。
    #[cfg(target_os = "windows")]
    #[test]
    fn the_aumid_is_resolved_at_runtime_not_hardcoded() {
        let src = include_str!("chatgpt_app.rs");
        let impl_region = src
            .split_once("\nmod tests {")
            .map(|(before, _)| before)
            .unwrap_or(src);
        // 文档注释里会引用这些字面量来解释「为什么不能写死」，那是应该的。
        let code: String = impl_region
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();

        // 用 regex 之外的办法认那串 hash：包名后面直接跟下划线就是 family name 的形状。
        assert!(
            !code.contains("OpenAI.Codex_"),
            "别把 PackageFamilyName / FullName 写进实现 —— 那串 hash 随签名证书变，\
             要用 Get-AppxPackage 运行时查。实现区域里出现了它。"
        );
        assert!(
            !code.contains("WindowsApps"),
            "别碰 WindowsApps 安装路径 —— 含版本号且默认拒绝访问"
        );
        assert_eq!(
            CHATGPT_PACKAGE_NAME, "OpenAI.Codex",
            "包名是 OpenAI.Codex（显示名却是 ChatGPT、exe 又叫 ChatGPT.exe，三者不一致）"
        );
        assert_eq!(CHATGPT_PROCESS_NAME, "ChatGPT.exe");
    }

    /// Windows：**`taskkill /F /T` + 轮询这套组合真的能杀掉一个进程树。**
    ///
    /// 这条守的是实现的**核心机制**，而不只是参数拼法。用一个替身进程树
    /// （`cmd` 起一个子 `cmd`）而不是真的 ChatGPT —— 那会杀掉维护者正在用的 app。
    ///
    /// 为什么值得单独立：`taskkill` **不带 `/F` 时会返回 exit 0 却什么都没做**
    /// （实测在 ChatGPT 上就是这样，见模块文档）。这条同时验证两件事：
    /// 带 `/F` 确实生效，且「轮询确认」比「看返回码」可靠。
    #[cfg(target_os = "windows")]
    #[test]
    fn force_kill_with_tree_flag_actually_terminates_a_process_tree() {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：别在测试机上闪出黑窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // 替身：`cmd /C ping -n 60 127.0.0.1` —— 父 cmd 会**一直活着等子进程**，
        // 于是天然是个两层进程树，正好测 `/T`。
        //
        // 踩过的两个坑：
        // - `start /B ... & ...`：父 cmd 立刻返回 ⇒ taskkill 跑到时它已经没了
        //   （实测报 `ERROR: The process "12576" not found`，这条测试因此红过一次）。
        // - `timeout /T`：要控制台句柄，在 `CREATE_NO_WINDOW` 下直接失败。
        let mut parent = std::process::Command::new("cmd")
            .args(["/C", "ping -n 60 127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("起替身进程失败");
        let pid = parent.id();
        // 确认它真起来了再动手 —— 否则这条测的是「杀一个不存在的进程」，恒绿。
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            parent.try_wait().expect("try_wait 失败").is_none(),
            "替身进程应当还活着（它在 ping 60 次），否则这条测试测不到东西"
        );

        // ⭐ **用实现产出的参数，不是自己另拼一遍** —— 后者是假闸：
        // 变异测试实证过，从 `quit_and_wait` 里去掉 `/F` 或 `/T`，
        // 自己拼参数的测试**一条都不会红**。
        let out = std::process::Command::new("taskkill")
            .args(force_kill_args(&[pid]))
            .output()
            .expect("taskkill 必须可执行");
        assert!(
            out.status.success(),
            "taskkill /F /T 应当成功，stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // 回收僵尸，别给测试机留句柄。
        let _ = parent.wait();

        // 与实现同一套判据：**轮询进程真的没了**，不看返回码。
        let deadline = std::time::Instant::now() + QUIT_TIMEOUT;
        let mut gone = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(QUIT_POLL_INTERVAL);
            let probe = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .expect("tasklist 必须可执行");
            // 解析策略与 `chatgpt_pids` 一致：解得出 pid 才算还在。
            let still_there = String::from_utf8_lossy(&probe.stdout).lines().any(|line| {
                line.split(',')
                    .nth(1)
                    .and_then(|f| f.trim().trim_matches('"').parse::<u32>().ok())
                    == Some(pid)
            });
            if !still_there {
                gone = true;
                break;
            }
        }
        assert!(
            gone,
            "`/F` 之后进程应当在 {QUIT_TIMEOUT:?} 内消失 —— 若没有，\
             说明这套「强杀 + 轮询」的组合在本机不成立，Windows 实现的前提就没了"
        );
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
