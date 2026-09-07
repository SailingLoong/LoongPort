//! 生图 MCP server：让 codex / claude 等 CLI 在**对话里**生图，用的是 LoongPort 已经
//! 备好的中转站档位。
//!
//! 本模块只做 MCP 的事：注册生命周期、stdio JSON-RPC 协议、把工具调用翻译成对
//! [`super::imagegen`]（生图核心：档位现读 → 调 `/v1/images/generations` → 落盘）
//! 的调用。**生图实现不在两处** —— App 内直接生图（生图页「生成」视图，不消耗
//! 任何 CLI 会话上下文）与这里是同一个核心的两个入口。
//!
//! # 为什么要有它（而不是让用户把档位的 `model` 改成 `gpt-image-2`）
//!
//! sub2api 上有两条生图链路，**它们要求上游提供的模型不同**：
//!
//! | 链路 | 端点 | 上游要能提供 |
//! |---|---|---|
//! | codex 主模型设成 `gpt-image-2` | `/v1/responses` | **`gpt-5.4-mini`**（见下） |
//! | 本模块 / App 内直接生图 | `/v1/images/generations` | `gpt-image-2` 本身 |
//!
//! 第一条那个反直觉的要求来自上游的归一化：`normalizeOpenAIResponsesImageOnlyModel`
//! 会把 image-only 主模型的请求改写成「文本主模型 + `image_generation` tool」的形状，
//! 而它写死的那个文本主模型是 `gpt-5.4-mini`（sub2api `service/openai_images.go` 的
//! `openAIImagesResponsesMainModel`）。⇒ 上游只挂了生图模型的中转站上，第一条**必然
//! 502**（实测有站点的两个生图分组：`sync-models` 问上游只回 `gpt-image-2`）。
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
//! 所以入口是 `LoongPort --mcp-image-gen`，
//! 在 [`crate::run`] **之前**分流（见那里的说明：走进去会被 single-instance 插件
//! 当成第二个实例而唤起主窗口）。
//!
//! # 为什么 sk 不写进 CLI 的配置文件
//!
//! codex 的 `[mcp_servers.*]` 支持 `env`，把 sk 塞进去最省事 —— 但那样 sk 会以明文
//! 落在 `~/.codex/config.toml` 里，并且**档位刷新换了 sk 之后就失效**（用户看到的是
//! 生图突然 401，而配置文件看起来一切正常）。
//!
//! 所以 sk 在**每次调用时**从 LoongPort 库里现读（见 [`super::imagegen`]）。
//! provision 换了 sk 下次生图自动是新的，不需要任何同步逻辑 —— 这也是为什么这件事
//! 只有 LoongPort 做得漂亮：库在我们手里。

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::app_config::{McpApps, McpServer};
use crate::error::AppError;
use crate::services::{McpService, ProviderService};
use crate::store::AppState;

use super::imagegen;

/// 生图 MCP 在统一 MCP 数据源里的稳定 id。
pub const IMAGEGEN_MCP_ID: &str = "loongport-imagegen";

/// 启动 MCP server 模式的命令行开关。
pub const IMAGEGEN_MCP_FLAG: &str = "--mcp-image-gen";

/// 让生图 MCP 注册状态与生图档位、用户开关保持一致，并投影到支持它的 CLI。
///
/// 这是生图 MCP 生命周期的唯一入口。provision、删站、应用启动、进入生图页与切换
/// 「在 CLI 对话中提供生图工具」开关都调用它；函数本身幂等，因此这些入口可以按
/// 各自生命周期无条件对齐。
///
/// 注销的判据有两个，满足其一即撤：
///
/// 1. 生图栏里没有托管档位了（没有可绑定的东西）；
/// 2. 用户关了开关（`settings::imagegen_mcp_enabled`，见那里的文档：只要注册着，
///    工具描述就占宿主每次会话的上下文 —— 只想直接生图的用户要的是**根本不注册**）。
pub fn sync_registration(state: &AppState) -> Result<(), AppError> {
    let has_image_tiers = ProviderService::list(state, crate::app_config::AppType::CodexImage)?
        .values()
        .any(|provider| crate::relay::is_managed(&provider.id));

    let mcp_enabled = crate::settings::get_imagegen_mcp_enabled();

    if !has_image_tiers || !mcp_enabled {
        let reason = if !has_image_tiers {
            "生图栏里没有档位了"
        } else {
            "用户关闭了「在 CLI 对话中提供生图工具」"
        };
        let removed = McpService::delete_server(state, IMAGEGEN_MCP_ID)?;
        if removed {
            log::info!("{reason}，撤掉生图 MCP 记录");
        }
        return Ok(());
    }

    let exe = std::env::current_exe()
        .map_err(|e| AppError::Message(format!("获取可执行文件路径失败: {e}")))?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| AppError::Message("可执行文件路径不是有效的 UTF-8".into()))?;

    McpService::upsert_server(state, registration_server(exe_str))
}

