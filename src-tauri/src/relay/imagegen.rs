//! 生图核心：读当前生图档位 → 调中转站 `/v1/images/generations` → 落盘与修剪。
//!
//! 这里是生图的**唯一实现**，两个入口共用：
//!
//! | 入口 | 进程 | 见 |
//! |---|---|---|
//! | MCP 工具（codex / claude 在对话里调用） | 主程序二进制 `--mcp-image-gen` 子进程 | [`super::imagegen_mcp`] |
//! | App 内直接生图（生图页「生成」视图） | 主程序 | `commands::relay::relay_imagegen_generate` |
//!
//! 两条链路的产品语义不同（前者让宿主模型看见图并继续对话，后者纯粹出图、
//! **不消耗任何 CLI 会话的上下文**），但档位选择、请求形状、落盘与修剪必须完全
//! 一致 —— 用户心里只有一个事实：「我现在用这家生图」。
//!
//! # 诊断走 `log::`，而不是 stderr
//!
//! [`super::imagegen_mcp`] 的 `diag!` 写 stderr，是因为 MCP 子进程里没有 logger。
//! 本模块被两个进程共用：在 MCP 进程里 `log::` 是空操作（那里约定俗成，见
//! imagegen_mcp 的文档），在主程序里则正常落日志文件。共享代码选 `log::`
//! 让主程序侧可观测，MCP 侧不丢它真正需要的协议行为。

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;

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
pub const DEFAULT_SIZE: &str = "1024x1024";

/// 一张生成好的图。
pub(crate) struct GeneratedImage {
    /// 落盘位置。
    pub(crate) path: PathBuf,
    /// 图片字节的 base64。**MCP 入口要回给宿主当 image content block** —— 让模型
    /// 真的看见图；App 内入口不需要它，只取 path。
    pub(crate) b64: String,
    /// 魔数嗅探出的真实格式（与文件扩展名同源）。url 变体可能下发 jpeg / webp，
    /// mimeType 声明错了宿主可能渲染不了。
    pub(crate) mime: &'static str,
}

/// 这次调用绑定的档位。
pub(crate) struct Tier {
    /// 明文 sk。
    pub(crate) api_key: String,
    /// 形如 `https://api.example.com/v1`（**末尾无斜杠**，见 [`images_url`]）。
    pub(crate) base_url: String,
    /// 生图模型名，取自档位配置里的 `model`。
    pub(crate) model: String,
    /// 档位显示名，用于结果文案（MCP 的 text block / App 内的 toast）。
    pub(crate) display_name: String,
}

/// 这个进程该用哪个数据目录。
///
/// ⚠️ **不能直接用 [`crate::config::get_app_config_dir`]**：它查的是
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

/// 读出**当前**该用哪个档位生图。
///
/// ⚠️ **每次生图都重新调它**，不缓存 —— 那正是「切生图档位不用重启 codex」的实现：
/// 用户在 LoongPort 里换了档位，下一次调用就读到新的。缓存一次就把这个好处抵消了。
///
/// 没有任何生图档位被启用时返回 `Err`，文案引导用户去 LoongPort 里选一个 ——
/// **不自动挑一个**：用户可能压根不想用生图（他那个站可能没有生图分组），
/// 替他选一个等于替他决定花钱。
pub(crate) fn load_current_tier() -> Result<Tier, String> {
    let provider_id = current_image_tier_id()?;
    load_tier(&provider_id)
}

/// 「没选生图档位」时给用户的话。定义一次，两个调用点共用。
pub(crate) const NO_IMAGE_TIER_HINT: &str =
    "还没有选定用哪个接入配置生图。请打开 LoongPort 的「生图」标签页，在一个接入配置上点「启用」。";

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
/// 启动时填的。MCP 子进程没有那段启动流程 ⇒ 拿到的是 `Default`（全 `None`）⇒
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
/// 只读是必须的：MCP 子进程与主程序可能同时在跑，绝不能拿写锁。
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

