//! 生图 MCP server：让 codex / claude 等 CLI 在**对话里**生图，用的是 LoongPort 已经
//! 备好的运营商档位。
//!
//! # 为什么要有它（而不是让用户把档位的 `model` 改成 `gpt-image-2`）
//!
//! sub2api 上有两条生图链路，**它们要求上游提供的模型不同**：
//!
//! | 链路 | 端点 | 上游要能提供 |
//! |---|---|---|
//! | codex 主模型设成 `gpt-image-2` | `/v1/responses` | **`gpt-5.4-mini`**（见下） |
//! | 本模块 | `/v1/images/generations` | `gpt-image-2` 本身 |
//!
//! 第一条那个反直觉的要求来自上游的归一化：`normalizeOpenAIResponsesImageOnlyModel`
//! 会把 image-only 主模型的请求改写成「文本主模型 + `image_generation` tool」的形状，
//! 而它写死的那个文本主模型是 `gpt-5.4-mini`（sub2api `service/openai_images.go` 的
//! `openAIImagesResponsesMainModel`）。⇒ 上游只挂了生图模型的中转站上，第一条**必然
//! 502**（实测鑫旺 Neko API 的两个生图分组：`sync-models` 问上游只回 `gpt-image-2`）。
//!
//! 而第二条在同一个档位上实测 200 出图。⇒ 走这条。
//!
//! 附带的好处比"能用"更重要：**用户的对话档位不必让位**。第一条路要求把 provider 的
//! `model` 改成生图模型，那个档位就没法对话了；本模块是独立工具，用户照旧用便宜的
//! 文本档位聊天，要图的时候顺手出图。
//!
//! # 为什么是「主程序加子命令」而不是独立 sidecar / Node 脚本
//!
//! MCP server 必须是个能被 CLI 启动的可执行体。三个候选里这个代价最小：
//!
//! | 做法 | 分发代价 |
//! |---|---|
//! | Node 脚本 | 要求用户机器有 node；若用 `sharp` 之类还得分平台带 native 二进制 |
//! | 独立 Rust sidecar | 每平台多一份二进制，macOS 上要多签名 + 公证一个 |
//! | **本模块** | **零新增**：已经签好的那个二进制自己就是 server |
//!
//! 所以入口是 `LoongPort --mcp-image-gen --tier <provider_id>`，
//! 在 [`crate::run`] **之前**分流（见那里的说明：走进去会被 single-instance 插件
//! 当成第二个实例而唤起主窗口）。
//!
//! # 为什么 sk 不写进 CLI 的配置文件
//!
//! codex 的 `[mcp_servers.*]` 支持 `env`，把 sk 塞进去最省事 —— 但那样 sk 会以明文
//! 落在 `~/.codex/config.toml` 里，并且**档位刷新换了 sk 之后就失效**（用户看到的是
//! 生图突然 401，而配置文件看起来一切正常）。
//!
//! 所以配置里只写 `--tier <provider_id>`，sk 在**每次启动时**从
//! `~/.loongport/loongport.db` 现读。provision 换了 sk 下次生图自动是新的，不需要
//! 任何同步逻辑 —— 这也是为什么这件事只有 LoongPort 做得漂亮：库在我们手里。

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

/// 走哪个模型生图。
///
/// **不是常量而是从档位配置里读**：档位的 `model` 已经由 provision 写成了该分组真实的
/// `gpt-image-*`（见 [`super::provision::pick_model`]），运营商上 `gpt-image-3` 那天
/// 自动跟上。读不出来时才回落到这个值。
const FALLBACK_IMAGE_MODEL: &str = "gpt-image-2";

/// 出图默认尺寸。
///
/// `gpt-image-2` 支持 `auto` 与任意合法 `WIDTHxHEIGHT`，但**不写 `auto`**：实测同一个
/// 请求给 `1024x1024` 出的是 1254×1254（上游自己会调），而给 `auto` 时行为更不可预期。
/// 给一个明确值让"用户没说尺寸"这件事有确定的含义。
const DEFAULT_SIZE: &str = "1024x1024";

/// MCP 协议版本。跟着 codex-cli 0.146 实际发的那个走。
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 这次运行绑定的档位。
struct Tier {
    /// 明文 sk。
    api_key: String,
    /// 形如 `https://api.example.com/v1`（**末尾无斜杠**，见 [`images_url`]）。
    base_url: String,
    /// 生图模型名，取自档位配置里的 `model`。
    model: String,
    /// 档位显示名，只用于日志与 `image_service_status`。
    display_name: String,
}

