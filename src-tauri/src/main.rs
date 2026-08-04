// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// `--mcp-image-gen --tier <provider_id>`：不开窗口，当生图 MCP server 跑。
///
/// ## ⚠️ 必须在 [`cc_switch_lib::run`] **之前**分流
///
/// `run()` 里挂了 `tauri_plugin_single_instance`：主程序已经在跑时，第二个实例会把
/// 参数转交给它、唤起主窗口然后自己退出。MCP server 走进去就等于**每次被 CLI 启动都
/// 只是把 LoongPort 窗口弹出来**，stdio 上一个字都不会说 —— 宿主那边看到的是启动超时。
///
/// 所以这里不碰 Tauri 的任何东西：自建 tokio runtime、直接读库、走 stdin/stdout。
///
/// 返回 `None` = 不是 MCP 模式，照常启动 GUI。
fn mcp_image_gen_tier() -> Option<String> {
    // 开关名从 lib 引 —— 写配置那侧（装工具时填进 args）用的是同一个常量。
    let flag = cc_switch_lib::IMAGEGEN_MCP_FLAG;
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == flag) {
        return None;
    }
    // `--tier <id>` 与 `--tier=<id>` 都接受：写配置的是我们自己，但用户可能手改。
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--tier=") {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
        if arg == "--tier" {
            if let Some(v) = iter.next() {
                // `!starts_with("--")` 是为了兑现上面那句「用户可能手改」——
                // `--tier --tier x` 不该把 `--tier` 自己当成档位 id
                // （那会走到「库里没有档位 --tier」这个莫名其妙的错误上）。
                if !v.is_empty() && !v.starts_with("--") {
                    return Some(v.clone());
                }
            }
        }
    }
    // 有 `--mcp-image-gen` 却没给档位 ⇒ 这是配置错误，不能静默退化成开 GUI
    // （用户会看到窗口莫名弹出，而真正的问题是配置少了一个参数）。
    // 用空串表达"要求了 MCP 模式但没说哪个档位"，由调用方报错退出。
    Some(String::new())
}

fn main() {
    // MCP 模式最先判：见 `mcp_image_gen_tier` 的文档（走进 run() 会被 single-instance 截走）。
    if let Some(provider_id) = mcp_image_gen_tier() {
        if provider_id.is_empty() {
            // stderr 而不是 stdout —— 后者是 MCP 协议通道，掺一句人话进去宿主会断连。
            eprintln!("--mcp-image-gen 需要 --tier <provider_id>");
            std::process::exit(2);
        }
        if let Err(e) = cc_switch_lib::run_imagegen_mcp(&provider_id) {
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
