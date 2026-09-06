//! 前端行/档位/站点的读侧命令与行级维护（排序、恢复默认、强删、安置）。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::provision;

/// 一个已添加站点的后端摘要。
///
/// 当前消费者是「一个站都没有吗」的自动引导判据（只数条数）。新增站点只有在
/// 注册或登录成功后才会写入；启动建表路径也会清理旧版遗留的未认证占位行。
///
/// 2026-08-04 一并收缩：原来还有 `id` / `site_name` / `label` / `logged_in` /
/// `is_current` 五个字段，服务的是已删独立页顶部那个**站点切换器**（要显示名、
/// 要标出当前选中的是哪个、要能按 id 切换）。那个控件删了之后没有消费者。
///
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteInfo {
    pub site_origin: String,
    /// 该站已有几个后端已识别身份的账号。未登录占位行不计入。
    pub account_count: usize,
}

/// 一个可选的档位。
///
/// `group_id` / `rate_multiplier` 是 `Option`：列表命令从本地 DB 读，而倍率只在 provision
/// 时从服务端拿到。**用 `Option` 而不是填 0 占位** —— 0 倍率意味着"免费"，UI 会把它显示成
/// 最便宜的一档，那是错的。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierInfo {
    pub provider_id: String,
    /// 这个档位落在哪个 CLI 上（`AppType::as_str()`，如 `"codex"` / `"claude"`）。
    ///
    /// ## 为什么必须有它
    ///
    /// provision 链路（[`refresh_relay_provision`]）一次探**全部平台**，返回的 `tiers`
    /// 是全平台的，而 UI 那一行只显示当前 app 的档位。没有这个字段，前端拿到一堆档位
    /// 却分不出哪条是自己的 ⇒ 「这个站没有该平台的分组」与「拉取失败」在界面上长得一样
    /// （都是零档位），而前者重试一百次也不会有、后者重试有意义。
    ///
    /// [`list_tiers_impl`] 那条路填的是它被查询的那个 app（那条命令按 app 查，
    /// 结果天然同质），所以两条路的语义一致：**这条档位属于哪个 CLI**。
    pub app_id: String,
    pub group_name: String,
    pub display_name: String,
    /// The model currently written into this provider's Codex config.
    pub model: String,
    /// Model ids discovered from this tier's `/v1/models` endpoint.
    /// An empty list means no complete remote catalog is available.
    pub models: Vec<String>,
    pub rate_multiplier: Option<f64>,
    pub is_current: bool,
    /// 后端是否支持对这个档位执行模型验证。
    pub can_verify_models: bool,
    /// 用户在 cc-switch 编辑页改过这个档位的配置吗。
    ///
    /// 判据是**存库标记** `providers.user_edited`（编辑页置位、恢复默认复位），
    /// 不是内容比对 —— 手动改到和默认一样也仍算「已手工维护」。
    ///
    /// `None` = 读标记失败（防御）。UI 在 `None` 时
    /// 什么标记都不显示：`false` 是在断言「刷新不会覆盖你的改动」，
    /// 而事实是「不知道」—— 让用户误信比不说更糟。
    ///
    /// ⚠️ 只有 [`list_relays_impl`] 填得出它（判据要 `api_base_url`，那在
    /// `creds` 里按站点存）。[`relay_list_tiers`] 那条路恒为 `None` ——
    /// 它的调用方不显示这个标记，见该命令的文档。
    pub user_edited: Option<bool>,
    /// 服务端说这个分组允许生图（`allow_image_generation`）。
    ///
    /// ⚠️ **纯生图分组不靠这个字段识别** —— 它们在 `codex-image` 那一栏，
    /// 所在的列表本身就说明了这件事（见 [`provision::image_tier_app_type`]）。
    /// 这个字段的价值在**混合分组**：实测 `pro池` 这类有文本模型的分组也是 `true`，
    /// 它们留在 codex 栏而同时支持生图。
    ///
    /// `None` = **判不了**：这是纯服务端信息（分组的开关），本地配置里没有它。
    /// 只有 provision 那条路填得出，[`list_relays_impl`] 恒为 `None`。
    /// UI 在 `None` 时不显示标记 —— 与 `user_edited` 同一条原则：不知道就别断言。
    pub allow_image_generation: Option<bool>,
    /// 这个档位的调用参数里应用了站长自报声明（`relay/site_config.rs`）的站点
    /// origin。非空 = UI 显示「站点推荐配置」标注；`None` = 纯内置默认。
    /// 与 `user_edited` 不同，它是**存库事实**（meta.siteDeclaredOrigin），
    /// 不是现算判据。
    pub site_declared_origin: Option<String>,
}

/// 「中转站 × 分组」页的一行中转站，连带它在当前 app 下的档位。
///
/// spec §三 定的是 `RelayRow { ..., tiers: Vec<TierRow> }`，这里**复用已有的
/// [`TierInfo`] 而不新建 `TierRow`** —— 两者字段本就一致（含那个关键的
/// `rate_multiplier: Option<f64>`），再建一个只会让同一个概念有两种形状，
/// 前端也得写两套类型（CLAUDE.md §一：能复用就复用）。
///
/// **它是只读本地的**，与 [`RelayStatus`] 的首屏契约一致（不发网络请求）——
/// 所以 `tiers` 里的 `rate_multiplier` 恒为 `None`，倍率要等用户主动刷新（provision）
/// 才有值。这不是缺陷：填 0 占位会让 UI 显示成「最便宜的一档」，那是错的。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayRow {
    pub id: i64,
    pub site_origin: String,
    pub site_name: String,
    /// 登录后的账号名（昵称优先，回落邮箱），未登录为空串。
    /// 同一个站可以挂多个账号，所以「登录了」不够 —— 得说清是**哪个**账号。
    pub account_label: String,
    /// 后端根据当前行的所有托管档位计算出的展示状态。
    pub status: RelayRowStatus,
    /// 当前 app 下是否有档位正在使用。
    pub is_current: bool,
    /// 这一行是否具备余额查询所需的凭据（有效登录态或至少一把托管 SK）。
    pub can_query_balance: bool,
    /// 后端是否确认这条账号行可以打开签名目录配置的购买入口。
    pub can_purchase: bool,
    /// 「查看用量」入口资格：`usage_url` 已配置且登录态满足开窗条件（与
    /// `can_purchase` 同一判据，只是查另一个入口字段）。
    pub can_view_usage: bool,
    /// 这一行是否可以重新拉取最新账号信息、额度、可用分组与倍率。
    pub can_refresh: bool,
    /// 这一行名下**正被某个 app 当作当前项**的档位。空 = 可直接删；
    /// 非空 = 前端删除弹窗换成「点名 app」的强删变体，确认后带 `force` 重删。
    ///
    /// ⚠️ 这里与 [`relay_remove_site`] 的闸是**同一次扫描**（`apps_using_this_accounts_tiers`），
    /// 不再另设一个 `can_delete: bool` —— 那会让「能不能删」与「谁在用」两个表述分叉
    /// （尺子 1.4）：前者前端自己 `is_empty()` 派生即可。
    pub usage_blockers: Vec<UsageBlocker>,
    /// 删除确认框使用哪一种后端定义的业务语义。
    pub remove_confirmation: RemoveConfirmation,
    /// 这个中转站在**当前 app_id** 下已备好的档位。
    pub tiers: Vec<TierInfo>,
}