/// 从 LoongPort 库里读出某个档位的 sk / base_url / model。
///
/// ## 为什么直接读 sqlite 而不复用 `ProviderService`
///
/// 那一层要 `AppState`（Tauri 托管的状态），而这个进程**没有 Tauri app** —— 它在
/// `run()` 之前就分流走了。为一个只读三个字段的场景把 Tauri 运行时拉起来是本末倒置。
///
/// 代价是这里对 `providers` 表的形状有了第二处依赖。可接受：读的是 `id` /
/// `settings_config` 这两个最稳定的列（`settings_config` 的结构还共用
/// [`super::provision::extract_api_key`]，没有另写一份解析）。
fn load_tier(provider_id: &str) -> Result<Tier, String> {
    let db_path: PathBuf = crate::config::get_app_config_dir().join(crate::config::DB_FILE_NAME);
    if !db_path.exists() {
        return Err(format!(
            "找不到 LoongPort 数据库（{}）。请先启动 LoongPort 并登录运营商。",
            db_path.display()
        ));
    }

    // 只读打开：这个进程与主程序可能同时在跑，绝不能拿写锁。
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开数据库失败: {e}"))?;

    let (name, settings_raw): (String, String) = conn
        .query_row(
            "SELECT name, settings_config FROM providers WHERE id = ?1",
            [provider_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => format!(
                "库里没有档位 {provider_id}。它可能已被删除 —— \
                 请在 LoongPort 里重新点「装生图工具」。"
            ),
            other => format!("读取档位失败: {other}"),
        })?;

    let settings: Value =
        serde_json::from_str(&settings_raw).map_err(|e| format!("档位配置解析失败: {e}"))?;

    // sk 的位置按 CLI 分派，复用那一处定义 —— 硬编码 `auth.OPENAI_API_KEY` 会让将来
    // 挂到 claude 档位上时静默取不到（那个在 `env.ANTHROPIC_AUTH_TOKEN`）。
    let api_key = super::provision::extract_api_key(&settings, &crate::app_config::AppType::Codex)
        .ok_or_else(|| {
            format!(
                "档位「{name}」的配置里读不出密钥。\
                 请在 LoongPort 里对它点「获取密钥」重新生成。"
            )
        })?;

    let config_toml = settings
        .get("config")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("档位「{name}」的配置里没有 config.toml 内容"))?;

    let base_url = extract_toml_string(config_toml, "base_url")
        .ok_or_else(|| format!("档位「{name}」的配置里没有 base_url"))?;
    // 读不出 model 不是错误：老档位（本功能上线前 provision 的）可能没有生图模型名，
    // 回落到默认值让它仍然能用。
    let model = extract_toml_string(config_toml, "model").unwrap_or_else(|| {
        log::warn!("档位「{name}」读不出 model，生图回落 {FALLBACK_IMAGE_MODEL}");
        FALLBACK_IMAGE_MODEL.to_string()
    });

    Ok(Tier {
        api_key,
        base_url: base_url.trim_end_matches('/').to_string(),
        model,
        display_name: name,
    })
}

/// 从 config.toml 文本里抠一个顶层或表内的 `key = "value"`。
///
/// **不引 toml 解析器**：这个进程要尽量轻，而要读的两个键都是 `key = "值"` 这种最简
/// 形状（由 [`super::provision::codex_config_toml`] 生成，形状我们自己定的）。
///
/// ⚠️ 取**第一个**匹配。`base_url` 在生成的配置里只出现一次；`model` 则要小心 ——
/// `model_provider` / `model_reasoning_effort` 都以 `model` 开头，所以必须匹配到
/// 等号前的完整键名（下面 `split_once('=')` + `trim` 后严格相等）。
fn extract_toml_string(toml_text: &str, key: &str) -> Option<String> {
    for line in toml_text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        if lhs.trim() != key {
            continue;
        }
        let value = rhs.trim();
        // 只认双引号字符串（生成器只产出这种）。
        let unquoted = value.strip_prefix('"')?.strip_suffix('"')?;
        if unquoted.is_empty() {
            return None;
        }
        return Some(unquoted.to_string());
    }
    None
}

/// 生图端点的完整 URL。
///
/// `base_url` 已经带 `/v1`（[`super::api::codex_base_url`] 保证），所以这里只接
/// `/images/generations`。
fn images_url(base_url: &str) -> String {
    format!("{base_url}/images/generations")
}

/// 出的图存哪。
///
/// 放 `~/.loongport/generated_images/`：与数据库同目录，用户找得到，也不会污染他当前
/// 的工作目录（Agent 常在用户仓库里跑，往那里丢文件会进 git status）。
fn output_dir() -> PathBuf {
    crate::config::get_app_config_dir().join("generated_images")
}