/// 从 LoongPort 库里读出某个档位的 sk / base_url / model。
///
/// ## 为什么 MCP 进程直接读 sqlite 而不复用 `ProviderService`
///
/// 那一层要 `AppState`（Tauri 托管的状态），而 MCP 子进程**没有 Tauri app** —— 它在
/// `run()` 之前就分流走了。为一个只读三个字段的场景把 Tauri 运行时拉起来是本末倒置。
///
/// 代价是对 `providers` 表的形状有了第二处依赖。可接受：读的是 `id` /
/// `settings_config` 这两个最稳定的列（`settings_config` 的结构还共用
/// [`super::provision::extract_api_key`]，没有另写一份解析）。
fn load_tier(provider_id: &str) -> Result<Tier, String> {
    let db_path: PathBuf = app_dir().join(crate::config::DB_FILE_NAME);
    let conn = open_readonly(&db_path)?;

    // ⚠️ **`app_type` 必须参与查询** —— `providers` 的主键是
    // `(id, app_type)`，一个 `provider_id` **真的会有多行**：同一个 id 能合法地挂在
    // 多个 app 栏下。不带这个条件的后果：`query_row` 拿到的是 SQLite 先返回的那一行，
    // 若是 claude 那行，下面 `extract_api_key(.., Codex)` 读不出
    // `auth.OPENAI_API_KEY` ⇒ 报「配置里读不出密钥」—— 而那条建议永远修不好它
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
            // ⚠️ **没有任何东西会自动清掉这个悬空的 `is_current`**。
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
                "生图接入配置 {provider_id} 已经不在了（可能被删除，或中转站下架了那个分组）。请打开 LoongPort 的「生图」标签页，在一个接入配置上点「启用」。"
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
                "接入配置「{name}」的配置里读不出密钥。请在 LoongPort 里对它点「获取密钥」重新生成。"
            )
        })?;

    let config_toml = settings
        .get("config")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("接入配置「{name}」的配置里没有 config.toml 内容"))?;

    let base_url = extract_toml_string(config_toml, "base_url")
        .ok_or_else(|| format!("接入配置「{name}」的配置里没有 base_url"))?;
    // 读不出 model 不是错误：老档位（本功能上线前 provision 的）可能没有生图模型名，
    // 回落到默认值让它仍然能用。
    let model = extract_toml_string(config_toml, "model").unwrap_or_else(|| {
        log::debug!("档位「{name}」读不出 model，生图回落 {FALLBACK_IMAGE_MODEL}");
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
/// **不引 toml 解析器**：要读的两个键都是 `key = "值"` 这种最简形状
/// （由 [`super::provision::codex_config_toml`] 生成，形状我们自己定的）。
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
/// `base_url` 已经带 `/v1`（[`super::sub2api::codex_base_url`] 保证），所以这里只接
/// `/images/generations`。
pub(crate) fn images_url(base_url: &str) -> String {
    format!("{base_url}/images/generations")
}

/// 出的图存哪。
///
/// 优先用户自定义（settings.json 设备级 `imagegenOutputDir`，生图页「更改存储位置」
/// 写入）；缺省 `<数据目录>/generated_images/`：与数据库同目录，用户找得到，也不会
/// 污染他当前的工作目录（Agent 常在用户仓库里跑，往那里丢文件会进 git status）。
pub(crate) fn output_dir() -> PathBuf {
    if let Some(custom) = custom_output_dir() {
        return custom;
    }
    // 同样走 `app_dir()` —— 用户把数据目录挪走了，图也该跟着落在那里，
    // 而不是散在默认目录（他会找不到）。
    app_dir().join("generated_images")
}

/// 设备级设置里的自定义出图目录（绝对路径字符串）。
///
/// 直读 settings.json **文件**而不是 `crate::settings` 的进程内缓存 —— 与
/// [`device_level_image_tier`] 同一个理由：MCP 子进程没有主程序的启动流程，缓存恒为
/// 空，读缓存会一直得到「没有自定义」⇒ MCP 把图写进默认目录而 App 看自定义目录，
/// 两边静默分叉。读文件让两个进程天然一致，且换路径后 **codex 不必重启**
/// （MCP 每次生图现读，与切档位同一好处）。
fn custom_output_dir() -> Option<PathBuf> {
    let path = crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let dir = json.get("imagegenOutputDir")?.as_str()?.trim();
    if dir.is_empty() {
        return None;
    }
    Some(PathBuf::from(dir))
}

/// 一次请求的超时上限 = 每张图 [`SINGLE_IMAGE_TIMEOUT_SECS`] 的既证预算 × 张数。
///
/// 生图慢（实测单张 30-90s），串行上游的耗时随 `n` 线性涨，超时也跟着涨才不至于
/// 把能出完的请求掐死。两条入口都按 [`request_timeout`] 取值：
///
/// - **App 内直接生图**没有宿主超时这层约束，按张数放大的封顶就是诚实的等待上限
///   （n=4 ⇒ 16 分钟封顶；正常远快于此，超时只兜底死掉的上游）。
/// - **MCP 工具**的 `n` 由宿主 agent 传（默认 1）。codex 的工具超时默认正好 300s，
///   单张时两边同为 300 会让宿主先报它那句泛泛的错 —— 所以单张预算是 240s 而不是
///   300s，留 60s 余量让「请求生图接口失败」这条更具体的先到；多张时宿主默认
///   超时大概率先杀掉调用（schema 的 `n` 描述里写明了这点，并建议要更多张时
///   并发多次调用工具、每次各自计时）。我们这层仍按张数放大：宿主侧调大了超时
///   （或 claude / gemini 这类默认更宽的宿主）时，不该被我们自己的客户端掐死。
const SINGLE_IMAGE_TIMEOUT_SECS: u64 = 240;

/// 见 [`SINGLE_IMAGE_TIMEOUT_SECS`] 的文档。
pub(crate) fn request_timeout(n: u32) -> std::time::Duration {
    std::time::Duration::from_secs(SINGLE_IMAGE_TIMEOUT_SECS * n.max(1) as u64)
}

/// 一次生成最多几张。闸在核心层这一份（两条入口都调 [`validate_count`]）：
/// 一次 `n` 张就是 `n` 张的钱，前端与工具 schema 只是入口，真正的闸必须在后端 ——
/// 别的入口不该绕过它。50 是「用户明确要一批」的合理上限，也是防手滑的天花板
/// （比如把循环变量当张数传进来）。
pub(crate) const MAX_IMAGE_COUNT: u32 = 50;

/// 张数的唯一判据（合法原样返回，越界报错）。App 内命令与 MCP 工具共用，
/// 各自再写一遍范围就会分叉。
pub(crate) fn validate_count(n: u32) -> Result<u32, String> {
    if (1..=MAX_IMAGE_COUNT).contains(&n) {
        Ok(n)
    } else {
        Err(format!("张数只支持 1 到 {MAX_IMAGE_COUNT}"))
    }
}

/// 并发模式下同时在途的请求数。
///
/// 50 个请求同时打一家中转站容易撞限流（429），排队比触发对方风控便宜；
/// 4 与单请求的张数上限对齐，一个小批量就是一波。串行模式走 1（逐张）。
const BATCH_CONCURRENCY: usize = 4;

/// 「并发提交」关掉时，合并成一条请求的张数上限。
///
/// 勾掉并发的用户要的是「别一下子打太多请求」：小批量合并一条（上游原生 `n`，
/// 账单一行、原子成败）；大批量不能合并 —— 上游的 `n` 参数不是无限收的（官方
/// images API 单请求有上限，中转站各自截断）—— 只能逐张串行。
const SERIAL_MERGE_LIMIT: u32 = 4;

/// 批量生图的分单 + 并发度，由「并发提交」一个开关决定（用户勾选，App 内入口）：
///
/// | 开关 | 张数 | 形状 | 体感 |
/// |---|---|---|---|
/// | 并发（默认） | >1 | 全部拆单张，并发 [`BATCH_CONCURRENCY`] 跑 | 快；总耗时 ≈ 张数÷4 × 单张 |
/// | 串行 | ≤4 | **一条请求带 `n`** | 慢而稳：账单一行、原子成败、只占一个请求额度 |
/// | 串行 | >4 | 拆单张逐张发 | 慢而稳：永不并发，对站点最友好 |
///
/// MCP 入口不传开关，恒为并发 —— agent 想串行有自己的表达（逐次调用工具天然串行），
/// 不该为它加 schema 噪音。
fn split_batch(total: u32, parallel: bool) -> (Vec<u32>, usize) {
    if total <= 1 {
        return (vec![1], 1);
    }
    if parallel {
        (vec![1; total as usize], BATCH_CONCURRENCY)
    } else if total <= SERIAL_MERGE_LIMIT {
        (vec![total], 1)
    } else {
        (vec![1; total as usize], 1)
    }
}

/// 批量生图入口：分单 → 有界并发执行 → 汇总。
///
/// **部分失败不整体报错**：成功的图都已落盘、用户拿得到，返回值带失败张数，
/// 由调用方提示（App 内的 warning toast / MCP 的 text block）。全部失败才 `Err`
/// —— 那时把第一条错误原文带出去（生图失败的原因几乎全在服务端）。
pub(crate) async fn generate_batch(
    tier: &Tier,
    prompt: &str,
    size: Option<&str>,
    total: u32,
    parallel: bool,
) -> Result<(Vec<GeneratedImage>, usize), String> {
    let (batches, concurrency) = split_batch(total, parallel);
    let results: Vec<Result<Vec<GeneratedImage>, String>> = {
        use futures::stream::StreamExt as _;
        // 先 `.copied()` 再闭包：闭包参数是所有权 u32 而不是引用 —— 闭包借迭代项
        // 引用会撞上「closure 的 FnOnce 不够 general」的 HRTB 限制（编译器把签名
        // 统一成对任意生命周期成立的那种，async 块办不到）。
        futures::stream::iter(
            batches
                .iter()
                .copied()
                .map(|n| generate_image(tier, prompt, size, n, request_timeout(n))),
        )
        .buffered(concurrency)
        .collect()
        .await
    };

    let mut images = Vec::new();
    let mut failed = 0usize;
    let mut first_error = None;
    for (batch_size, result) in batches.iter().zip(results) {
        match result {
            Ok(mut saved) => images.append(&mut saved),
            Err(e) => {
                log::warn!("批量生图中一张（n={batch_size}）失败: {e}");
                failed += *batch_size as usize;
                first_error.get_or_insert(e);
            }
        }
    }
    if images.is_empty() {
        Err(first_error.unwrap_or_else(|| "生图接口没有返回任何图片".into()))
    } else {
        Ok((images, failed))
    }
}

/// 调一次生图（`n` 张），返回落盘后的文件（每张一个元素）。
///
/// `n` 由调用方给定并已过 [`validate_count`]：App 内入口来自生成视图的张数选择；
/// MCP 入口来自工具参数（宿主 agent 传，默认 1）。响应解析、落盘与修剪从第一天起
/// 就按「一次可能多张」处理，这里只是把入口接上。
pub(crate) async fn generate_image(
    tier: &Tier,
    prompt: &str,
    size: Option<&str>,
    n: u32,
    timeout: std::time::Duration,
) -> Result<Vec<GeneratedImage>, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败: {e}"))?;

    // ⚠️ **有意不发 `response_format`**：
    //
    // - 官方 `/v1/images/generations` 对 `gpt-image-*` 带这个字段**直接 400**
    //   （`Unknown parameter: 'response_format'` —— 它是给已下线的 `dall-e-*` 留的）。
    // - sub2api 把请求体**原样透传**给上游（只改 `model`，见其
    //   `rewriteOpenAIImagesModel`）⇒ 上游是 API-key 类账号时那个 400 会真的打回来。
    //
    // ⚠️ **本地测出 200 不能证明它安全**：调度器挑到 OAuth 类账号时该字段被丢弃，
    // 于是同一个档位在不同的调度结果下表现不同。不发它则两条路都对。
    //
    // ## 响应形态：`b64_json` 与 `url` 两种都要认（2026-09-05 实测修正）
    //
    // 官方 API 的 `gpt-image-*` 只回 `b64_json`，但**中转站不一定**：实测有 new-api
    // 站点对 `gpt-image-2` 回 `data[].url`（另一个模型才回 b64），其官方教程也明确
    // 「两种形态都可能出现、客户端都要兼容」。只认 b64 的解析在这种站点上必报
    // 「data 项里没有 b64_json」—— [`read_image_bytes`] 对两种形态都处理，url 走下载。
    let body = serde_json::json!({
        "model": tier.model,
        "prompt": prompt,
        "n": n,
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
        let bytes = read_image_bytes(&client, item).await?;
        let (ext, mime) = image_format(&bytes);

        // 文件名带序号与内容哈希 —— **不带时间戳**：同一秒内多张图会互相覆盖，
        // 内容哈希则天然唯一且可复现。扩展名来自魔数嗅探，不是写死 png
        // （url 变体可能下发 jpeg / webp）。
        let name = format!("gpt-image-{}-{idx}.{ext}", short_hash(&bytes));
        let path = dir.join(name);
        std::fs::write(&path, &bytes).map_err(|e| format!("写图片文件失败: {e}"))?;
        // 重新编码成 base64 —— MCP 入口要把它作为 image content block 回给宿主，
        // 让模型**真的看到图**而不只是拿到一个路径（App 内入口不消费这个字段）。
        saved.push(GeneratedImage {
            path,
            b64: base64_encode(&bytes),
            mime,
        });
    }

    if saved.is_empty() {
        return Err("生图接口没有返回任何图片".into());
    }
    // 顺手修剪 —— 见 `prune_old_images`。**把这次刚写的排除在外**：它们的 mtime 是最新的，
    // 正常不会被当成「最旧」删掉，但目录恰好满员时没有必要让「刚生成的图」参与这场竞争
    // （返回给调用方的路径必须还在）。失败只记一行：修剪不成功不影响这次出图。
    let just_written: Vec<&std::path::Path> = saved.iter().map(|i| i.path.as_path()).collect();
    if let Err(e) = prune_old_images(&dir, &just_written) {
        log::warn!("清理旧图片失败（不影响本次生成）: {e}");
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
/// 按 mtime 排序删最旧的。读不到 mtime 的排最前（当最旧）—— 那种文件多半是异常留下的。
///
/// ## 并发是安全的
///
/// codex 与 claude 各起一个 MCP 进程、主程序又可能同时直接生图，多方可能同时修剪。
/// 这不会互相删掉对方的图：判据是 mtime，而另一方**刚写的图 mtime 是最新的**，
/// 排在末尾。删不掉的（已被对方删了）只记一行，不当错误 —— 修剪是收尾动作，
/// 不该影响出图。
///
/// `keep` 是本次调用刚写的那些，一律排除（见调用处）。
fn prune_old_images(dir: &std::path::Path, keep: &[&std::path::Path]) -> Result<(), String> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("读出图目录失败: {e}"))?
        .filter_map(Result::ok)
        // 只管图片 —— 修剪的本意是「这些产物会自己堆积」，目录里若有用户手放的
        // 其它文件不该被动到。（曾经只认 `.png`：url 变体会落 jpg / webp，
        // 那些就永远不参与修剪、无限堆积 —— 已修。）
        .filter(|e| is_image_file(&e.path()))
        // 这次刚写的不参与 —— 调用方要把它们的路径返回出去。
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
            log::debug!("删不掉旧图片 {}: {e}", path.display());
        }
    }
    log::debug!("出图目录已修剪：删掉 {excess} 张最旧的，保留 {MAX_KEPT_IMAGES} 张");
    Ok(())
}

