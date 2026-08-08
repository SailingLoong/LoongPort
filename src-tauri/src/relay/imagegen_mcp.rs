//! 生图 MCP server：让 codex / claude 等 CLI 在**对话里**生图，用的是 LoongPort 已经
//! 备好的中转站档位。
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

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

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
macro_rules! diag {
    ($($arg:tt)*) => {
        eprintln!("[loongport-imagegen] {}", format!($($arg)*))
    };
}

/// 走哪个模型生图。
///
/// **不是常量而是从档位配置里读**：档位的 `model` 已经由 provision 写成了该分组真实的
/// `gpt-image-*`（见 [`super::provision::pick_model`]），中转站上 `gpt-image-3` 那天
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

/// 一张生成好的图。
struct GeneratedImage {
    /// 落盘位置。
    path: PathBuf,
    /// 原始 base64。**要回给宿主当 image content block** —— 见 [`handle_tool_call`]。
    b64: String,
}

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

/// 这个进程该用哪个数据目录。
///
/// ⚠️ **不能直接用 [`crate::config::get_app_config_dir`]**（review 抓出）：它查的是
/// `app_store` 里那个**进程内缓存**，而缓存只由 `refresh_app_config_dir_override`
/// （要 `AppHandle`）填 —— MCP 进程在 `run()` 之前就分流走了，没有 Tauri app ⇒
/// 缓存永远空 ⇒ 设过「LoongPort 配置目录」的用户会读到默认目录下的旧库（或读不到库），
/// 两种都不报错。
///
/// 所以走 [`crate::app_store::read_app_config_dir_override_without_tauri`]：
/// 直接读那个 store 文件。没设过覆盖时回落到默认目录 —— 与主程序一致。
fn app_dir() -> PathBuf {
    crate::app_store::read_app_config_dir_override_without_tauri()
        .unwrap_or_else(crate::config::get_app_config_dir)
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
/// 读出**当前**该用哪个档位生图。
///
/// ⚠️ **每次生图都重新调它**，不缓存 —— 那正是「切生图档位不用重启 codex」的实现：
/// 用户在 LoongPort 里换了档位，下一次工具调用就读到新的。缓存一次就把这个好处抵消了。
///
/// 没有任何生图档位被启用时返回 `Err`，文案引导用户去 LoongPort 里选一个 ——
/// **不自动挑一个**：用户可能压根不想用生图（他那个站可能没有生图分组），
/// 替他选一个等于替他决定花钱。
fn load_current_tier() -> Result<Tier, String> {
    let provider_id = current_image_tier_id()?;
    load_tier(&provider_id)
}

/// 「没选生图档位」时给用户的话。定义一次，两个调用点共用。
const NO_IMAGE_TIER_HINT: &str =
    "还没有选定用哪个档位生图。请打开 LoongPort 的「Codex 生图」标签页，在一个档位上点「启用」。";

/// 当前该用哪个档位生图 = `codex-image` 栏的当前项。
///
/// ## 为什么与聊天档位共用同一套机制
///
/// 「哪个档位生图」和「哪个档位聊天」是**同一类事实**（当前项），只是分属两栏。
/// 上一版为它另存了一个 `settings` 表的键（`loongport_current_image_tier`），那等于
/// 同一个概念有两套实现 —— 而分栏之后 `providers.is_current` 天然就是每栏一份，
/// 那个键成了纯粹的重复。已删除，不留兼容读取：它只在测试期存在过。
///
/// ## 两层来源，与主程序 `get_effective_current_provider` 严格对齐
///
/// | 层 | 位置 | 优先级 |
/// |---|---|---|
/// | 设备级 | `~/.loongport/settings.json` 的 `currentProviderCodexImage` | 高 |
/// | 库 | `providers.is_current`（`app_type='codex-image'`） | 低（fallback） |
///
/// ⚠️ **两层都要读**：主程序 `switch` 时两处都写（`settings::set_current_provider` 与
/// `db.set_current_provider`），所以只读 DB 那层在多数情况下也对。但设备级那层的存在
/// 意义正是「这台机器上用哪个」—— 云同步把另一台机器的 `is_current` 带过来时，本机
/// settings 才是对的。只读 DB 会让生图用错档位，而用户看界面（它读的是同一套两层逻辑）
/// 会觉得没问题。
///
/// ⚠️ **每次生图都重新调它**，不缓存 —— 那正是「切生图档位不用重启 codex」的实现：
/// 用户在 LoongPort 里换了档位，下一次工具调用就读到新的。缓存一次就把这个好处抵消了。
///
/// 一个都没有时返回 `Err`，文案引导用户去选 —— **不自动挑一个**：用户可能压根不想生图
/// （他那个站可能没有生图分组），替他选一个等于替他决定花钱。
fn current_image_tier_id() -> Result<String, String> {
    let db_path: PathBuf = app_dir().join(crate::config::DB_FILE_NAME);
    let conn = open_readonly(&db_path)?;

    // 第一层：设备级 settings.json。读不到 / 解析失败都只是「没有覆盖」，不是错误。
    if let Some(id) = device_level_image_tier() {
        // 与主程序同一条校验：本机记的那个档位得真的还在库里，否则回落到 DB
        // （`get_effective_current_provider` 在那种情况下会清掉本机的记录）。
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = ?1 AND app_type = ?2",
                rusqlite::params![&id, IMAGE_APP_TYPE],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if exists > 0 {
            return Ok(id);
        }
    }

    // 第二层：库里的 is_current。
    let value: Option<String> = conn
        .query_row(
            // ⚠️ **`ORDER BY id LIMIT 1`** —— 不省。
            //
            // 正常情况下这一栏只有一行 `is_current = 1`（`set_current_provider` 会先清
            // 其余的）。但「正常情况」是个不变量，不是保证：迁移、云同步导入、外部改库
            // 都可能留下两行，而 review 的探针实测抓到过一次（迁移换栏时带过去了 codex
            // 栏的 is_current）。那时裸 `query_row` 拿的是 SQLite 的返回顺序 ⇒
            // **用户选 4K 档、出的是 1K 的图，且换台机器结果不同、无法复现**。
            //
            // 排序不能修正「选错了哪一个」，但能让它**确定** —— 一个稳定的错比一个
            // 随机的错好查一个量级。真正的修正在迁移那侧（清零 is_current）。
            "SELECT id FROM providers WHERE app_type = ?1 AND is_current = 1 \
             ORDER BY id LIMIT 1",
            [IMAGE_APP_TYPE],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("读取当前生图档位失败: {e}"))?;

    value
        .filter(|v| !v.is_empty())
        .ok_or_else(|| NO_IMAGE_TIER_HINT.to_string())
}

/// 生图栏的 `app_type` 字符串。**从枚举取，不写字面量** —— 那个值同时用在
/// 三条 SQL 与写入侧，各写一遍迟早分叉，而症状是「切了没反应」。
const IMAGE_APP_TYPE: &str = crate::app_config::AppType::CODEX_IMAGE_STR;

/// 读设备级 settings.json 里记的生图档位。
///
/// ## 为什么不复用 `crate::settings::get_current_provider`
///
/// 那一层走一个进程内的 `OnceLock` 缓存（`settings_store()`），而它是在**主程序**
/// 启动时填的。这个进程没有那段启动流程 ⇒ 拿到的是 `Default`（全 `None`）⇒
/// 恒返回 `None`，而那是个静默的错误答案：生图会一直用 DB 那层，云同步场景下用错档位。
///
/// 所以直接读文件。路径与 `AppSettings::settings_path()` 必须一致 ——
/// 已加闸 `the_settings_path_matches_the_main_programs`。
fn device_level_image_tier() -> Option<String> {
    let path = crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    // 键名由 `AppSettings` 的 `#[serde(rename_all = "camelCase")]` 决定。
    let id = json.get("currentProviderCodexImage")?.as_str()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// 只读打开数据库。
///
/// 只读是必须的：这个进程与主程序可能同时在跑，绝不能拿写锁。
fn open_readonly(db_path: &std::path::Path) -> Result<rusqlite::Connection, String> {
    if !db_path.exists() {
        return Err(format!(
            "找不到 LoongPort 数据库（{}）。请先启动 LoongPort 并登录中转站。",
            db_path.display()
        ));
    }
    rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开数据库失败: {e}"))
}

fn load_tier(provider_id: &str) -> Result<Tier, String> {
    let db_path: PathBuf = app_dir().join(crate::config::DB_FILE_NAME);
    let conn = open_readonly(&db_path)?;

    // ⚠️ **`app_type` 必须参与查询**（review 抓出）—— `providers` 的主键是
    // `(id, app_type)`，一个 `provider_id` **真的会有多行**：实测维护者库里
    // `loongport-vendor-…` 那条有 6 行（claude / claude-desktop / codex / hermes /
    // openclaw / opencode）。
    //
    // 不带这个条件的后果：`query_row` 拿到的是 SQLite 先返回的那一行，若是 claude 那行，
    // 下面 `extract_api_key(.., Codex)` 读不出 `auth.OPENAI_API_KEY` ⇒ 报
    // 「配置里读不出密钥，请点获取密钥重新生成」—— 而**那条建议永远修不好它**
    // （重新 provision 只会再造出同样的多行），且成败取决于返回顺序、无法复现。
    //
    // 取 `codex-image` 是因为**生图档位就存在那一栏**（provision 按
    // `provision::image_tier_app_type` 分流）。取 codex 会查不到，症状是
    // 「档位已经不在了」而它明明在界面上。
    let (name, settings_raw): (String, String) = conn
        .query_row(
            "SELECT name, settings_config FROM providers WHERE id = ?1 AND app_type = ?2",
            rusqlite::params![provider_id, IMAGE_APP_TYPE],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| match e {
            // 标记指向的档位没了（用户删了账号 / 中转站下架了那个分组）。
            // ⚠️ **没有任何东西会自动清掉这个悬空的 `is_current`**（review 抓出）。
            // 设备级那层读的时候会校验存在性并跳过（见 `current_image_tier_id`），
            // 但库里那一行 `is_current = 1` 会一直留着 —— 删档位的路径（`remove_site_impl` /
            // `prune_stale_tiers` / 用户手工删）都只删记录，不管这个标记。
            //
            // 不为它加一条清理：`ProviderService::delete` 删掉那行之后
            // `is_current` 自然就查不到了（它是那一行上的列，不是一个独立指针）。
            // 走到这条错误分支说明记录**已经不在**，所以下次读就会落到
            // 「还没有选定」那条提示上 —— 状态自然收敛，不需要额外的清理逻辑。
            //
            // 所以这里只要把话说清楚：让用户去重选，而不是去「获取密钥」。
            rusqlite::Error::QueryReturnedNoRows => format!(
                "生图档位 {provider_id} 已经不在了（可能被删除，或中转站下架了那个分组）。请打开 LoongPort 的「Codex 生图」标签页，在一个档位上点「启用」。"
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
                "档位「{name}」的配置里读不出密钥。请在 LoongPort 里对它点「获取密钥」重新生成。"
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
        diag!("档位「{name}」读不出 model，生图回落 {FALLBACK_IMAGE_MODEL}");
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
    // 同样走 `app_dir()` —— 用户把数据目录挪走了，图也该跟着落在那里，
    // 而不是散在默认目录（他会找不到）。
    app_dir().join("generated_images")
}

/// 调一次生图，返回落盘后的文件路径。
async fn generate_image(
    tier: &Tier,
    prompt: &str,
    size: Option<&str>,
) -> Result<Vec<GeneratedImage>, String> {
    let client = reqwest::Client::builder()
        // 生图慢（实测 30-90s），默认超时会在出图前就断。
        //
        // **240 而不是 300**：codex 的 MCP 工具超时默认正好是 300s
        // （`codex-rs/codex-mcp/src/rmcp_client.rs` 的 `DEFAULT_TOOL_TIMEOUT`，
        // 本机 0.146 实测：310s 的调用在 300.16s 被它切断）。两边同为 300 时，
        // 真的超时那次是宿主先报它自己那句泛泛的超时，我们这句「请求生图接口失败」
        // 反而抢不到 —— 留 60s 余量让**更具体的那条**错误信息先到用户眼前。
        .timeout(std::time::Duration::from_secs(240))
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败: {e}"))?;

    // ⚠️ **有意不发 `response_format`** —— 这里曾经加过 `"b64_json"`，是个过度修正
    // （第二轮 review 抓出）：
    //
    // - `gpt-image-*` **只返回 base64，没有 url 模式**，所以下面只认 `b64_json` 的解析
    //   本来就是对的，不需要这个字段来保证。
    // - 而官方 `/v1/images/generations` 对 `gpt-image-*` 带这个字段**直接 400**
    //   （`Unknown parameter: 'response_format'` —— 它是给已下线的 `dall-e-*` 留的）。
    // - sub2api 把请求体**原样透传**给上游（只改 `model`，见其
    //   `rewriteOpenAIImagesModel`）⇒ 上游是 API-key 类账号时那个 400 会真的打回来。
    //
    // ⚠️ **本地测出 200 不能证明它安全**：调度器挑到 OAuth 类账号时该字段被丢弃，
    // 于是同一个档位在不同的调度结果下表现不同。不发它则两条路都对。
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
        // base64 原样留着 —— 下面要作为 MCP 的 image content block 回给宿主，
        // 让模型**真的看到图**而不只是拿到一个路径。见 `handle_tool_call`。
        saved.push(GeneratedImage {
            path,
            b64: b64.to_string(),
        });
    }

    if saved.is_empty() {
        return Err("生图接口没有返回任何图片".into());
    }
    // 顺手修剪 —— 见 `prune_old_images`。**把这次刚写的排除在外**：它们的 mtime 是最新的，
    // 正常不会被当成「最旧」删掉，但目录恰好满员时没有必要让「刚生成的图」参与这场竞争
    // （返回给宿主的路径必须还在）。失败只记一行：修剪不成功不影响这次出图。
    let just_written: Vec<&std::path::Path> = saved.iter().map(|i| i.path.as_path()).collect();
    if let Err(e) = prune_old_images(&dir, &just_written) {
        diag!("清理旧图片失败（不影响本次生成）: {e}");
    }
    Ok(saved)
}

/// 出图目录最多留多少张。
///
/// 一张 1024² 的 PNG 实测 0.7–2 MB，200 张约 150–400 MB —— 对「随手生成的中间产物」
/// 这个量级够用，也不至于让用户某天发现家目录里躺了几十 G。
///
/// **不按时间修剪**：用户可能几个月才生一次图，按天数删会把他唯一那几张删掉；
/// 而按数量删的语义清楚 ——「留最近的 N 张」。
const MAX_KEPT_IMAGES: usize = 200;

/// 把出图目录修剪到 [`MAX_KEPT_IMAGES`] 张，删最旧的。
///
/// ## 为什么要有它
///
/// 文件名是内容哈希，所以同图不会重复占位；但不同 prompt 会一直堆积，而**没有任何
/// 东西会清它** —— 那就是「知情引入却没留痕的占位」，属技术债（本函数就是那笔债的偿还）。
///
/// 按 mtime 排序删最旧的。读不到 mtime 的排最前（当最旧）—— 那种文件多半是异常留下的。
///
/// ## 并发是安全的
///
/// codex 与 claude 各起一个 MCP 进程时，两边可能同时修剪。这不会互相删掉对方的图：
/// 判据是 mtime，而另一个进程**刚写的图 mtime 是最新的**，排在末尾。删不掉的（已被
/// 对方删了）只记一行，不当错误 —— 修剪是收尾动作，不该影响出图。
///
/// `keep` 是本次调用刚写的那些，一律排除（见调用处）。
fn prune_old_images(dir: &std::path::Path, keep: &[&std::path::Path]) -> Result<(), String> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("读出图目录失败: {e}"))?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        // 这次刚写的不参与 —— 调用方要把它们的路径返回给宿主。
        .filter(|e| !keep.contains(&e.path().as_path()))
        .map(|e| {
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (mtime, e.path())
        })
        .collect();

    if files.len() <= MAX_KEPT_IMAGES {
        return Ok(());
    }
    // 旧的在前，删掉超出的那些。
    files.sort_by_key(|(mtime, _)| *mtime);
    let excess = files.len() - MAX_KEPT_IMAGES;
    for (_, path) in files.iter().take(excess) {
        if let Err(e) = std::fs::remove_file(path) {
            diag!("删不掉旧图片 {}: {e}", path.display());
        }
    }
    diag!("出图目录已修剪：删掉 {excess} 张最旧的，保留 {MAX_KEPT_IMAGES} 张");
    Ok(())
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
        "description": "用 LoongPort 绑定的中转站档位生成图片（gpt-image 系列模型）。直接返回图片本身，同时给出保存到本地的路径。",
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

    // ⚠️ **每次调用都重查当前档位**，不用启动时那份 —— 用户在 LoongPort 里换了生图
    // 档位，下一次生图就该用新的，**不必重启 codex**。见 `current_image_tier_id`。
    let tier = load_current_tier()?;
    let images = generate_image(&tier, prompt, size).await?;
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
    let mut content = vec![json!({
        "type": "text",
        "text": format!(
            "已生成 {} 张图片（档位：{}，模型：{}），已存到：\n{list}",
            images.len(),
            tier.display_name,
            tier.model
        )
    })];
    for img in &images {
        content.push(json!({
            "type": "image",
            "data": img.b64,
            "mimeType": "image/png",
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
    match load_current_tier() {
        Ok(tier) => diag!(
            "生图 MCP 启动：档位「{}」，模型 {}，端点 {}",
            tier.display_name,
            tier.model,
            images_url(&tier.base_url)
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
    /// ⭐ **settings.json 的路径必须与主程序一致。**
    ///
    /// 这个进程读设备级「当前生图档位」是**自己拼路径读文件**（不能复用
    /// `crate::settings`，见 [`device_level_image_tier`] 的文档）。路径一分叉，
    /// 读到的永远是「没有覆盖」⇒ 静默回落到 DB 那层 ⇒ 云同步场景下生图用错档位，
    /// 而界面显示的是对的（它走两层逻辑），没有任何东西会报错。
    #[test]
    fn the_settings_path_matches_the_main_programs() {
        let settings_rs = include_str!("../settings.rs");
        // 主程序那份是三段拼接：home / APP_DIR_NAME / "settings.json"。
        assert!(
            settings_rs.contains("crate::config::APP_DIR_NAME")
                && settings_rs.contains("\"settings.json\""),
            "主程序的 settings.json 路径拼法变了 —— 生图 MCP 那份手抄的跟着改，\
             否则设备级「当前生图档位」永远读不到"
        );
    }

    /// ⭐ **那个 JSON 键名必须与 `AppSettings` 的字段对得上。**
    ///
    /// 键名由 `#[serde(rename_all = "camelCase")]` 从字段名派生，所以这里是一份手抄。
    /// 抄错的后果同上：静默读不到。
    #[test]
    fn the_device_level_key_matches_the_settings_field() {
        let settings_rs = include_str!("../settings.rs");
        assert!(
            settings_rs.contains("pub current_provider_codex_image: Option<String>"),
            "`AppSettings::current_provider_codex_image` 改名了 —— \
             `device_level_image_tier` 里那个 camelCase 键名跟着改"
        );
    }

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
