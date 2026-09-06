//! `--add-site`：一次性 CLI 配置，给无桌面 Linux（服务器）用户的最简接入路径。
//!
//! ```text
//! LoongPort --add-site https://example.com --key sk-xxx [--app codex] [--model gpt-5.4]
//! ```
//!
//! ## 定位（2026-09-06 立项时的边界，别滑成 daemon 产品线）
//!
//! 这是**配置写入器**，不是 LoongPort 的 headless 模式：探一次站点、把选定的
//! CLI 配置文件写好就退出。不进 LoongPort 自己的数据库（服务器和桌面是两台
//! 机器，互不影响），不做登录窗/多账号/档位/路由 —— 那些等真实服务器用户
//! 反馈再议。
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
//! ## 分流时机的约束（同 `--mcp-image-gen`）
//!
//! 必须在 [`crate::run`] **之前**分流：`run()` 挂了 single-instance，GUI 已在跑
//! 时第二个实例的参数会被转交给它并弹窗。服务器上没有 GUI，但「装着桌面版
//! 的机器上跑 CLI」同样会被截走 —— 所以这里不碰 Tauri 的任何东西。

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

pub fn is_add_site_mode() -> bool {
    std::env::args().any(|a| a == ADD_SITE_FLAG)
}

/// CLI 入口：返回进程退出码。不初始化 Tauri / GTK / 日志（写路径里的 `log::*`
/// 在无 logger 时静默丢弃，真实错误都走 `Result` 返回）。
pub fn run_add_site() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    match runtime.block_on(add_site(options)) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("❌ {message}");
            1
        }
    }
}

const USAGE: &str = "用法:
  LoongPort --add-site <站点域名或完整网址> --key <sk-密钥> [--app <codex|claude|gemini|grok|opencode|openclaw|hermes>] [--model <模型id>]

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

async fn add_site(options: AddSiteOptions) -> Result<(), String> {
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

    let settings =
        provision::settings_config_for(&options.app, &options.key, &site_name, &base_url, &model)
            .ok_or_else(|| {
            format!(
                "无法为 {} 生成配置（该 CLI 暂不支持 CLI 接入）",
                options.app.as_str()
            )
        })?;

    let provider = Provider::with_id(
        CLI_PROVIDER_ID.to_string(),
        format!("{site_name}（LoongPort CLI）"),
        settings,
        Some(site_origin.clone()),
    );
    crate::services::provider::write_live_snapshot(&options.app, &provider)
        .map_err(|e| format!("写入配置失败: {e}"))?;

    println!();
    println!(
        "✅ 已为 {} 配置「{site_name}」（模型 {model}）",
        options.app.as_str()
    );
    println!("   密钥已写进 CLI 自己的配置文件，无需再设环境变量。");
    println!("   验证: {}", verify_hint(&options.app));
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