/// 调一次生图，返回落盘后的文件路径。
async fn generate_image(
    tier: &Tier,
    prompt: &str,
    size: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let client = reqwest::Client::builder()
        // 生图慢（实测 30-90s），默认超时会在出图前就断。
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败: {e}"))?;

    let body = json!({
        "model": tier.model,
        "prompt": prompt,
        "n": 1,
        "size": size.unwrap_or(DEFAULT_SIZE),
    });

    let resp = client
        .post(images_url(&tier.base_url))
        .bearer_auth(&tier.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求生图接口失败: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取生图响应失败: {e}"))?;

    if !status.is_success() {
        // 把服务端的错误原文带给用户 —— 生图失败的原因几乎全在服务端
        // （余额不足、分组不允许生图、上游没挂这个模型），自己编一句会掩盖它。
        return Err(format!(
            "生图失败（HTTP {}）：{}",
            status.as_u16(),
            first_line(&text)
        ));
    }

    let parsed: Value =
        serde_json::from_str(&text).map_err(|e| format!("生图响应解析失败: {e}"))?;
    let items = parsed
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "生图响应里没有 data 数组".to_string())?;

    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建输出目录失败: {e}"))?;

    let mut saved = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let b64 = item
            .get("b64_json")
            .and_then(Value::as_str)
            .ok_or_else(|| "生图响应的 data 项里没有 b64_json".to_string())?;
        let bytes = base64_decode(b64)?;

        // 文件名带序号与随机后缀 —— **不带时间戳**：这个进程里拿不到「当前时间」的
        // 稳定来源不是问题，但同一秒内多张图会互相覆盖。用内容哈希则天然唯一且可复现。
        let name = format!("gpt-image-{}-{idx}.png", short_hash(&bytes));
        let path = dir.join(name);
        std::fs::write(&path, &bytes).map_err(|e| format!("写图片文件失败: {e}"))?;
        saved.push(path);
    }

    if saved.is_empty() {
        return Err("生图接口没有返回任何图片".into());
    }
    Ok(saved)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("图片 base64 解码失败: {e}"))
}

/// 内容哈希的前 12 位 hex，用作文件名。
fn short_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())[..12].to_string()
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(300).collect()
}

/// 本 server 暴露的工具清单。
///
/// **只有一个工具**：需求是「在对话里生图」。编辑图 / 批量 / 透明背景那些等有人真要
/// 再加 —— 每个工具都要写 schema、要在 prompt 里占位置，先把一件事做对。
fn tools_list() -> Value {
    json!([{
        "name": "generate_image",
        "description": "用 LoongPort 绑定的运营商档位生成图片（gpt-image 系列模型）。\
                        返回保存到本地的 PNG 文件路径。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "要生成的图片的描述。用英文写通常效果更好。"
                },
                "size": {
                    "type": "string",
                    "description": "图片尺寸，形如 1024x1024 或 1536x1024。省略则用 1024x1024。\
                                    注意上游可能返回与请求不同的实际尺寸。"
                }
            },
            "required": ["prompt"]
        }
    }])
}

/// 处理一条 JSON-RPC 请求，返回要写回去的响应（`None` = 这是个通知，不必回）。
async fn handle_request(tier: &Tier, req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    // 通知（没有 id）不需要响应。`notifications/initialized` 就是这种。
    let id = req.get("id")?.clone();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "loongport-imagegen", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(json!({ "tools": tools_list() })),
        "tools/call" => handle_tool_call(tier, req).await,
        // ping 是协议里的保活，必须答。
        "ping" => Ok(json!({})),
        other => Err(format!("不支持的方法: {other}")),
    };

    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        // 工具执行失败走 `result.isError` 而不是 JSON-RPC 的 `error` —— 那是协议层
        // 错误（方法不存在之类），而"生图失败"是业务结果，宿主要把它当文本给模型看。
        Err(msg) => json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "isError": true, "content": [{ "type": "text", "text": msg }] }
        }),
    })
}

async fn handle_tool_call(tier: &Tier, req: &Value) -> Result<Value, String> {
    let params = req.get("params").ok_or("tools/call 缺 params")?;
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != "generate_image" {
        return Err(format!("没有这个工具: {name}"));
    }
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("generate_image 需要非空的 prompt")?;
    let size = args.get("size").and_then(Value::as_str);

    let paths = generate_image(tier, prompt, size).await?;
    let list = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!(
                "已生成 {} 张图片（档位：{}，模型：{}）：\n{list}",
                paths.len(), tier.display_name, tier.model
            )
        }]
    }))
}