/// 出图目录里一张图的元数据（画廊列表条目）。
///
/// 只报事实（路径、大小、mtime），格式由扩展名推 —— 列表不读文件内容。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GalleryImage {
    pub name: String,
    pub path: String,
    /// 由扩展名推出的 mimeType，展示与 <img> 用。
    pub mime: String,
    pub size_bytes: u64,
    /// mtime 的 Unix 秒。
    pub modified_at: i64,
}

/// 扫描出图目录，按 mtime 从新到旧返回（画廊顺序）。
///
/// 目录不存在视为空画廊，不是错误 —— 没生成过图是常态。
/// 只认 [`is_image_file`]（我们自己的命名）—— 存储路径指到用户目录时，
/// 他的文件不进画廊。
pub(crate) fn gallery_images() -> Vec<GalleryImage> {
    let dir = output_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut items: Vec<GalleryImage> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image_file(p))
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let modified_at = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default();
            let name = path.file_name()?.to_string_lossy().to_string();
            let ext = path.extension()?.to_string_lossy().to_lowercase();
            Some(GalleryImage {
                name,
                path: path.to_string_lossy().to_string(),
                mime: mime_from_extension(&ext).to_string(),
                size_bytes: meta.len(),
                modified_at,
            })
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified_at));
    items
}