/// `RelayRow::usage_blockers` 的元素：哪个 app 正在把哪条档位当当前项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBlocker {
    /// `AppType::as_str()`，与前端 `AppId` 同一套字符串（前端拿它查显示名）。
    pub app: String,
    /// 正在被当作当前项的那条档位的名字。
    pub tier_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoveConfirmation {
    NeverLoggedIn,
    Configured,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RelayRowStatus {
    NotLoggedIn,
    SessionExpired,
    SessionExpiredUsable,
    NoTiers,
    Ready,
}

/// 「中转站 × 分组」页的数据源：一次返回渲染整页所需的全部内容。
///
/// **只读本地，不发网络请求**（spec §三）—— 与 [`relay_status`] 的首屏契约一致。
/// 代价是 `rate_multiplier` 恒为 `None`，要等用户主动 provision 才有值；
/// 那是有意的取舍，首屏不该卡在网络上。
///
/// `app` 决定读哪个 app_type 下的 provider（前端本来就知道当前是哪个 tab）。
#[tauri::command]
pub fn relay_list_relays(state: State<'_, AppState>, app: String) -> Result<Vec<RelayRow>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    list_relays_impl(state.inner(), app_type).map_err(|e| e.to_string())
}

/// 「这一行能开带登录态的站点窗吗」——充值与查看用量共用同一判据
/// （配置了入口 + 登录态有效 + NewAPI 还要有可轮换的 refresh cookie）。
pub(crate) fn can_open_site_window(
    relay: &creds::Relay,
    logged_in: bool,
    configured_url: bool,
) -> bool {
    configured_url
        && logged_in
        && match relay.backend_kind {
            creds::BackendKind::Sub2Api => true,
            creds::BackendKind::NewApi => relay
                .refresh_token
                .as_deref()
                .is_some_and(|refresh_cookie| !refresh_cookie.trim().is_empty()),
        }
}

pub(crate) fn list_relays_impl(
    state: &AppState,
    app_type: AppType,
) -> Result<Vec<RelayRow>, AppError> {
    let relays = with_conn(state, creds::list)?;
    // 一次读全量再在内存里按站分组，而不是对每个站各查一次 —— 站点通常 1-5 个，
    // 而 ProviderService::list 每次都要解一遍 settings_config 的 JSON。
    // `app_type` 下面在闭环里要按站点各用一次（判「用户改过配置没有」），
    // 而它没派生 Copy（上游结构，别为此改它）⇒ 先 clone 一份给 `list_tiers_impl`。
    let tiers = list_tiers_impl(state, app_type.clone())?;
    // 签名目录只读一次；逐行只做纯解析，避免重复验签与磁盘读取。
    let signed_config = remote_config::load_cached().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();

    relays
        .into_iter()
        .map(|op| -> Result<RelayRow, AppError> {
            let mine = tiers_of_site(state, &tiers, &op.site_origin, op.account_id, &app_type)?;
            let logged_in = op.token_looks_valid(now);
            let session_expired = op.session_expired(now);
            let has_balance_key = !relay_balance_inputs(state, &op).1.is_empty();
            let status = if session_expired {
                if has_balance_key {
                    RelayRowStatus::SessionExpiredUsable
                } else {
                    RelayRowStatus::SessionExpired
                }
            } else if !logged_in && !op.can_refresh(now) {
                RelayRowStatus::NotLoggedIn
            } else if mine.is_empty() {
                RelayRowStatus::NoTiers
            } else {
                RelayRowStatus::Ready
            };
            let configured_url =
                match remote_config::configured_purchase_url(&signed_config, &op.site_origin) {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(_) => {
                        let host = url::Url::parse(&op.site_origin)
                            .ok()
                            .and_then(|url| url.host_str().map(str::to_owned))
                            .unwrap_or_else(|| "<unknown>".into());
                        log::warn!("中转站 {host} 的购买入口配置无效，已禁用购买");
                        false
                    }
                };
            let usage_url_configured =
                match remote_config::configured_usage_url(&signed_config, &op.site_origin) {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(_) => {
                        let host = url::Url::parse(&op.site_origin)
                            .ok()
                            .and_then(|url| url.host_str().map(str::to_owned))
                            .unwrap_or_else(|| "<unknown>".into());
                        log::warn!("中转站 {host} 的用量入口配置无效，已禁用查看用量");
                        false
                    }
                };
            let usage_blockers =
                apps_using_this_accounts_tiers(state, &op.site_origin, op.account_id)
                    .into_iter()
                    .map(|(app_type, tier_name)| UsageBlocker {
                        app: app_type.as_str().to_string(),
                        tier_name,
                    })
                    .collect::<Vec<_>>();
            Ok(RelayRow {
                id: op.id,
                site_origin: op.site_origin.clone(),
                site_name: op.site_name.clone(),
                // 有 account_id 才算真的认得这个账号 —— email 可能被中转站留空。
                account_label: if op.account_id.is_some() {
                    op.account_label.clone()
                } else {
                    String::new()
                },
                status,
                is_current: mine.iter().any(|tier| tier.is_current),
                can_query_balance: logged_in || has_balance_key,
                can_purchase: can_open_site_window(&op, logged_in, configured_url),
                can_view_usage: can_open_site_window(&op, logged_in, usage_url_configured),
                can_refresh: op.can_refresh(now),
                usage_blockers,
                remove_confirmation: if op.account_id.is_some() {
                    RemoveConfirmation::Configured
                } else {
                    RemoveConfirmation::NeverLoggedIn
                },
                tiers: mine,
            })
        })
        .collect()
}

/// 把一个托管档位的配置**恢复成默认值**。
///
/// ## 为什么需要这个命令
///
/// 编辑走 cc-switch 现成的编辑页（那页支持全部字段，我们不重做）。代价是用户可能改坏 ——
/// 改错 base_url、删掉 `disable_response_storage`、把 `model_provider` 从 `custom` 改成
/// `OpenAI`（那会让会话历史分家）。这些改动都不会报错，只会让调用静默失败。
///
/// 所以给一条回头路。**它是唯一会重写用户编辑的入口** —— 重复 provision 不再覆盖
/// （见 `persist_provision_batch` 里那段），改配置的责任明确落在用户显式点这个按钮上。
///
/// **sk 保留不变**：从现有配置里读出来再塞回去。恢复默认是「修配置」不是「换密钥」，
/// 顺手换掉 sk 会让用户的其它设备上那把 key 失效（虽然认领逻辑会重新拿到，
/// 但那是多余的服务端写操作）。sk 读不出来时（配置被改得面目全非）**明确报错**，
/// 让用户走「获取密钥」重建 —— 不静默生成一份没有 sk 的配置。
#[tauri::command]
pub async fn relay_reset_tier_config(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    reset_tier_config_impl(&app_handle, &provider_id, app_type)
        .await
        .map_err(|e| e.to_string())
}

async fn reset_tier_config_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
) -> Result<(), AppError> {
    let state = app_handle.state::<AppState>();
    reset_tier_config_in_state(state.inner(), provider_id, app_type)
}

