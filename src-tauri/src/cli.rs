//! `loongport-cli --add-site`：一次性 CLI 配置，给无桌面 Linux（服务器、
//! Ubuntu 20.04 等跑不了 GUI 二进制的老发行版）用户的最简接入路径。
//!
//! ```text
//! loongport-cli --add-site https://example.com --key sk-xxx [--app codex] [--model gpt-5.4]
//! ```
//!
//! ## 定位（2026-09-06 立项时的边界，别滑成 daemon 产品线）
//!
//! 这是**配置写入器**，不是 LoongPort 的 headless 模式：探一次站点、把选定的
//! CLI 配置文件写好就退出。不进 LoongPort 自己的数据库（服务器和桌面是两台
//! 机器，互不影响），不做登录窗/多账号/档位/路由 —— 那些等真实服务器用户
//! 反馈再议。
//!
//! ## 打包形状（为什么是独立 bin 而不是 GUI 二进制的 flag）
//!
//! GUI 二进制在 Ubuntu 20.04 上**加载期**就挂（focal 源无 webkit2gtk-4.1、
//! glibc 2.35 符号墙），挂在 main.rs 的 flag 分流永远轮不到执行。所以这个
//! 入口做成独立 bin，`--no-default-features --target x86_64-unknown-linux-musl`
//! 静态构建，零动态依赖，任何 x86_64 Linux 都能跑。
//!
//! ## 复用面（几乎零新逻辑）
//!
//! - 协议探测：[`discovery::probe_site`]（纯 HTTP，GUI 同一条）
//! - base_url 约定：[`api::base_url_for`]（claude/gemini 拿站点根、codex/grok
//!   拿 `/v1` 根 —— 唯一数据源，别在这里重写）
//! - 配置生成：[`provision::settings_config_for`]（复用上游 deeplink 构造器，
//!   全部 CLI 一份形状）
//! - 落盘：[`write_live_snapshot`](crate::services::provider::write_live_snapshot)
//!   （GUI 切档走的同一批文件级写入函数）
//!
//! ## 异步边界
//!
//! 探测/拉模型是异步的，写盘是同步的（复用 GUI 的同步写入函数）。两个阶段
//! 在 [`run_add_site`] 里先后完成——不在异步上下文里调用写盘路径，避免
//! [`crate::rt::block_on`] 嵌套。

use std::io::{IsTerminal, Write};
use std::str::FromStr;

use crate::app_config::AppType;
use crate::provider::Provider;
use crate::relay::{api, discovery, provision};

/// 触发标志，与 `--add-site <域名>` 的值成对出现。
pub const ADD_SITE_FLAG: &str = "--add-site";

/// 写进各 CLI 配置的 provider id。固定值：CLI 场景是「一台服务器一条中转站」，
/// 重复跑 = 覆盖同一 id（last-write-wins），不会堆积残条目。
const CLI_PROVIDER_ID: &str = "loongport-relay";

/// CLI 入口：返回进程退出码。不初始化 Tauri / GTK / 日志（写路径里的 `log::*`
/// 在无 logger 时静默丢弃，真实错误都走 `Result` 返回）。
pub fn run_add_site() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("loongport-cli {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let options = match parse_args(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            eprintln!();
            eprintln!("{}", USAGE);
            return 2;
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("❌ 无法创建异步运行时: {error}");
            return 1;
        }
    };
    let prepared = match runtime.block_on(prepare(options)) {
        Ok(prepared) => prepared,
        Err(message) => {
            eprintln!("❌ {message}");
            return 1;
        }
    };
    match write_config(prepared) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("❌ {message}");
            1
        }
    }
}

// USAGE 里的 app 清单与 `cli_supported` 的穷尽 match 保持一致（新增 CLI 支持时
// 两边都要动；解析与拒绝行为有测试兜着，这里只是帮助文本）。
const USAGE: &str = "用法:
  loongport-cli --add-site <站点域名或完整网址> --key <sk-密钥> [--app <codex|claude|gemini|grok|opencode|openclaw|hermes>] [--model <模型id>]