#[cfg(feature = "gui")]
/// 把出图目录加进 asset 协议的运行时白名单，前端画廊才能用 `convertFileSrc` 显示。
///
/// 静态 scope（tauri.conf.json）写不了这里：出图目录跟着「LoongPort 配置目录」
/// 覆盖走，用户可以指到任意路径，只有运行时知道。幂等，重复调用无害；
/// 失败只记日志 —— 画廊显示不出来不该把生图本身也拖死。
pub(crate) fn ensure_asset_scope(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Err(e) = app
        .asset_protocol_scope()
        .allow_directory(output_dir(), true)
    {
        log::warn!("把出图目录加进 asset 协议白名单失败: {e}");
    }
}

/// 扩展名 → mimeType（与 [`image_format`] 嗅探出的集合一致）。
fn mime_from_extension(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
}

/// 换存储位置时把旧目录里**我们生成的图**搬到新目录（一次性迁移）。
///
/// - 只搬 [`is_image_file`] 认的文件：旧目录若是用户自己的目录，他的东西不动。
/// - 幂等：目标已有同名文件（内容寻址命名 ⇒ 同名即同内容）就跳过 ——
///   中途失败重试不撞名、不重复占位；迁移发生在写设置**之前**，失败则设置不变。
/// - 跨盘（如 C:→D:）`rename` 会失败，回落 copy + remove（对任何 rename 失败
///   形态都安全）。
/// - 返回实际搬过去的张数。
pub(crate) fn migrate_images(from: &Path, to: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(to).map_err(|e| format!("创建新目录失败: {e}"))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("读取旧目录失败: {e}"))?;
    let mut moved = 0usize;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_image_file(&path) {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = to.join(name);
        if dest.exists() {
            continue;
        }
        if std::fs::rename(&path, &dest).is_err() {
            std::fs::copy(&path, &dest).map_err(|e| format!("复制图片失败: {e}"))?;
            std::fs::remove_file(&path).map_err(|e| format!("删除旧图失败: {e}"))?;
        }
        moved += 1;
    }
    Ok(moved)
}