pub(crate) fn reset_tier_config_in_state(
    state: &AppState,
    provider_id: &str,
    app_type: AppType,
) -> Result<(), AppError> {
    // 只对托管档位有效 —— 用户自建的 provider 没有「默认配置」这个概念。
    // 用正向判据 `is_managed`，不要拿 `reject_if_managed` 的 Err 反着判 ——
    // 那个函数的语义是「撞到托管项就拦下」（给通用命令用），这里要的恰好相反
    // （只对托管项生效），借它的错误来表达「是托管的」会让代码反着读。
    if !crate::relay::is_managed(provider_id) {
        return Err(AppError::Config(
            "只有 LoongPort 托管的档位才能恢复默认配置".into(),
        ));
    }

    let verification_scope =
        crate::relay::model_verification::types::TargetScope::new(provider_id, app_type.as_str());
    state.model_verification.cancel_scope(&verification_scope);
    let existing = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())
        .map_err(|e| AppError::Database(format!("读取档位失败: {e}")))?
        .ok_or_else(|| AppError::Config("这个档位不存在".into()))?;

    // ⚠️ **中转站必须按这个档位自己的归属取，绝不能用 `creds::load()`**（review 抓出的 P0）。
    //
    // `creds::load` 返回的是**全局「当前站」**（`ORDER BY is_current DESC LIMIT 1`），而
    // 分组页把所有中转站并列显示 —— 用户展开 B 站那一行、点它某个档位的「恢复默认配置」时，
    // 拿到的会是 A 站的 `api_base_url`，于是那个档位被写成「B 的 sk + A 的端点」⇒
    // **每次调用都 401**，而界面显示恢复成功。「恢复默认」恰恰是用户在档位坏了时点的按钮，
    // 那等于让它把自己要修的问题弄得更糟。
    //
    // `website_url` 是档位归属的唯一可靠依据（provision 时写入，见 `:799`；
    // `prune_stale_tiers` 也是靠它认主人）—— `provider_id` 是
    // `sha256(site_origin + group_id)`，单向不可逆，反推不出属于哪个站。
    //
    // 这是 `b2400000`「中转站之间彻底解耦」那一轮的漏网之鱼：这条命令写在那之前
    // （`ea2a32b7`），保留了「靠全局当前站定位」的旧写法，而本轮给它接上 UI 入口
    // 才让这个潜在缺陷变得可达。
    let site_origin = existing
        .website_url
        .as_deref()
        .ok_or_else(|| {
            AppError::Config(
                "这个档位没有记录它属于哪个中转站，请用「获取密钥」重新生成它。".into(),
            )
        })?
        .to_string();

    // ⚠️ **必须连账号一起认**，不能只按站点取第一个匹配的行 ——
    // 同一个站可以挂多个账号，取错了就是「用 A 账号的凭据重建 B 账号的档位」，
    // 而那把 sk 属于 A ⇒ 用户拿到一条指向错误账号的配置（还会算错账）。
    //
    // 归属记在 `meta.loongportAccountId`（provision 时写入）。
    // `None` = 旧数据：那时只能按站点回落，且**只在该站只有一行时**才敢用 ——
    // 有多行还猜就是重演这个 bug。
    let account_id = existing.meta.as_ref().and_then(|m| m.loongport_account_id);
    let candidates: Vec<_> = with_conn(state, creds::list)?
        .into_iter()
        .filter(|candidate| {
            same_site_identity(Some(&candidate.site_origin), Some(site_origin.as_str()))
        })
        .collect();
    let op = match account_id {
        Some(want) => candidates
            .into_iter()
            .find(|candidate| candidate.account_id == Some(want))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "这个档位属于 {site_origin} 上的某个账号，但那个账号已经不在列表里了。\
                     重新登录它、或者直接删掉这个档位。"
                ))
            })?,
        // 旧数据没有账号标记。
        None if candidates.len() == 1 => candidates.into_iter().next().expect("刚判过只有一个"),
        None if candidates.is_empty() => {
            return Err(AppError::Config(format!(
                "这个档位属于 {site_origin}，但那个中转站已经不在列表里了。\
                 重新添加它、或者直接删掉这个档位。"
            )))
        }
        // 该站有多个账号，而这条档位没记归属 ⇒ **不猜**。
        // 猜错的后果（用错账号的 sk 重建）比让用户重新生成一次糟得多。
        None => {
            return Err(AppError::Config(format!(
                "这个档位没有记录属于 {site_origin} 上的哪个账号，而那个站现在挂着多个账号。\
                 请用「获取密钥」重新生成它 —— 那会带上账号归属。"
            )))
        }
    };

    // sk 从现有配置里取。取不到就让用户走「获取密钥」重建 —— 生成一份没有 sk 的
    // 「默认配置」比保持现状更糟（那是一条必定 401 的记录）。
    let api_key =
        provision::extract_api_key(&existing.settings_config, &app_type).ok_or_else(|| {
            AppError::Config("这个档位的配置里读不出密钥了，请用「获取密钥」重新生成它。".into())
        })?;

    // 带模型目录的档位（唯一源 [`provision::model_catalog_apps`]）：远端目录已经
    // 存进 `modelCatalog`，恢复默认时按同一份目录重新挑默认模型，并保留目录本身。
    // 否则这个动作会把刚外露的模型列表清空，还可能把不支持 `DEFAULT_MODEL` 的分组
    // 重置成一条选中即 404 的配置。claude / gemini 的角色分档（roles）是官网直连
    // 才有的概念、中转站档位恒 `None`（`settings_config_with_models` 内部就是走
    // `roles = None`），所以这里统一用 [`provision::settings_config_with_models`]。
    let catalog_models = if provision::supports_model_catalog(&app_type) {
        models_from_settings(&existing.settings_config)
    } else {
        Vec::new()
    };

    // ⚠️ **生图档位要保住它自己的模型名**（review 抓出）。
    //
    // 这条路拿不到分组数据（手上只有本地 `settings_config`），所以原来无条件写
    // `DEFAULT_MODEL` —— 那会把纯生图档位重置成一个**必定 404** 的形状，
    // 即「恢复默认」这个专门用来救砖的按钮，反过来把生图档位弄砖。
    //
    // 判据用配置里现有的模型名：是 `gpt-image-*` 就留着（那个值本就是这个档位的正解，
    // 由 `provision::pick_model` 按服务端的模型列表定），否则回落 `DEFAULT_MODEL`。
    //
    // 这**不是**「保留用户改的模型名」—— 用户把模型改成任何文本模型时仍然会被重置成
    // 默认值，那正是这个按钮该做的事。
    let model = if catalog_models.is_empty() {
        provision::extract_model(&existing.settings_config)
            .filter(|m| provision::is_image_model(m))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    } else {
        provision::pick_tier_models(&app_type, Some(&catalog_models)).main
    };
    let base_url = api::base_url_for(&app_type, &op.site_origin, &op.api_base_url);

    let settings_config = if !catalog_models.is_empty() {
        provision::settings_config_with_models(
            &app_type,
            &api_key,
            &existing.name,
            &base_url,
            &model,
            Some(&catalog_models),
        )
    } else {
        provision::settings_config_for(&app_type, &api_key, &existing.name, &base_url, &model)
    }
    .ok_or_else(|| {
        AppError::Config(format!(
            "还不能为 {} 生成默认配置（`settings_config_for` 里没有它的形状）。",
            app_type.as_str()
        ))
    })?;

    // 除 settings_config 外其余字段保持原样（sort_index / created_at 等都不该被重置）。
    //
    // `managed_meta` 传 `op.account_id` 而不是上面那个 `account_id` ——
    // 后者可能是 `None`（旧数据），而这次重建正好是**补上归属标记**的时机：
    // 我们刚刚确认了它属于 `op` 这一行。
    //
    // 分组身份同样趁重建补上——但这条路拿不到分组数据（手上只有本地
    // `settings_config`），只能保留旧值；旧值也是 `None` 时维持 `None`
    // （能力判定按 Unknown 处理，不排除任何模型）。
    let preserved_group = existing
        .meta
        .as_ref()
        .and_then(|meta| meta.loongport_group.clone());
    let restored = Provider {
        settings_config,
        meta: Some(managed_meta(&app_type, op.account_id, preserved_group)),
        ..existing
    };

    state
        .db
        .save_provider(app_type.as_str(), &restored)
        .map_err(|e| AppError::Database(format!("恢复默认配置失败: {e}")))?;

    // 「恢复默认配置」= 回到 LoongPort 的默认 ⇒ 清掉「已手工维护」标记。
    state
        .db
        .set_user_edited(app_type.as_str(), &restored.id, false)
        .map_err(|e| AppError::Database(format!("清除已手工维护标记失败: {e}")))?;

    state
        .model_verification
        .clear_scope(&verification_scope)
        .map_err(|_| AppError::Database("清除模型验证结果失败".into()))?;

    // 重置的正是当前项 ⇒ 把默认配置落到 live 文件上。
    //
    // ⚠️ **这一步不可省，否则这个命令对当前项整体无效**：这个按钮的全部意义就是
    // 「用户把配置改坏了，给他一条回头路」，而改坏的配置**就在 live 文件里**
    // （CLI 读那个文件，不读 DB）。只写 DB 的话，界面提示已恢复默认、库里也确实是
    // 默认配置，而 CLI 用的仍是那份坏配置 —— 且用户没有自救手段：UI 认为它已经是
    // 当前项，再点一次不会触发切换（前端 `if (tier.isCurrent) return;`）。
    //
    // 与 `provision_relay` 同一条路（见那边关于为什么用 `sync_current_provider_for_app`
    // 而不是 `switch` 的说明）。失败只 warn：DB 已经是对的，切一次即生效，
    // 不该因为落地文件写不下去就报「恢复失败」。
    let is_current = ProviderService::current(state, app_type.clone())
        .map(|current| current == restored.id)
        .unwrap_or(false);
    if is_current {
        refresh_live_for_current_tiers(state, std::slice::from_ref(&app_type));
    }

    Ok(())
}