fn registration_server(exe: &str) -> McpServer {
    McpServer {
        id: IMAGEGEN_MCP_ID.to_string(),
        name: "LoongPort 生图".to_string(),
        server: json!({
            "type": "stdio",
            "command": exe,
            "args": [IMAGEGEN_MCP_FLAG],
        }),
        apps: McpApps {
            codex: true,
            claude: true,
            gemini: true,
            ..Default::default()
        },
        description: Some(
            "用 LoongPort「生图」标签页里当前那个接入配置生图（gpt-image、grok-imagine 等生图模型）。\
             由 LoongPort 自动维护，密钥不写进 CLI 配置 —— 换档位也不必重启 CLI。"
                .to_string(),
        ),
        homepage: None,
        docs: None,
        tags: vec!["loongport".into(), "image".into()],
    }
}

/// 本模块的诊断输出：**写 stderr**。
///
/// ## 为什么不用 `log::`（review 抓出的一个真空档）
///
/// 这个 crate 的 logger 由 `tauri_plugin_log` 在 [`crate::run`] 的 setup 里安装，而
/// MCP 模式**在 `run()` 之前就分流走了** ⇒ 这个进程里根本没有 logger ⇒ 所有 `log::`
/// 宏都是**空操作**。原来那几行 `log::info!` 一个字都没落下来，而模块文档却宣称
/// 「诊断走 log（落文件）」—— 那是最糟的状态：承诺了一条不存在的通道。
///
/// ## 为什么是 stderr 而不是自己装一个文件 logger
///
/// 1. **stderr 天然是 MCP server 的诊断通道**：宿主（codex / claude）会捕获子进程的
///    stderr 落进自己的会话日志，用户报问题时那份日志本来就要看 —— 比让他去翻我们
///    另一个目录里的文件更可能被找到。
/// 2. **它不在协议通道上**，没有污染 stdout 的风险（那是本模块最怕的事）。
/// 3. 自己装 logger 要么引新依赖，要么把 `tauri_plugin_log` 的初始化挪到 `run()` 之外
///    —— 后者恰好会把它那个 stdout target 带进 MCP 进程，即**制造**我们要防的故障。
fn format_stderr_diagnostic(message: &str) -> String {
    format!(
        "[loongport-imagegen] {}",
        crate::diagnostics::redact_log_text(message)
    )
}

macro_rules! diag {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        eprintln!("{}", format_stderr_diagnostic(&message));
    }};
}

/// MCP 协议版本。跟着 codex-cli 0.146 实际发的那个走。
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 本 server 暴露的工具清单。
///
/// **只有一个工具**：需求是「在对话里生图」。编辑图 / 透明背景那些等有人真要
/// 再加 —— 每个工具都要写 schema、要在 prompt 里占位置，先把一件事做对。
/// （张数 `n` 是参数不是工具：一次调 `n` 张与并发调 `n` 次是同一张账单。）
fn tools_list() -> Value {
    json!([{
        "name": "generate_image",
        "description": "用 LoongPort 绑定的中转站接入配置生成图片（gpt-image、grok-imagine 等生图模型）。直接返回图片本身，同时给出保存到本地的路径。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "要生成的图片的描述。用英文写通常效果更好。"
                },
                "size": {
                    "type": "string",
                    "description": "图片尺寸，形如 1024x1024 或 1536x1024。省略则用 1024x1024。注意上游可能返回与请求不同的实际尺寸。"
                },
                "n": {
                    "type": "integer",
                    "description": "一次生成几张（1-50，默认 1）。按张数计费；多张会自动拆成并发单张请求。大批量耗时更长，宿主的工具超时（codex 默认 300 秒）可能在完成前先到 —— 超大批量时并发多次调用本工具（每次各自计时）通常更稳。"
                }
            },
            "required": ["prompt"]
        }
    }])
}