/// 这个文件是不是 **LoongPort 生成的图**：认 [`output_dir`] 里我们自己写的
/// 内容寻址命名（`gpt-image-<12 位 hex>-<序号>.<图片扩展名>`，见 `generate_image`
/// 的落盘处），不认「任何图片文件」。
///
/// ## 为什么必须按名字、不能按扩展名（自定义存储路径的安全前提）
///
/// 修剪（删最旧）和画廊都吃这个判据。用户一旦把存储路径指到**自己的目录**
/// （比如放照片的文件夹），按扩展名过滤的修剪会把他自己的图当成「最旧的」
/// **删掉**，画廊也会混进他的私人文件。按命名过滤则只认我们的产物：
/// 用户目录里自己的文件永远不进画廊、永远不会被修剪碰到。
fn is_image_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif"
    ) {
        return false;
    }
    let Some((prefix, index)) = stem.rsplit_once('-') else {
        return false;
    };
    let Some(hash) = prefix.strip_prefix("gpt-image-") else {
        return false;
    };
    hash.len() == 12
        && hash.chars().all(|c| c.is_ascii_hexdigit())
        && !index.is_empty()
        && index.chars().all(|c| c.is_ascii_digit())
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("图片 base64 解码失败: {e}"))
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 一个 `data` 项 → 图片字节：`b64_json` 直接解码；`url` 变体下载
/// （new-api 站点实测会对 `gpt-image-*` 回 url，见 `response_format` 那段注释）。
/// 下载走的是**返回 url 的那个站点自己**或它的对象存储 —— 预签名链接，不带鉴权头
/// （OpenAI images API 的 url 模式就是这个约定）。
async fn read_image_bytes(client: &reqwest::Client, item: &Value) -> Result<Vec<u8>, String> {
    if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
        return base64_decode(b64);
    }
    let url = item
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| "生图响应的 data 项里没有 b64_json 或 url".to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载生图结果失败: {e}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("读取生图结果失败: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "下载生图结果失败（HTTP {}）：{}",
            status.as_u16(),
            url
        ));
    }
    if bytes.is_empty() {
        return Err("生图结果下载下来是空文件".into());
    }
    Ok(bytes.to_vec())
}