/// 保存中转站行的手工顺序。
///
/// `relay_ids` 是拖动后的完整顺序，下标即新的 `sort_index`。
///
/// ## 为什么行序要用户说了算
///
/// 原来 `creds::list` 排的是 `ORDER BY is_current DESC, id ASC` —— 「当前站」永远第一。
/// 而 `is_current` 会因为用户点某一行的登录/获取密钥而改变 ⇒ **行序跟着跳**。
/// 用户明确指出过：选一个档位不该重排中转站的顺序。
///
/// 现在改成按 `sort_index` 排，而这个命令是唯一会写它的地方 —— 只有用户拖动才改顺序。
#[tauri::command]
pub fn relay_reorder(state: State<'_, AppState>, relay_ids: Vec<i64>) -> Result<(), String> {
    with_conn(state.inner(), |conn| creds::reorder(conn, &relay_ids)).map_err(|e| e.to_string())
}

/// 一条档位 + 它的归属信息。
///
/// **打成结构体而不是 `(TierInfo, Option<String>, Option<i64>)`** —— 两个 `Option`
/// 并列时调换了编译器也不会报，而后果是把档位分给错的账号。带字段名就调不错。
#[derive(Debug, Clone)]
pub(crate) struct OwnedTier {
    pub(crate) tier: TierInfo,
    /// provision 时写下的 `site_origin`。`None` = 历史数据 / 手工造的。
    pub(crate) site_origin: Option<String>,
    /// provision 时写下的中转站账号 id（`meta.loongportAccountId`）。
    /// `None` = 升级前生成的档位（那时还没记账号）。
    pub(crate) account_id: Option<i64>,
}