/// 处理一条 JSON-RPC 请求，返回要写回去的响应（`None` = 这是个通知，不必回）。
async fn handle_request(req: &Value) -> Option<Value> {
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
        "tools/call" => handle_tool_call(req).await,
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

async fn handle_tool_call(req: &Value) -> Result<Value, String> {
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
    // 张数由宿主 agent 传，默认 1。校验放在读档位**之前**：越界是调用方的错，
    // 该先报它（没配档位的机器上也能得到这条而不是「还没选定档位」）。
    // 范围判据的唯一源在核心层（`imagegen::validate_count`），与 App 内入口共用。
    let n = args.get("n").and_then(Value::as_u64).unwrap_or(1);
    let n = imagegen::validate_count(u32::try_from(n).unwrap_or(u32::MAX))?;

    // ⚠️ **每次调用都重查当前档位**，不用启动时那份 —— 用户在 LoongPort 里换了生图
    // 档位，下一次生图就该用新的，**不必重启 codex**。见 `imagegen::current_image_tier_id`。
    let tier = imagegen::load_current_tier()?;
    // MCP 入口恒为并发：agent 想串行有自己的表达（逐次调用工具天然串行），
    // 不为它加 schema 噪音。见 `imagegen::split_batch` 的表。
    let (images, failed) = imagegen::generate_batch(&tier, prompt, size, n, true).await?;
    let list = images
        .iter()
        .map(|i| i.path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    // ⚠️ **必须回 `image` content block，不能只给文件路径**（review 抓出）。
    //
    // 两个原因，缺一个这功能就是半残的：
    //
    // 1. **模型看不见图**。只给路径的话它只能去读文件，而 codex 默认沙箱是
    //    `workspace-write` / `read-only` ⇒ `~/.loongport/` 在工作区之外，
    //    它**连读都读不到**那个路径。于是「生成一张图」的结果是一句它自己也打不开的
    //    文字，更没法据此迭代（「把猫改成橘色的」）。
    // 2. **宿主本来就支持**：codex 0.146 实现了完整的 `ContentBlock` 联合类型
    //    （`TextContent | ImageContent | AudioContent | ResourceLink |
    //    EmbeddedResource`），还有它自己的 `_meta: {"codex/imageDetail": ...}` 扩展。
    //    不发等于白放着能力不用。
    //
    // bytes 在写文件前就在手上，所以这不额外发请求。
    let failure_note = if failed > 0 {
        format!("（另有 {failed} 张失败）")
    } else {
        String::new()
    };
    let mut content = vec![json!({
        "type": "text",
        "text": format!(
            "已生成 {} 张图片{failure_note}（接入配置：{}，模型：{}），已存到：\n{list}",
            images.len(),
            tier.display_name,
            tier.model
        )
    })];
    for img in &images {
        content.push(json!({
            "type": "image",
            "data": img.b64,
            "mimeType": img.mime,
        }));
    }

    Ok(json!({ "content": content }))
}

/// MCP server 主循环：stdin 读一行一条 JSON-RPC，stdout 写一行一条响应。
///
/// ⚠️ **stdout 只许写协议消息** —— 宿主按行解析 JSON，掺一句日志进去它就断连。
/// 本模块的诊断一律走 **stderr**（[`diag!`]），绝不 `println!`、也不用 `log::`
/// （见 [`diag!`] 的文档：那个宏在这个进程里是空操作）。
pub fn serve() -> Result<(), String> {
    // ⚠️ **启动时不要求「已选定生图档位」** —— 那会让没选过的用户在 codex 里看到
    // 「工具启动失败」，而正确的表达是「工具在，但你还没选用哪个档位」：
    // 前者像是软件坏了，后者是一句他能照做的话。所以这里只记一行诊断，
    // 真正的检查推迟到 `tools/call`（那时报的错会作为工具结果显示给模型与用户）。
    match imagegen::load_current_tier() {
        Ok(tier) => diag!(
            "生图 MCP 启动：档位「{}」，模型 {}，端点 {}",
            tier.display_name,
            tier.model,
            imagegen::images_url(&tier.base_url)
        ),
        Err(e) => diag!("生图 MCP 启动（尚未选定档位）：{e}"),
    }

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
                diag!("收到无法解析的消息（已跳过）: {e}");
                continue;
            }
        };
        if let Some(resp) = runtime.block_on(handle_request(&req)) {
            let mut out =
                serde_json::to_string(&resp).map_err(|e| format!("序列化响应失败: {e}"))?;
            out.push('\n');
            stdout
                .write_all(out.as_bytes())
                .map_err(|e| format!("写 stdout 失败: {e}"))?;
            stdout.flush().map_err(|e| format!("flush 失败: {e}"))?;
        }
    }
    diag!("生图 MCP 退出（stdin 关闭）");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_diagnostics_redact_credentials_and_response_bodies() {
        let message = format_stderr_diagnostic(
            "Authorization: Bearer sk-private-token\nresponse_body=<html>secret</html>",
        );

        assert!(!message.contains("sk-private-token"));
        assert!(!message.contains("<html>secret</html>"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn registration_targets_supported_hosts_without_binding_a_tier() {
        let server = registration_server("/Applications/LoongPort.app/Contents/MacOS/LoongPort");

        assert_eq!(server.id, IMAGEGEN_MCP_ID);
        assert!(server.apps.codex && server.apps.claude && server.apps.gemini);
        assert!(!server.apps.opencode && !server.apps.hermes);
        assert_eq!(
            server.server,
            json!({
                "type": "stdio",
                "command": "/Applications/LoongPort.app/Contents/MacOS/LoongPort",
                "args": [IMAGEGEN_MCP_FLAG],
            })
        );
    }

    /// ⭐ **开关的 JSON 键名必须与 `AppSettings` 的字段对得上**，且默认值必须与
    /// `get_imagegen_mcp_enabled` 的回落一致（缺省 = 开）。键名抄错 ⇒ 前端永远写不进
    /// 那个开关；默认值分叉 ⇒ 升级后用户没动过开关，注册行为却变了。
    #[test]
    fn the_mcp_switch_key_and_default_match_the_settings_field() {
        let settings_rs = include_str!("../settings.rs");
        assert!(
            settings_rs.contains("pub imagegen_mcp_enabled: Option<bool>"),
            "`AppSettings::imagegen_mcp_enabled` 改名了 —— \
             `sync_registration` 的开关判定与前端 camelCase 键名都跟着改"
        );
        assert!(
            settings_rs.contains("imagegen_mcp_enabled.unwrap_or(true)"),
            "`get_imagegen_mcp_enabled` 的回落不再是「默认开」—— \
             这是个产品行为决定（升级用户的注册行为不变），改它要连着文档一起改"
        );

        // 前端那份默认值（`?? true`）也必须与后端回落一致：分叉的症状是开关显示
        // 「开」而后端行为是「关」（或反过来），跨语言编译器管不到 `.ts`。
        let notice_tsx = include_str!("../../../src/components/relay/ImageTabNotice.tsx");
        assert!(
            notice_tsx.contains("settings?.imagegenMcpEnabled ?? true"),
            "src/components/relay/ImageTabNotice.tsx 的开关默认值不再是 `?? true` —— \
             它必须与 settings.rs 的 `imagegen_mcp_enabled.unwrap_or(true)` 保持一致，\
             两边要一起改"
        );
    }

    /// ⭐ **注册同步的触发点全在数据层**（2026-09-07 收口，删掉了「进入生图页时
    /// 同步」的前端入口——那是视图读路径驱动数据层行为）。
    ///
    /// 这道闸钉住**启动**那一处接线：其余触发点（provision 收尾、开关写入、删站点）
    /// 各有行为测试或在源码闸里被顺带覆盖，启动这处在 lib.rs 的初始化闭包里，
    /// 没有任何行为测试会碰它 —— 删掉它的症状是静默的：升级后不再对齐注册，
    /// 用户的 CLI 里残留（或缺失）生图工具，直到下次 provision。
    #[test]
    fn startup_wires_the_registration_sync() {
        let lib_rs = include_str!("../lib.rs");
        assert!(
            lib_rs.contains("imagegen_mcp::sync_registration(&app_state)"),
            "lib.rs 的启动初始化不再调用 imagegen_mcp::sync_registration —— \
             这是注册同步在数据层的启动触发点，删它要有替代方案（见触发点清单），\
             别只是把断言改绿"
        );
        // 前端读路径不再有直接触发（那是这轮收口删掉的形状，别让它回来）。
        let app_tsx = include_str!("../../../src/App.tsx");
        assert!(
            !app_tsx.contains("relay_sync_imagegen_mcp"),
            "App.tsx 又出现了生图 MCP 同步的直接触发 —— 视图读路径不驱动数据层行为，\
             触发点归数据层（启动/provision/设置写入/删除）"
        );
    }

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
}