说明:
  --app    要配置的 CLI，默认 codex
  --model  模型 id。缺省时交互选择（非交互环境会列出站点可用模型后要求带 --model 重跑）";

#[derive(Debug)]
struct AddSiteOptions {
    site: String,
    key: String,
    app: AppType,
    model: Option<String>,
}

/// 异步阶段的产出：探测与模型选择做完，剩下的全是同步写盘。
struct Prepared {
    site_origin: String,
    site_name: String,
    base_url: String,
    key: String,
    app: AppType,
    model: String,
}

fn parse_args(args: &[String]) -> Result<AddSiteOptions, String> {
    let mut site = None;
    let mut key = None;
    let mut app = None;
    let mut model = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            ADD_SITE_FLAG => site = Some(take_value(&mut iter, ADD_SITE_FLAG)?),
            "--key" => key = Some(take_value(&mut iter, "--key")?),
            "--app" => app = Some(take_value(&mut iter, "--app")?),
            "--model" => model = Some(take_value(&mut iter, "--model")?),
            other => return Err(format!("无法识别的参数: {other}")),
        }
    }

    let site = site.ok_or("缺少 --add-site <站点域名>")?;
    let key = key.ok_or("缺少 --key <sk-密钥>")?;
    let app_name = app.unwrap_or_else(|| "codex".to_string());
    let app = AppType::from_str(&app_name).map_err(|e| e.to_string())?;
    // 这两个没有「写一份 CLI 配置」的语义：desktop 应用本身就没有服务器用法，
    // 生图走 LoongPort 自带 MCP（读图形界面的库），都不属于本命令能服务的形状。
    match app {
        AppType::ClaudeDesktop => {
            return Err(
                "claude-desktop 是桌面应用，没有服务器用法；请用 claude（Claude Code）".into(),
            )
        }
        AppType::CodexImage => {
            return Err("codex-image（生图）依赖 LoongPort 图形界面的库，不支持 CLI 配置".into())
        }
        _ => {}
    }

    Ok(AddSiteOptions {
        site,
        key,
        app,
        model,
    })
}

fn take_value<'a, I>(iter: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = &'a String>,
{
    iter.next()
        .cloned()
        .ok_or_else(|| format!("{flag} 需要一个值"))
}

/// 异步阶段：探测站点协议、确定 base_url、选定模型。
async fn prepare(options: AddSiteOptions) -> Result<Prepared, String> {
    let site_origin =
        api::normalize_site_origin(&options.site).map_err(|e| format!("站点地址无法解析: {e}"))?;
    println!("探测站点 {site_origin} …");
    let detected = discovery::probe_site(&site_origin)
        .await
        .map_err(|error| format!("站点协议识别失败: {error}"))?;

    let site_name = if detected.site_name.trim().is_empty() {
        site_origin
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    } else {
        detected.site_name.clone()
    };
    let base_url = api::base_url_for(&options.app, &site_origin, &detected.api_base_url);

    let model = match options.model {
        Some(model) => model,
        None => pick_model(&base_url, &options.key).await?,
    };

    Ok(Prepared {
        site_origin,
        site_name,
        base_url,
        key: options.key,
        app: options.app,
        model,
    })
}

/// 同步阶段：生成配置并落盘（GUI 切档同批写入函数）。
fn write_config(prepared: Prepared) -> Result<(), String> {
    let Prepared {
        site_origin,
        site_name,
        base_url,
        key,
        app,
        model,
    } = prepared;

    let settings = provision::settings_config_for(&app, &key, &site_name, &base_url, &model)
        .ok_or_else(|| {
            format!(
                "无法为 {} 生成配置（该 CLI 暂不支持 CLI 接入）",
                app.as_str()
            )
        })?;

    let provider = Provider::with_id(
        CLI_PROVIDER_ID.to_string(),
        format!("{site_name}（LoongPort CLI）"),
        settings,
        Some(site_origin.clone()),
    );
    crate::services::provider::write_live_snapshot(&app, &provider)
        .map_err(|e| format!("写入配置失败: {e}"))?;

    println!();
    println!(
        "✅ 已为 {} 配置「{site_name}」（模型 {model}）",
        app.as_str()
    );
    println!("   密钥已写进 CLI 自己的配置文件，无需再设环境变量。");
    println!("   验证: {}", verify_hint(&app));
    println!("   （本命令只写该 CLI 的配置文件，不涉及 LoongPort 图形界面数据。）");
    Ok(())
}