/// 从档位列表里挑出属于某一行中转站（站点 × 账号）的那些，保持原有顺序。
///
/// ## 归属判据是「站点 + 账号」两项
///
/// provision 时把 `site_origin` 写进 `website_url`、把账号写进
/// `meta.loongportAccountId`（见本文件 provision 段）。
///
/// ⚠️ **只看站点是不够的** —— 同一个站可以挂多个账号，那样每一行都会显示该站的
/// **全部**档位（包括别的账号的），用户看到的档位数与他实际拥有的不符，
/// 点进去用的还是别人的 sk。
///
/// ⚠️ **不能靠 `provider_id` 反推**：它是 sha256 的前 16 位 hex
/// （`provision::provider_id_for`），单向不可逆 —— 那不是判据。
///
/// `website_url` 为 `None` 的档位**不归任何行**（历史数据或手工造的），
/// 宁可不显示也不能猜着塞给某个站 —— 塞错了用户会以为自己在 A 站买的档位属于 B 站。
///
/// `account_id` 为 `None` 的档位（升级前生成的）**按站点归属**：那时还没记账号，
/// 不显示它们等于让老档位在界面上凭空消失。它们在下次 provision 后就带上标记了。
/// `app_type` 只用来读「已手工维护」标记（存库，见 `providers.user_edited`）。
pub(crate) fn tiers_of_site(
    state: &AppState,
    tiers: &[OwnedTier],
    site_origin: &str,
    account_id: Option<i64>,
    app_type: &AppType,
) -> Result<Vec<TierInfo>, AppError> {
    tiers
        .iter()
        .filter(|owned| same_site_identity(owned.site_origin.as_deref(), Some(site_origin)))
        .filter(|owned| match (account_id, owned.account_id) {
            // 两边都知道账号 ⇒ 必须相等。
            (Some(want), Some(owner)) => want == owner,
            // 档位没记账号（旧数据）⇒ 只按站点归，见上面的文档。
            (_, None) => true,
            // 这一行还没登录（没有 account_id），而档位有主 ⇒ 不是它的。
            (None, Some(_)) => false,
        })
        .map(|owned| -> Result<TierInfo, AppError> {
            Ok(TierInfo {
                // ⚠️ **`app_id` 靠 `..owned.tier.clone()` 隐式继承**（来自
                // `list_tiers_impl`，那条路按 app 查所以值天然正确）。改成显式构造、
                // 或在中间插一层跨 app 的合并时**必须重新想清楚它** —— 那时它会静默
                // 变错（前端据它筛「属于当前那一屏的档位」），而没有测试会红。
                //
                // 「已手工维护」读存库标记（编辑页置位、恢复默认复位）。
                user_edited: Some(
                    state
                        .db
                        .get_user_edited(app_type.as_str(), &owned.tier.provider_id)?,
                ),
                ..owned.tier.clone()
            })
        })
        .collect()
}

// 曾经这里有个 `relay_list_tiers`（扁平列出全部档位，不按中转站分组）。
// 它在 `relay_list_relays` 上线后就没有调用方了 —— 界面按中转站分行显示，
// 拿一份不带归属的扁平列表没法渲染。2026-08-04 删掉命令壳，
// `list_tiers_impl` 留着（`list_relays_impl` 在用它）。

// 拿一份不带归属的扁平列表没法渲染。2026-08-04 删掉命令壳，
// `list_tiers_impl` 留着（`list_relays_impl` 在用它）。
// （托盘「模型」子菜单与自动模式共用的目录解析已搬到
// `relay::provision::models_from_settings`。）

/// A LoongPort model-chip click is a managed preference, so refreshing the
/// tier should keep it while the newly fetched catalog still advertises it.
/// Once the upstream removes the model, the freshly computed default wins.
pub(crate) fn preserve_supported_codex_model(
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    let Some(model) = provision::extract_model(previous) else {
        return defaults;
    };
    select_codex_model(&defaults, &model).unwrap_or(defaults)
}

/// [`preserve_supported_codex_model`] 的跨平台版：claude / gemini 走 env 形状
/// （剥掉 `[1M]` 声明后对新目录查成员资格），grokbuild 走 TOML 形状，其余平台
/// 没有「选模型」概念，新默认直接接管。四个 arm 与
/// [`provision::model_catalog_apps`] 对齐（那边是名单唯一源，这里按形状分派）。
pub(crate) fn preserve_supported_model(
    app_type: &AppType,
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    match app_type {
        AppType::Codex => preserve_supported_codex_model(defaults, previous),
        AppType::Claude | AppType::Gemini => {
            let catalog = models_from_settings(&defaults);
            provision::preserve_supported_env_model(app_type, defaults, previous, &catalog)
        }
        AppType::GrokBuild => preserve_supported_grok_model(defaults, previous),
        _ => defaults,
    }
}