/// MCP server 主循环：stdin 读一行一条 JSON-RPC，stdout 写一行一条响应。
///
/// ⚠️ **stdout 只许写协议消息** —— 宿主按行解析 JSON，掺一句日志进去它就断连。
/// 所以本模块所有诊断信息走 `log`（落文件）或 stderr，绝不 `println!`。
pub fn serve(provider_id: &str) -> Result<(), String> {
    let tier = load_tier(provider_id)?;
    log::info!(
        "生图 MCP 启动：档位「{}」，模型 {}，端点 {}",
        tier.display_name,
        tier.model,
        images_url(&tier.base_url)
    );

    // 自建 runtime：这个进程没走 Tauri，没有现成的 async 环境。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("创建 async runtime 失败: {e}"))?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("读取 stdin 失败: {e}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                // 解析不了就跳过 —— 宿主发了坏消息不该让 server 死掉。
                log::warn!("收到无法解析的消息（已跳过）: {e}");
                continue;
            }
        };
        if let Some(resp) = runtime.block_on(handle_request(&tier, &req)) {
            let mut out =
                serde_json::to_string(&resp).map_err(|e| format!("序列化响应失败: {e}"))?;
            out.push('\n');
            stdout
                .write_all(out.as_bytes())
                .map_err(|e| format!("写 stdout 失败: {e}"))?;
            stdout.flush().map_err(|e| format!("flush 失败: {e}"))?;
        }
    }
    log::info!("生图 MCP 退出（stdin 关闭）");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ **这个 crate 的 logger 写 stdout，而 stdout 是 MCP 的协议通道。**
    ///
    /// 当前安全**只是因为 logger 在 [`crate::run`] 里才初始化**
    /// （`tauri_plugin_log` 带 `TargetKind::Stdout`），而 MCP 模式在 `run()` 之前就
    /// 分流走了 ⇒ 这个进程里根本没有 logger，`log::` 全是空操作。
    ///
    /// 但那是**很脆的安全**：谁把日志初始化提到 `main()` 开头（很自然的想法：
    /// 「让启动早期的问题也能记下来」），MCP 就会往协议通道里吐日志行 ⇒
    /// 宿主解析不了那一行 ⇒ **断连**。而症状是「codex 里生图工具时好时坏」，
    /// 没有任何东西会报错。
    ///
    /// 这道闸盯的是那个前提：`lib.rs` 里的 stdout target 必须仍然在 `run()` 内部。
    /// 它红了说明**要么**把那个 target 去掉、**要么**在 MCP 模式下显式装一个
    /// 只写文件的 logger，别只是把断言改绿。
    #[test]
    fn the_stdout_logger_must_stay_inside_run_or_mcp_breaks() {
        let lib_rs = include_str!("../lib.rs");
        let stdout_target = "Target::new(TargetKind::Stdout)";
        assert!(
            lib_rs.contains(stdout_target),
            "lib.rs 里找不到 {stdout_target} —— 这道闸的前提变了，\
             请重新确认「MCP 模式下没有 logger 往 stdout 写」是否仍然成立"
        );

        // 那个 target 必须出现在 `pub fn run()` 之后 —— 即它属于 run 的初始化，
        // 而不是被提到了模块层 / main 里。
        let run_at = lib_rs
            .find("pub fn run()")
            .expect("lib.rs 里应当有 pub fn run()");
        let target_at = lib_rs.find(stdout_target).expect("上面已经断言过它存在");
        assert!(
            target_at > run_at,
            "stdout 日志 target 被移到了 run() 之前 ⇒ MCP 模式会往协议通道写日志、\
             导致宿主断连。要么去掉那个 target，要么给 MCP 模式装一个只写文件的 logger。"
        );
    }

    /// `model` 的前缀与 `model_provider` / `model_reasoning_effort` 撞车 ——
    /// 抠错了会把 `"custom"` 当成模型名发出去（服务端 404，而错误信息里看不出原因）。
    #[test]
    fn extract_toml_string_matches_the_whole_key_not_a_prefix() {
        let toml = r#"
model_provider = "custom"
model = "gpt-image-2"
model_reasoning_effort = "high"

[model_providers.custom]
base_url = "https://api.example.com/v1"
"#;
        assert_eq!(
            extract_toml_string(toml, "model").as_deref(),
            Some("gpt-image-2"),
            "把 model_provider 或 model_reasoning_effort 当成了 model"
        );
        assert_eq!(
            extract_toml_string(toml, "base_url").as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(extract_toml_string(toml, "not_there"), None);
    }

    /// 空串等于没有 —— 回落到默认模型，而不是发一个空 model 出去。
    #[test]
    fn an_empty_value_reads_as_absent() {
        assert_eq!(extract_toml_string(r#"model = """#, "model"), None);
    }

    /// base_url 已带 `/v1`，端点只补后半段。多一个 `/v1` 会 404。
    #[test]
    fn images_url_does_not_double_the_v1_prefix() {
        assert_eq!(
            images_url("https://api.example.com/v1"),
            "https://api.example.com/v1/images/generations"
        );
    }

    /// 同一份内容得到同一个名字（可复现），不同内容不撞名。
    #[test]
    fn file_names_are_content_addressed() {
        assert_eq!(short_hash(b"abc"), short_hash(b"abc"));
        assert_ne!(short_hash(b"abc"), short_hash(b"abd"));
        assert_eq!(short_hash(b"abc").len(), 12);
    }
}