/// 魔数嗅探图片格式 →（文件扩展名， mimeType）。认不出按 png ——
/// 生图端点的事实默认就是 png，错标也只是视图按扩展名猜的问题。
pub(crate) fn image_format(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ("png", "image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        ("jpg", "image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ("webp", "image/webp")
    } else if bytes.starts_with(b"GIF8") {
        ("gif", "image/gif")
    } else {
        ("png", "image/png")
    }
}

/// 内容哈希的前 12 位 hex，用作文件名。
fn short_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())[..12].to_string()
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ **settings.json 的路径必须与主程序一致。**
    ///
    /// 读设备级「当前生图档位」是**自己拼路径读文件**（不能复用
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
            "主程序的 settings.json 路径拼法变了 —— 生图那份手抄的跟着改，\
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

    /// 超时随张数线性放大（App 内入口的封顶），n=0 兜底按一张算 —— 不是 0 秒必超时。
    /// 两个入口（App 内 / MCP）都按张数放大；n=1 时即那条「给 codex 300s 留 60s 余量」
    /// 的基线 240s。
    #[test]
    fn request_timeout_scales_with_the_image_count() {
        assert_eq!(request_timeout(1).as_secs(), 240);
        assert_eq!(request_timeout(4).as_secs(), 960);
        assert_eq!(
            request_timeout(0).as_secs(),
            240,
            "n=0 必须兜底成一张的预算，否则是个 0 秒必超时的请求"
        );
    }

    /// 张数闸：合法原样返回、越界报错。两条入口共用这一份判据 ——
    /// 它分叉的那天，App 内和 MCP 会各自接受不同的张数。
    #[test]
    fn validate_count_is_the_single_range_gate() {
        assert_eq!(validate_count(1).unwrap(), 1);
        assert_eq!(validate_count(4).unwrap(), 4);
        assert_eq!(validate_count(50).unwrap(), 50);
        for bad in [0, 51, 500] {
            assert!(validate_count(bad).is_err(), "n={bad} 必须被拒");
        }
    }

    /// 拆单规则（「并发提交」开关的完整语义，见 `split_batch` 的表）：
    /// 单张无差别；并发=全拆单张、并发度 4；串行小批=合并一条（原生 n）；
    /// 串行大批=逐张（上游不收大 n）。
    #[test]
    fn split_batch_follows_the_parallel_switch() {
        // 单张：两种模式无差别。
        assert_eq!(split_batch(1, true), (vec![1], 1));
        assert_eq!(split_batch(1, false), (vec![1], 1));
        // 并发：>1 全拆单张、并发度 4。
        assert_eq!(split_batch(3, true), (vec![1; 3], 4));
        assert_eq!(split_batch(50, true), (vec![1; 50], 4));
        // 串行小批：合并成一条请求（账单一行、原子成败）。
        assert_eq!(split_batch(4, false), (vec![4], 1));
        // 串行大批：不能合并（上游不收），逐张串行。
        assert_eq!(split_batch(5, false), (vec![1; 5], 1));
        assert_eq!(split_batch(50, false), (vec![1; 50], 1));
    }

    /// 只认自己写的内容寻址命名（`gpt-image-<12hex>-<序号>.<图片扩展名>`）——
    /// 自定义存储路径的安全前提：用户的文件永远不进画廊、不被修剪碰到。
    #[test]
    fn is_image_file_only_recognizes_our_content_addressed_names() {
        assert!(is_image_file(Path::new("/x/gpt-image-0123456789ab-0.png")));
        assert!(is_image_file(Path::new("gpt-image-abcdef123456-12.webp")));
        // 用户的图 / 命名形状不对的都不是我们的。
        assert!(!is_image_file(Path::new("vacation.png")));
        assert!(!is_image_file(Path::new("gpt-image-short-0.png")));
        assert!(!is_image_file(Path::new("gpt-image-0123456789ab-x.png")));
        assert!(!is_image_file(Path::new("gpt-image-0123456789ab.png")));
        assert!(!is_image_file(Path::new("gpt-image-0123456789ab-0.txt")));
    }

    /// 迁移只搬我们的图、用户的文件留原地，且幂等（同名跳过，重试不撞）。
    #[test]
    fn migrate_moves_only_our_named_files_and_is_idempotent() {
        let base = std::env::temp_dir().join(format!("lp-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let from = base.join("from");
        let to = base.join("to");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("gpt-image-0123456789ab-0.png"), b"a").unwrap();
        std::fs::write(from.join("gpt-image-abcdef123456-1.webp"), b"b").unwrap();
        std::fs::write(from.join("vacation.png"), b"mine").unwrap();
        std::fs::write(from.join("notes.txt"), b"x").unwrap();

        assert_eq!(migrate_images(&from, &to).unwrap(), 2);
        assert!(to.join("gpt-image-0123456789ab-0.png").exists());
        assert!(!from.join("gpt-image-0123456789ab-0.png").exists());
        assert!(
            from.join("vacation.png").exists() && from.join("notes.txt").exists(),
            "用户的文件必须留在原地"
        );

        // 幂等：旧目录再出现同名文件（中途失败的残局）时目标已有同名 ⇒ 跳过。
        std::fs::write(from.join("gpt-image-0123456789ab-0.png"), b"a").unwrap();
        assert_eq!(migrate_images(&from, &to).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// ⭐ `imagegenOutputDir` 这个手抄的 JSON 键必须与 `AppSettings` 字段对得上
    /// （serde camelCase 派生）。抄错 ⇒ 自定义路径永远读不到，图悄悄落回默认目录，
    /// 而 App 与 MCP 两边一致地错 —— 没有任何东西会报错。
    #[test]
    fn the_output_dir_key_matches_the_settings_field() {
        let settings_rs = include_str!("../settings.rs");
        assert!(
            settings_rs.contains("pub imagegen_output_dir: Option<String>"),
            "`AppSettings::imagegen_output_dir` 改名了 —— `custom_output_dir` 里那个 \
             camelCase 键名跟着改"
        );
    }

    /// 同一份内容得到同一个名字（可复现），不同内容不撞名。
    #[test]
    fn file_names_are_content_addressed() {
        assert_eq!(short_hash(b"abc"), short_hash(b"abc"));
        assert_ne!(short_hash(b"abc"), short_hash(b"abd"));
        assert_eq!(short_hash(b"abc").len(), 12);
    }

    /// 魔数嗅探：扩展名与 mimeType 同源，认不出回落 png。
    #[test]
    fn image_format_sniffs_magic_bytes() {
        assert_eq!(
            image_format(&[0x89, b'P', b'N', b'G', 0x00]),
            ("png", "image/png")
        );
        assert_eq!(
            image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            ("jpg", "image/jpeg")
        );
        assert_eq!(
            image_format(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            ("webp", "image/webp")
        );
        assert_eq!(image_format(b"GIF89a"), ("gif", "image/gif"));
        // 认不出的按 png —— 生图端点的事实默认。
        assert_eq!(image_format(b"\x00\x01\x02"), ("png", "image/png"));
    }

    /// 画廊条目的 mimeType 与扩展名一致（写文件用嗅探、列表用扩展名，
    /// 两边对不上图就显示不出来）。
    #[test]
    fn gallery_mime_matches_the_extension_image_format_writes() {
        for ext in ["png", "jpg", "webp", "gif"] {
            let (_, written_mime) = image_format(match ext {
                "png" => &[0x89u8, b'P', b'N', b'G'] as &[u8],
                "jpg" => &[0xFFu8, 0xD8, 0xFF] as &[u8],
                "webp" => b"RIFF\x00\x00\x00\x00WEBP" as &[u8],
                _ => b"GIF89a" as &[u8],
            });
            assert_eq!(mime_from_extension(ext), written_mime, "ext={ext}");
        }
    }

    /// **url 变体必须能出图**（2026-09-05 实测踩中：new-api 站点对 gpt-image-2 回
    /// `data[].url`，只认 b64_json 的解析直接报「没有 b64_json」）。
    ///
    /// 伪服务就回一张 1×1 JPEG 的 url；同时钉住「两种形态都没有」的报错文案。
    #[tokio::test]
    async fn url_shaped_image_responses_are_downloaded_and_decoded() {
        // 1×1 像素 JPEG（合法魔数 FFD8FF）。
        const JPEG: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
        ];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let origin = format!("http://{}", listener.local_addr().expect("addr"));
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let (mut socket, _) = listener.accept().await.expect("accept");
            // 读完请求头（读到空行为止）再回 —— HTTP/1.1 的规矩。
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let n = socket.read(&mut chunk).await.expect("read request");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                JPEG.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, head.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, JPEG).await;
        });

        let client = reqwest::Client::new();
        let item: Value = serde_json::json!({ "url": format!("{origin}/img.jpg") });
        let bytes = read_image_bytes(&client, &item)
            .await
            .expect("url 变体必须下载成功");
        assert_eq!(bytes, JPEG);
        assert_eq!(image_format(&bytes), ("jpg", "image/jpeg"));
        server.abort();

        // b64 分支不受影响，两种形态都缺失时报错点名两者。
        let b64_item: Value = serde_json::json!({ "b64_json": base64_encode(b"png!") });
        assert_eq!(
            read_image_bytes(&client, &b64_item)
                .await
                .expect("b64 分支照旧"),
            b"png!"
        );
        let empty: Value = serde_json::json!({ "revised_prompt": "x" });
        let error = read_image_bytes(&client, &empty)
            .await
            .expect_err("必须报错");
        assert!(error.contains("b64_json"), "{error}");
        assert!(error.contains("url"), "{error}");
    }
}