/// [`preserve_supported_codex_model`] 的 grok 版：读旧选中值（TOML 选中模型表的
/// `model` 字段，[`provision::selected_model`]）对新目录查成员资格，命中就把新
/// 默认的该字段改回旧值 —— profile 名、端点、密钥都不动。
pub(crate) fn preserve_supported_grok_model(
    defaults: serde_json::Value,
    previous: &serde_json::Value,
) -> serde_json::Value {
    let Some(old) = provision::selected_model(&AppType::GrokBuild, previous) else {
        return defaults;
    };
    if !models_from_settings(&defaults).iter().any(|m| m == &old) {
        return defaults;
    }
    select_grok_model(&defaults, &old).unwrap_or(defaults)
}

pub(crate) fn select_codex_model(
    settings: &serde_json::Value,
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let model = model.trim();
    if model.is_empty()
        || !models_from_settings(settings)
            .iter()
            .any(|candidate| candidate == model)
    {
        return Err(AppError::Config(format!(
            "模型 {model:?} 不在这个档位支持的模型列表中"
        )));
    }

    let config = settings
        .get("config")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Config("这个 Codex 档位缺少 config.toml".to_string()))?;
    let updated_config = crate::codex_config::update_codex_toml_field(config, "model", model)
        .map_err(AppError::Config)?;
    let mut updated = settings.clone();
    updated["config"] = serde_json::Value::String(updated_config);
    Ok(updated)
}

/// [`select_codex_model`] 的 grok 版（成员资格校验由调用方对着目录做，与 env 分支
/// 对齐）：改写 config TOML 里选中模型表的 `model` 字段。profile 名
/// （`models.default` 指向的表名）、端点、密钥、上下文窗口都与模型无关，不动；
/// toml_edit 保格式，用户的手工编辑不丢。
pub(crate) fn select_grok_model(
    settings: &serde_json::Value,
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let config = settings
        .get("config")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Config("这个 Grok 档位缺少 config.toml".to_string()))?;
    let updated_config = crate::grok_config::update_selected_model_string(config, "model", model)?;
    let mut updated = settings.clone();
    updated["config"] = serde_json::Value::String(updated_config);
    Ok(updated)
}

fn list_tiers_impl(state: &AppState, app_type: AppType) -> Result<Vec<OwnedTier>, AppError> {
    // AppType 没派生 Copy（上游结构，别为此改它），所以 clone 一份给第二个调用点。
    let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();
    // 这条路按 app 查，所以结果天然同质 —— 每条档位的 `app_id` 就是被查的那个。
    // 先取出来：`app_type` 下一行就被 move 进 `list` 了。
    let app_id = app_type.as_str().to_string();
    let can_verify_models = verification_target::supports_app_type(&app_type);
    let providers = ProviderService::list(state, app_type.clone())?;

    let mut tiers: Vec<OwnedTier> = providers
        .values()
        .filter(|p| is_managed(p))
        .map(|p| OwnedTier {
            tier: TierInfo {
                provider_id: p.id.clone(),
                app_id: app_id.clone(),
                // 倍率读**上次 provision 写下的那个值**（`providers.tier_rate_multiplier`）。
                //
                // 它是服务端定价，不是实时量 —— 所以「刷新倍率」就等于「重新拉分组」，
                // 界面上是顶部刷新 / 更新可用分组 / 登录成功那几下。这条命令仍然
                // **只读本地不发网络**，首屏契约不变，但首屏现在就有倍率可显示了。
                //
                // 读不出来（旧库、行刚被别处删掉）⇒ `None`，UI 显示「倍率未知」。
                // **绝不能退化成 0** —— 那会让用户以为这是最便宜的一档。
                rate_multiplier: state
                    .db
                    .get_tier_rate_multiplier(&app_id, &p.id)
                    .unwrap_or(None),
                group_name: p.name.clone(),
                display_name: p.name.clone(),
                model: provision::selected_model(&app_type, &p.settings_config).unwrap_or_default(),
                // 目录没有就返回空 —— UI/托盘按「无目录」处理，不用按 app 分支
                models: models_from_settings(&p.settings_config),
                is_current: current == p.id,
                can_verify_models,
                // 判据要 `api_base_url`（按站点存），这里拿不到 ⇒ 留 None，
                // 由 `tiers_of_site` 在按站分组时填。见该字段的文档。
                user_edited: None,
                // **这个在本地就能算**（判据是配置里的 `model`），所以首屏就有真值 ——
                // 不像倍率那样留 None 等异步填。见该字段的文档：入口忽隐忽现是有害的。
                // 纯服务端信息，本地推不出来 ⇒ None（UI 不显示标记）。
                allow_image_generation: None,
                site_declared_origin: p
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.site_declared_origin.clone()),
            },
            site_origin: p.website_url.clone(),
            account_id: p.meta.as_ref().and_then(|m| m.loongport_account_id),
        })
        .collect();

    // 按 provision 时写下的 sort_index 排（倍率低的在前）。provider_id 是哈希，
    // 按它排等于随机顺序。
    let order: std::collections::HashMap<&str, usize> = providers
        .values()
        .map(|p| (p.id.as_str(), p.sort_index.unwrap_or(usize::MAX)))
        .collect();
    tiers.sort_by_key(|owned| {
        (
            order
                .get(owned.tier.provider_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
            owned.tier.provider_id.clone(),
        )
    });
    Ok(tiers)
}

/// 列出全部已添加的站点。
#[tauri::command]
pub fn relay_list_sites(state: State<'_, AppState>) -> Result<Vec<SiteInfo>, String> {
    list_sites_impl(state.inner()).map_err(|e| e.to_string())
}

