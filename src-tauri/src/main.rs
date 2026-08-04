// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// `--mcp-image-gen`：不开窗口，当生图 MCP server 跑。
///
/// ## ⚠️ 必须在 [`cc_switch_lib::run`] **之前**分流
///
/// `run()` 里挂了 `tauri_plugin_single_instance`：主程序已经在跑时，第二个实例会把
/// 参数转交给它、唤起主窗口然后自己退出。MCP server 走进去就等于**每次被 CLI 启动都
/// 只是把 LoongPort 窗口弹出来**，stdio 上一个字都不会说 —— 宿主那边看到的是启动超时。
///
/// 所以这里不碰 Tauri 的任何东西：自建 tokio runtime、直接读库、走 stdin/stdout。
///
/// ## 为什么不带「用哪个档位」这个参数
///
/// 用哪个档位生图存在库里（`imagegen_mcp::CURRENT_IMAGE_TIER_KEY`），由 MCP 进程
/// **每次生图时现读**。写进命令行参数的话，用户每换一次生图档位都会改到 CLI 的配置
/// 文件，而 codex 只在启动时读它 ⇒ 必须新开终端才生效。
fn is_imagegen_mcp_mode() -> bool {
    let flag = cc_switch_lib::IMAGEGEN_MCP_FLAG;
    std::env::args().any(|a| a == flag)
}

fn main() {
    // MCP 模式最先判：见 `is_imagegen_mcp_mode` 的文档（走进 run() 会被 single-instance 截走）。
    if is_imagegen_mcp_mode() {
        if let Err(e) = cc_switch_lib::run_imagegen_mcp() {
            // stderr 而不是 stdout —— 后者是 MCP 协议通道，掺一句人话进去宿主会断连。
            eprintln!("生图 MCP 启动失败: {e}");
            std::process::exit(1);
        }
        return;
    }

    // 在 Linux 上设置 WebKit 环境变量以解决 DMA-BUF 渲染问题
    // 某些 Linux 系统（如 Debian 13.2、Nvidia GPU）上 WebKitGTK 的 DMA-BUF 渲染器可能导致白屏/黑屏
    // 参考: https://github.com/tauri-apps/tauri/issues/9394
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // 禁用 WebKitGTK 合成模式，规避 resize 时 webview 崩溃以及部分 Wayland
        // 合成器下的 surface 协商问题（整窗 UI 点击无响应、必须最大化-还原才能恢复）。
        // 参考: https://github.com/tauri-apps/tauri/issues/9394
        if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }

        // AppImage 的 GTK 启动钩子 (linuxdeploy-plugin-gtk.sh) 会无条件
        // `export GDK_BACKEND=x11` 强制走 XWayland，以规避历史上的 Wayland 崩溃
        // (tauri-apps/tauri#8541)。但在较新的 Wayland + NVIDIA 环境下，强制 XWayland
        // 反而使 WebKitGTK 的 webview 收不到指针事件（标题栏可点、网页内容点不动），
        // resize 后黑屏；改回原生 Wayland 即可解决，且该崩溃在 WebKitGTK 2.52 上已不复现。
        // 由于该钩子会覆盖用户预设的 GDK_BACKEND，这里提供一个钩子不会触碰的逃生开关：
        // 设置 CC_SWITCH_GDK_BACKEND=wayland 即可强制覆盖，默认行为保持不变（零回归）。
        if let Ok(backend) = std::env::var("CC_SWITCH_GDK_BACKEND") {
            if !backend.is_empty() {
                std::env::set_var("GDK_BACKEND", backend);
            }
        }
    }

    cc_switch_lib::run();
}