/// `--model` 缺省时的模型选择：拉站点模型列表；TTY 交互选，非 TTY 列清单退出。
async fn pick_model(base_url: &str, key: &str) -> Result<String, String> {
    println!("未指定 --model，拉取站点模型列表 …");
    let models =
        crate::services::model_fetch::fetch_models(base_url, key, false, None, None, None, None)
            .await
            .map_err(|error| format!("拉取模型列表失败（可加 --model 显式指定绕过）: {error}"))?;
    if models.is_empty() {
        return Err("站点模型列表为空；请用 --model 显式指定".to_string());
    }

    if !std::io::stdin().is_terminal() {
        println!("站点可用模型（非交互环境，请带 --model <id> 重新运行）:");
        for model in &models {
            println!("  {}", model.id);
        }
        return Err("非交互环境无法选择模型".to_string());
    }

    println!("站点可用模型（输入序号，默认 1）:");
    for (index, model) in models.iter().enumerate() {
        println!("  {:>3}. {}", index + 1, model.id);
    }
    print!("选择 [1]: ");
    std::io::stdout().flush().ok();

    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    let choice: usize = line.trim().parse().unwrap_or(1);
    models
        .get(choice.saturating_sub(1))
        .map(|model| model.id.clone())
        .ok_or_else(|| format!("序号超出范围（1-{}）", models.len()))
}

fn verify_hint(app: &AppType) -> &'static str {
    match app {
        AppType::Codex => "codex exec \"回复一个字：好\"",
        AppType::Claude => "claude -p \"回复一个字：好\"",
        _ => "直接运行该 CLI 发一条消息",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(list: &[&str]) -> Result<AddSiteOptions, String> {
        let owned: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        parse_args(&owned)
    }

    #[test]
    fn parses_minimal_with_codex_default() {
        let options = arg(&["--add-site", "https://example.com", "--key", "sk-x"])
            .expect("minimal args parse");
        assert_eq!(options.site, "https://example.com");
        assert_eq!(options.key, "sk-x");
        assert!(matches!(options.app, AppType::Codex));
        assert!(options.model.is_none());
    }

    #[test]
    fn parses_all_flags_and_app_aliases() {
        let options = arg(&[
            "--add-site",
            "example.com",
            "--key",
            "sk-x",
            "--app",
            "grok",
            "--model",
            "grok-5",
        ])
        .expect("full args parse");
        assert!(matches!(options.app, AppType::GrokBuild));
        assert_eq!(options.model.as_deref(), Some("grok-5"));
    }

    #[test]
    fn rejects_missing_key_or_site() {
        assert!(arg(&["--add-site", "https://example.com"]).is_err());
        assert!(arg(&["--key", "sk-x"]).is_err());
    }

    #[test]
    fn rejects_valueless_flag_and_unknown_flag() {
        assert!(arg(&["--add-site", "--key", "sk-x"]).is_err());
        assert!(arg(&[
            "--add-site",
            "https://example.com",
            "--key",
            "sk-x",
            "--wat"
        ])
        .is_err());
    }

    #[test]
    fn rejects_apps_without_cli_shape() {
        let desktop = arg(&[
            "--add-site",
            "https://example.com",
            "--key",
            "sk-x",
            "--app",
            "claude-desktop",
        ])
        .unwrap_err();
        assert!(desktop.contains("桌面应用"), "{desktop}");
        let image = arg(&[
            "--add-site",
            "https://example.com",
            "--key",
            "sk-x",
            "--app",
            "codex-image",
        ])
        .unwrap_err();
        assert!(image.contains("生图"), "{image}");
    }
}