fn list_sites_impl(state: &AppState) -> Result<Vec<SiteInfo>, AppError> {
    with_conn(state, |conn| {
        let mut summaries = Vec::<SiteInfo>::new();
        for relay in creds::list(conn)? {
            // 按注册域身份聚行：同一站的 www/apex/api 拼写变体算一家，不再拆行。
            if let Some(summary) = summaries.iter_mut().find(|summary| {
                same_site_identity(Some(&summary.site_origin), Some(&relay.site_origin))
            }) {
                summary.account_count += usize::from(relay.account_id.is_some());
            } else {
                summaries.push(SiteInfo {
                    site_origin: relay.site_origin,
                    account_count: usize::from(relay.account_id.is_some()),
                });
            }
        }
        Ok(summaries)
    })
}

/// 删掉一个站点，**连带它已生成的托管档位**。
///
/// ## 为什么连带删（2026-08-03 改，原来是只删站点）
///
/// 原来的行为是「不删 provider 记录，要清理走 provider 列表」。那条在只有一个入口
/// （站点切换器上的小叉）时说得通，但中转站行现在有了自己的删除按钮，而用户对那个
/// 按钮的预期是「这一行连它下面那几个档位一起没了」—— 留下一堆没有主人的托管档位
/// （登录态已经删了 ⇒ 它们必定 401），比删干净糟。
///
/// **只删这个站的托管档位**，判据是 `website_url == site_origin`（`prune_stale_tiers`
/// 里写清了为什么 `provider_id` 反推不出归属）。用户自建的 provider 一律不碰。
///
/// ## ⚠️ 「在用的档位」有两条出路：默认拦下；`force` 才放行 —— 闸始终在后端
///
/// 前端的判据只看当前 tab（`RelayRow::usageBlockers` 虽是后端跨 app 算的，但按钮态
/// 仍可能被绕过 / 旧版本前端没有它）。于是：
///
/// 1. 用户在 **Claude** tab 上看某一行 —— 它在 claude 下没有当前项；
/// 2. 而同一个账号在 **Codex** 下的档位正是 codex 的当前项；
/// 3. 删下去 ⇒ codex 的当前 provider 记录没了，而 `~/.codex/config.toml` 还指着它。
///
/// 「删 Claude 页的账号，把 Codex 正在用的配置删掉了」对用户是不可预期的，所以这里
/// **必须自己查一遍全部 app**。查到在用时的两条出路：
///
/// - **默认（`force == false`）**：报错拦下，文案点名平台与档位，让用户先去切走。
/// - **`force == true`**：放行。它只该来自前端那道**点名了在用 app 的确认弹窗**
///   （`RelayRow::usage_blockers` 与这道闸同一次扫描，弹窗文案由它驱动 —— 用户
///   是看着「Codex 正在用 BestApi · Pro」这句话按的确认），即知情删除。
///
/// ## 与 `prune_stale_tiers` 「当前项也删」的区别（两者都对，因为前提不同）
///
/// `prune_stale_tiers` 在 provision 路径上会删当前项，理由是走到那一步说明**服务端已经
/// 没有那个分组了** ⇒ 它的 sk 是死的，留着当当前项只会让 CLI 拿废密钥去 401（见它的文档）。
///
/// 删账号这条路的前提相反：那些档位**还是好的**。默认路径因此拦下（不忍心毁一份能用的
/// 配置）；`force` 路径是用户在弹窗里**看着点名**做出的选择 —— 删掉一条还活着的当前项
/// 正是他要的（「我不想再用这个站了」），与那条裁决不矛盾 —— 判据从「档位还活着吗」
/// 变成「用户知道自己在删什么吗」。
///
/// 顺序：**（force 时）先切官方 → 先删档位再删站点**。反过来的话，站点行没了而
/// `site_origin` 是档位归属的唯一依据 —— 删站点之后就再也认不出哪些档位属于它，
/// 那些记录会永久留在 provider 列表里。切官方放在删档位之前，则是让被安置的档位
/// 先卸下「当前项」身份、删除全走常规路径（见 [`switch_affected_apps_to_official`]）。
#[tauri::command]
pub fn relay_remove_site(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
    force: Option<bool>,
) -> Result<(), String> {
    remove_site_impl(state.inner(), id, force.unwrap_or(false), Some(&app))
        .map_err(|e| e.to_string())
}

pub(crate) fn remove_site_impl(
    state: &AppState,
    id: i64,
    force: bool,
    // `None` = 单测（没有 AppHandle）：切官方照常执行，只是不发事件、不刷托盘。
    app: Option<&tauri::AppHandle>,
) -> Result<(), AppError> {
    // 先取归属信息 —— 删掉那行之后就没法知道该清哪些档位了。
    //
    // ⚠️ **`account_id` 与 `site_origin` 一样必须取**：删的是**一个账号**（一行），
    // 不是「这个站的全部」。同站另一个账号的档位不该被连带清掉 ——
    // 那正是 `prune_stale_tiers` 加账号维度要挡的事（见它的文档）。
    let op = with_conn(state, |conn| creds::get(conn, id))?
        .ok_or_else(|| AppError::Config("这个站点已经不存在了".into()))?;
    let site_origin = op.site_origin;
    let account_id = op.account_id;

    // ⚠️ 闸：这个账号名下有档位正被某个 app 用着 ⇒ 默认**一条都不删，直接报错**；
    // `force == true` 才放行（只该来自点名了在用 app 的前端确认弹窗，见命令文档）。
    //
    // 全有或全无（在 `prune_stale_tiers` 之前拦，而不是让它逐条跳过正在用的那些）：
    // 半删的结果是「账号行没了，名下还剩一条孤儿档位」—— 而托管档位在 provider 列表里
    // 被前端过滤、被通用删除命令拒绝 ⇒ 那条记录用户再也处置不了。
    //
    // 文案点名**哪个平台、哪个档位**：只说「有档位在使用中」的话，用户得自己去六个 tab
    // 里翻是哪一个。他要做的处置（切走 / 或回弹窗里选强制删）完全取决于这个信息。
    //
    let in_use = apps_using_this_accounts_tiers(state, &site_origin, account_id);
    if !in_use.is_empty() && !force {
        let detail = in_use
            .iter()
            .map(|(app_type, name)| format!("{}（{}）", name, app_type.as_str()))
            .collect::<Vec<_>>()
            .join("、");
        return Err(AppError::Config(format!(
            "这个账号名下还有档位正在使用中：{detail}。请先在对应平台切换到别的供应商，再删除这个账号。"
        )));
    }

    // force 路径对「被删掉的当前项」的安置：**先切回官方 seed provider，再删档位**
    // （[`switch_affected_apps_to_official`] 的文档写了完整理由 —— 删本地登录态不会
    // 吊销服务端 sk，留旧配置等于 CLI 继续把请求打向一个用户明确要删掉的站）。
    // 切过去之后档位不再是当前项，`prune_stale_tiers` 全走常规删除路径。
    //
    // 切不动的（codex-image 无官方 seed / seed 被删 / 代理接管禁止切官方）降级成
    // 悬空自愈（`get_effective_current_provider`）—— 宁可降级也不让「删账号」失败。
    if force && !in_use.is_empty() {
        let switched = switch_affected_apps_to_official(state, &in_use);
        if let Some(app) = app {
            for (app_type, provider_id) in &switched {
                // 背后切了 current 就必须广播（2026-08-04 的教训：不发事件的症状是
                // provider 页 / 中转站页静默陈旧，用户以为删除功能坏了）。
                emit_provider_switched(app, app_type, provider_id);
            }
            if !switched.is_empty() {
                crate::tray::refresh_tray_menu(app);
            }
        }
    }

    // 空 `keep` = 这一行名下的托管档位全不保留。
    //
    // 清理失败（`delete_provider` 报错）**不阻止删站点**：那会让用户卡在「删不掉这一行」
    // 上，而他要的是把这个账号清走，站点行也只有这一个入口。
    //
    // ⚠️ **代价要说清：残留的档位用户自己处置不了**。托管档位在 provider 列表里被前端
    // 按 id 前缀过滤掉（`ProviderList.tsx`），`delete_provider` / `update_provider` 也会
    // 被 `reject_if_managed` 拦下 ⇒ 那条记录既看不见也删不掉，只能靠下一次对这个站
    // provision 时的 `prune_stale_tiers` 顺手清（而账号行已经删了 ⇒ 不会再有那一次）。
    //
    // 仍选「不阻断」是因为走到这里的前提已经很窄：上面那道闸保证了它不是任何平台的当前项，
    // 所以残留的是一条**没人在用**的死记录 —— 它不会让任何 CLI 出错，只是脏。
    // 而阻断的代价是用户永久删不掉这个账号。两害相权取其轻，但**别把它写成没有代价**。
    match prune_stale_tiers(
        state,
        &site_origin,
        account_id,
        &std::collections::HashSet::new(),
    ) {
        Ok(removed) => log::info!("删除站点 {site_origin} 时清掉了 {removed} 个托管档位"),
        Err(e) => log::warn!("删除站点 {site_origin} 时清理档位失败（站点仍会删掉）: {e}"),
    }

    // 生图工具跟着对齐一次 —— **这条路必须自己调**（review 抓出）。
    //
    // 另一个调用点在 `mark_pricing_after_success`（provision 收尾），但那条路依赖
    // 「还会再 provision 一次」。删掉的正是拥有生图档位的那个账号时，**不会再有
    // 下一次** ⇒ `loongport-imagegen` 这条 MCP 记录永久留在用户的 CLI 配置里，
    // 而它每次被调用都报「还没有选定用哪个档位生图」—— 一个删不掉的坏工具。
    //
    // 失败只 warn：站点记录马上就删了，不该因为一个 MCP 记录撤不掉而让「删站点」失败
    // （与上面那段清理档位同一条原则）。
    if let Err(e) = imagegen_mcp::sync_registration(state) {
        log::warn!("删除站点 {site_origin} 后同步生图工具记录失败: {e}");
    }

    with_conn(state, |conn| creds::remove(conn, id))
}

/// force 删的收尾一步：把受影响的 app 切回各自的官方 seed provider。
///
/// ## 为什么是「切官方」而不是留着旧配置
///
/// 删登录态只是本地动作，**服务端的 sk 并没有被吊销** —— 留着 live 旧配置等于
/// CLI 继续把请求（和扣费）打向一个用户刚刚明确删掉的站，那才是违背「我不想再
/// 用这个站」的意图。官方 seed 的「空 env / 空 config」正是该 CLI 刚装好时的
/// 默认认证状态（seed 注释原文）：纯中转站用户下次运行会被引导登录；本来就有
/// 官方登录的直接用回它（那份登录与中转站无关，保留正是预期）。
///
/// ## 复用与边界
///
/// live 配置怎么清、current 怎么写、MCP 怎么同步全是 `ProviderService::switch`
/// 现成的，这里只编排「谁该被切」。没有官方 seed 的 app（codex-image 生图）跳过；
/// 切失败（seed 被用户删了 / 代理接管下 switch 主动拒绝切官方 —— 防封号）记日志
/// 降级成悬空自愈，**不让删账号失败** —— 安置是收尾，不是闸。
///
/// 返回成功切到官方的 `(app_type, official_id)`，调用方拿它发 `PROVIDER_SWITCHED`
/// 与刷托盘。
fn switch_affected_apps_to_official(
    state: &AppState,
    affected: &[(AppType, String)],
) -> Vec<(AppType, String)> {
    let mut switched = Vec::new();
    for (app_type, tier_name) in affected {
        let Some(official_id) = crate::database::official_seed_id(app_type) else {
            log::info!(
                "{} 没有官方 seed 可回落（原在用档位「{tier_name}」随删除悬空自愈）",
                app_type.as_str()
            );
            continue;
        };
        match ProviderService::switch(state, app_type.clone(), official_id) {
            Ok(result) => {
                for warning in &result.warnings {
                    log::info!("强删后 {} 切回官方的提示：{warning}", app_type.as_str());
                }
                log::info!(
                    "强删后把 {} 切回官方 provider（原在用档位：{tier_name}）",
                    app_type.as_str()
                );
                switched.push((app_type.clone(), official_id.to_string()));
            }
            Err(e) => {
                log::warn!(
                    "强删后把 {} 切回官方失败（current 将悬空自愈，不影响删除）：{e}",
                    app_type.as_str()
                );
            }
        }
    }
    switched
}
