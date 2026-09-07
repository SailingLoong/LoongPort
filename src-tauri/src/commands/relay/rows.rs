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
    let base_url = sub2api::base_url_for(&app_type, &op.site_origin, &op.api_base_url);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

    fn sub2api_with_session() -> creds::Relay {
        purchase_capability_relay(creds::BackendKind::Sub2Api)
    }

    fn newapi_with_refresh_cookie() -> creds::Relay {
        creds::Relay {
            refresh_token: Some("refresh-cookie".into()),
            ..purchase_capability_relay(creds::BackendKind::NewApi)
        }
    }

    fn newapi_without_refresh_cookie() -> creds::Relay {
        purchase_capability_relay(creds::BackendKind::NewApi)
    }

    #[test]
    fn purchase_capability_requires_login_config_and_backend_credentials() {
        assert!(can_open_site_window(&sub2api_with_session(), true, true));
        assert!(can_open_site_window(
            &newapi_with_refresh_cookie(),
            true,
            true
        ));
        assert!(!can_open_site_window(
            &newapi_without_refresh_cookie(),
            true,
            true
        ));
        assert!(!can_open_site_window(&sub2api_with_session(), false, true));
        assert!(!can_open_site_window(&sub2api_with_session(), true, false));

        let newapi_with_blank_refresh_cookie = creds::Relay {
            refresh_token: Some("   ".into()),
            ..newapi_with_refresh_cookie()
        };
        assert!(!can_open_site_window(
            &newapi_with_blank_refresh_cookie,
            true,
            true
        ));
    }

    /// `RelayRowStatus` 的线上名由 `RelayRow.tsx` 的 `RowStatus` switch 直接消费
    /// （裸字符串比较，无编译器把守）。这里把每个变体的 serde 输出钉死 ——
    /// 改枚举变体名 / 改 rename 规则时这条会红，提醒同步前端 union。
    #[test]
    fn relay_row_statuses_serialize_to_the_wire_names_the_frontend_matches() {
        for (status, wire) in [
            (RelayRowStatus::NotLoggedIn, "\"notLoggedIn\""),
            (RelayRowStatus::SessionExpired, "\"sessionExpired\""),
            (
                RelayRowStatus::SessionExpiredUsable,
                "\"sessionExpiredUsable\"",
            ),
            (RelayRowStatus::NoTiers, "\"noTiers\""),
            (RelayRowStatus::Ready, "\"ready\""),
        ] {
            assert_eq!(
                serde_json::to_string(&status).expect("status 可序列化"),
                wire,
                "{status:?} 的线上名变了，src/lib/api/relay.ts 的 union 与 RowStatus 要跟着改"
            );
        }
    }

    #[test]
    fn relay_row_serializes_backend_owned_remove_confirmation() {
        let row = RelayRow {
            id: 7,
            site_origin: "https://api.example.com".into(),
            site_name: "Example".into(),
            account_label: String::new(),
            status: RelayRowStatus::NotLoggedIn,
            is_current: false,
            can_query_balance: false,
            can_purchase: true,
            can_view_usage: false,
            can_refresh: false,
            usage_blockers: Vec::new(),
            remove_confirmation: RemoveConfirmation::NeverLoggedIn,
            tiers: Vec::new(),
        };

        let json = serde_json::to_value(row).expect("serialize relay row");
        assert_eq!(json["canPurchase"], true);
        assert_eq!(json["canViewUsage"], false);
        assert_eq!(json["removeConfirmation"], "neverLoggedIn");
        assert_eq!(json["usageBlockers"], serde_json::json!([]));
    }

    fn codex_settings(model: &str, models: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": format!(
                "model_provider = \"custom\"\nmodel = {model:?}\n\n[model_providers.custom]\nname = \"Test\"\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            ),
            "modelCatalog": {
                "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
            }
        })
    }

    #[test]
    fn codex_model_list_requires_a_real_catalog() {
        let settings = serde_json::json!({
            "config": "model_provider = \"custom\"\nmodel = \"gpt-current\"\n"
        });

        assert!(
            models_from_settings(&settings).is_empty(),
            "旧 provider 只有当前模型时，不能把它冒充成完整支持列表"
        );
    }

    #[test]
    fn selecting_a_codex_model_validates_and_only_updates_the_model_field() {
        let settings = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);

        let selected = select_codex_model(&settings, " gpt-b ").expect("supported model");
        assert_eq!(
            provision::extract_model(&selected).as_deref(),
            Some("gpt-b")
        );
        assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
        assert_eq!(selected["auth"], settings["auth"]);

        assert!(select_codex_model(&settings, "gpt-unknown").is_err());
    }

    #[test]
    fn refreshing_a_managed_codex_tier_keeps_only_a_still_supported_selection() {
        let defaults = codex_settings("gpt-a", &["gpt-a", "gpt-b"]);
        let previous = codex_settings("gpt-b", &["gpt-a", "gpt-b"]);
        let kept = preserve_supported_codex_model(defaults.clone(), &previous);
        assert_eq!(provision::extract_model(&kept).as_deref(), Some("gpt-b"));

        let removed = codex_settings("gpt-removed", &["gpt-removed"]);
        let reset = preserve_supported_codex_model(defaults, &removed);
        assert_eq!(provision::extract_model(&reset).as_deref(), Some("gpt-a"));
    }

    /// 形状对齐 [`relay::provision`] 生成侧（`deeplink::build_grokbuild_settings`
    /// + `modelCatalog`）：选中模型在 `[model."<default>"]` 表的 `model` 字段。
    fn grok_settings(model: &str, models: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "config": format!(
                "[models]\ndefault = \"{model}\"\n\n[model.\"{model}\"]\nmodel = \"{model}\"\nbase_url = \"https://api.example.com\"\nname = \"Test\"\napi_key = \"sk-test\"\napi_backend = \"responses\"\ncontext_window = 500000\n"
            ),
            "modelCatalog": {
                "models": models.iter().map(|model| serde_json::json!({ "model": model })).collect::<Vec<_>>()
            }
        })
    }

    #[test]
    fn selecting_a_grok_model_only_updates_the_model_field() {
        let settings = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);

        let selected = select_grok_model(&settings, "grok-4.6").expect("supported model");
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &selected).as_deref(),
            Some("grok-4.6")
        );
        // profile 名（models.default 指向的表）、端点、密钥、目录都不动 ——
        // 它们与模型无关
        let config = selected["config"].as_str().expect("config text");
        assert!(config.contains("[model.\"grok-4.5\"]"), "{config}");
        assert!(config.contains("base_url = \"https://api.example.com\""));
        assert!(config.contains("api_key = \"sk-test\""));
        assert_eq!(selected["modelCatalog"], settings["modelCatalog"]);
    }

    #[test]
    fn refreshing_a_managed_grok_tier_keeps_only_a_still_supported_selection() {
        let defaults = grok_settings("grok-4.5", &["grok-4.5", "grok-4.6"]);
        let previous = grok_settings("grok-4.6", &["grok-4.5", "grok-4.6"]);
        let kept = preserve_supported_grok_model(defaults.clone(), &previous);
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &kept).as_deref(),
            Some("grok-4.6")
        );

        let removed = grok_settings("grok-gone", &["grok-gone"]);
        let reset = preserve_supported_grok_model(defaults, &removed);
        assert_eq!(
            provision::selected_model(&AppType::GrokBuild, &reset).as_deref(),
            Some("grok-4.5")
        );
    }

    /// ⭐ **`TierInfo` 必须说清自己落在哪个 CLI 上。**
    ///
    /// ## 它守的是什么缺陷（TODO 债 11）
    ///
    /// provision 链路（`refresh_relay_provision`）一次探**全部平台**，`tiers` 收的是
    /// 全平台的结果，而 UI 那一行只显示**当前 app** 的档位。于是「这个站没有
    /// anthropic 分组」与「拉取失败」在界面上长得一样（都是零档位 +
    /// 「该账号在此平台下没有可用分组」）—— 而前者重试一百次也不会有，后者重试有意义。
    ///
    /// 区分它们所需的信息 provision 时**本来就在手上**（每个分组的 `app_type`），
    /// 少的只是把它发给前端。没有这个字段，前端拿到一堆 tiers 却分不出哪条是自己的。
    ///
    /// ## 为什么键名是 `appId`
    ///
    /// 前端那边这个概念叫 `AppId`（`lib/api/types.ts`），命令层签名也一直吃
    /// `app_id`。发 `appType` 会让同一个东西在两侧各有一个名字。
    #[test]
    fn tier_info_tells_the_frontend_which_cli_it_landed_on() {
        let tier = TierInfo {
            provider_id: "loongport-0123456789abcdef".into(),
            app_id: AppType::Claude.as_str().to_string(),
            group_name: "pro池".into(),
            display_name: "站 · pro池".into(),
            model: "claude-sonnet-5".into(),
            models: vec!["claude-sonnet-5".into()],
            rate_multiplier: Some(1.0),
            is_current: false,
            can_verify_models: true,
            user_edited: None,
            allow_image_generation: None,
            site_declared_origin: None,
        };

        let json = serde_json::to_value(&tier).expect("要能序列化");
        let obj = json.as_object().expect("是个对象");

        assert_eq!(
            obj.get("appId").and_then(|v| v.as_str()),
            Some("claude"),
            "前端要靠 appId 判断这条档位是不是属于它当前那一屏，实际：{:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            obj.get("canVerifyModels").and_then(|value| value.as_bool()),
            Some(true),
            "模型验证支持资格必须由后端随档位返回"
        );
        assert!(
            !obj.contains_key("app_id"),
            "别把 snake_case 键发给前端（TS 那边按 camelCase 读）"
        );
    }

    fn test_app() -> AppType {
        AppType::Codex
    }

    /// 构造一条带归属的档位。`account` 为 `None` 表示升级前生成的旧档位。
    fn owned(id: &str, site: Option<&str>, account: Option<i64>) -> OwnedTier {
        OwnedTier {
            tier: tier(id),
            site_origin: site.map(str::to_string),
            account_id: account,
        }
    }

    /// `tiers_of_site` 的归属参数在归属测试里恒定，包一层省得每处重复。
    /// 它内部造一个空内存库当 state（`tiers_of_site` 要读「已手工维护」标记；
    /// 这些归属测试不关心标记，空库读出来全是 false 即可）。
    fn tiers_of(tiers: &[OwnedTier], site: &str, account: Option<i64>) -> Vec<TierInfo> {
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().expect("内存库"),
        ));
        tiers_of_site(&state, tiers, site, account, &test_app()).expect("tiers_of_site 不该失败")
    }

    /// ⭐ **`tiers_of_site` 的 `user_edited` 来自存库标记，不是内容比对。**
    ///
    /// 旧实现靠比对 settings_config 与默认值算出「用户改过没有」；现在改为读
    /// `providers.user_edited`（编辑页置位、恢复默认复位）。这条钉住：分组时
    /// `user_edited` 如实反映库里标记，而不是原样透传 `None`。
    #[test]
    fn grouping_reads_the_user_edited_flag_from_the_database(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("内存库"));
        let state = AppState::new(db.clone());
        // 先造两条 provider 行（get_user_edited 读的是 providers 表，不是空表）。
        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute(
                    "INSERT INTO providers (id, app_type, name, settings_config) \
                     VALUES ('t-default','codex','t-default','{}'), ('t-edited','codex','t-edited','{}')",
                    [],
                )
                .expect("插行");
        }
        // 库里只给 t-edited 置位；t-default 不置。
        db.set_user_edited(AppType::Codex.as_str(), "t-edited", true)
            .expect("置位");

        let tiers = vec![
            owned("t-default", Some(site), Some(1)),
            owned("t-edited", Some(site), Some(1)),
        ];
        let got = tiers_of_site(&state, &tiers, site, Some(1), &test_app()).expect("分组不该失败");
        let flags: Vec<_> = got.iter().map(|t| t.user_edited).collect();
        assert_eq!(
            flags,
            vec![Some(false), Some(true)],
            "user_edited 该读库里标记（t-default 没置位=false，t-edited 置位=true）"
        );
        Ok(())
    }

    #[test]
    fn tiers_are_grouped_by_site_origin_not_by_guessing() {
        let a = "https://bestapi.store";
        let b = "https://other.dev";
        let tiers = vec![
            owned("t-a1", Some(a), Some(1)),
            owned("t-b1", Some(b), Some(1)),
            owned("t-a2", Some(a), Some(1)),
            // 没有 website_url 的历史数据：不归任何站。
            owned("t-orphan", None, Some(1)),
        ];

        assert_eq!(
            tiers_of(&tiers, a, Some(1))
                .iter()
                .map(|t| t.provider_id.clone())
                .collect::<Vec<_>>(),
            vec!["t-a1", "t-a2"],
            "同站的档位要按原顺序全带上（顺序 = provision 时的 sort_index，倍率低的在前）"
        );
        assert_eq!(tiers_of(&tiers, b, Some(1)).len(), 1);

        // 孤儿档位不能被塞给任何站 —— 塞错了用户会以为在 A 站买的档位属于 B 站。
        let all: usize = [a, b]
            .iter()
            .map(|s| tiers_of(&tiers, s, Some(1)).len())
            .sum();
        assert_eq!(all, 3, "4 条里那条没有 website_url 的必须落空");
    }

    /// ⭐ **同一个站上的两个账号不能看到对方的档位。**
    ///
    /// 实测踩到的类：归属原本只判 `website_url`（站点），于是同站每一行都显示该站的
    /// **全部**档位 —— 用户看到的档位数与他实际拥有的不符，点进去用的还是别人的 sk
    /// （连账单都算到别人头上）。
    #[test]
    fn tiers_are_split_between_two_accounts_on_the_same_site() {
        let site = "https://bestapi.store";
        let tiers = vec![
            owned("t-acct7", Some(site), Some(7)),
            owned("t-acct9", Some(site), Some(9)),
            // 升级前生成的：没记账号 ⇒ 只按站点归，两个账号都看得到（见函数文档）。
            owned("t-legacy", Some(site), None),
        ];

        let seven: Vec<_> = tiers_of(&tiers, site, Some(7))
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(
            seven,
            vec!["t-acct7", "t-legacy"],
            "账号 7 只该看到自己的 + 没记归属的旧档位，**不该看到账号 9 的**"
        );

        let nine: Vec<_> = tiers_of(&tiers, site, Some(9))
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(nine, vec!["t-acct9", "t-legacy"]);

        // 还没登录的行（没有 account_id）：有主的档位都不是它的。
        let anon: Vec<_> = tiers_of(&tiers, site, None)
            .iter()
            .map(|t| t.provider_id.clone())
            .collect();
        assert_eq!(anon, vec!["t-legacy"], "未登录的行不该认领任何有主的档位");
    }

    #[test]
    fn site_matching_is_exact_not_prefix() {
        // 前缀匹配会让 https://api.store 命中 https://api.store.evil.com。
        let tiers = vec![owned("t1", Some("https://api.store"), Some(1))];
        assert_eq!(tiers_of(&tiers, "https://api.store", Some(1)).len(), 1);
        assert!(tiers_of(&tiers, "https://api.sto", Some(1)).is_empty());
        assert!(tiers_of(&tiers, "https://api.store.evil.com", Some(1)).is_empty());
    }

    #[test]
    fn default_site_is_the_placeholder_from_the_requirement() {
        assert_eq!(DEFAULT_SITE, "790053500.com");
    }

    /// ⭐ 钉住「默认站在 aff **内置表**里有码」—— 这与它上一版的规则**正好相反**。
    ///
    /// 默认站曾是维护者自己的站，那时它**有意不在** aff 表里（服务端拒绝自己邀请自己）。
    /// 换成 `790053500.com` 之后那条理由不再适用，有码才是对的 —— 但
    /// [`crate::relay::aff`] 的测试里仍留着「维护者自己的站不该有码」那条，
    /// 很容易有人按类比把默认站也从表里划掉，而那**不报任何错**，
    /// 只是每一次「留空点确定」都白丢一笔返利。
    ///
    /// ⚠️ **它守的是内置那一层，不是运行时的最终取值**（codex review 纠正）：
    /// 实际取码走 [`crate::relay::remote_config::resolve_aff_code`] 的两层回落，
    /// 远端配置命中就用远端的，且**远端给空串 = 撤销、不回落到内置**。
    /// 所以本条断言不能、也不该保证「线上一定带码」—— 那取决于维护者当天发的配置。
    #[test]
    fn the_default_site_has_a_builtin_affiliate_code() {
        assert!(
            crate::relay::aff::aff_code_for(&format!("https://{DEFAULT_SITE}")).is_some(),
            "{DEFAULT_SITE} 是默认站且不是维护者自己的站，必须在 aff 内置表里"
        );
    }

    fn verification_report(target: TargetKey, verdict: Verdict) -> VerificationReport {
        VerificationReport {
            target,
            verdict,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: Vec::new(),
            diagnostics: Vec::new(),
            rules_version: RULES_VERSION,
            checked_at: 1_786_214_400,
        }
    }

    fn reset_state(valid_key: bool) -> (AppState, Arc<ResetVerifier>, String, String, TargetKey) {
        let site = "https://reset.example";
        let db = Arc::new(crate::database::Database::memory().expect("init db"));
        let verifier = Arc::new(ResetVerifier::new());
        let mut state = AppState::new(db.clone());
        state.model_verification = Arc::new(ModelVerificationCoordinator::with_verifier(
            db.clone(),
            verifier.clone(),
        ));
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "Reset", "https://reset.example/v1")
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "reset@example.com",
                    login_identifier: "reset@example.com",
                },
                "token",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let other_provider_id = provision::provider_id_for(site, Some(7), 2);
        let settings_config = if valid_key {
            provision::settings_config_for(
                &AppType::Codex,
                "sk-reset",
                "Reset tier",
                "https://reset.example/v1",
                DEFAULT_MODEL,
            )
            .expect("codex config")
        } else {
            serde_json::json!({"model_provider":"custom"})
        };
        let provider = Provider {
            settings_config,
            ..seeded_owned(&provider_id, "Reset tier", Some(site), 7)
        };
        db.save_provider("codex", &provider).expect("save provider");
        db.save_provider(
            "codex",
            &Provider {
                settings_config: provision::settings_config_for(
                    &AppType::Codex,
                    "sk-other",
                    "Other tier",
                    "https://reset.example/v1",
                    DEFAULT_MODEL,
                )
                .expect("other config"),
                ..seeded_owned(&other_provider_id, "Other tier", Some(site), 7)
            },
        )
        .expect("save other provider");
        db.set_user_edited("codex", &provider_id, true)
            .expect("mark edited");

        let running = TargetKey::new(&provider_id, "codex", "gpt-running");
        for report in [
            verification_report(
                TargetKey::new(&provider_id, "codex", "gpt-a"),
                Verdict::Suspicious,
            ),
            verification_report(
                TargetKey::new(&provider_id, "codex", "gpt-b"),
                Verdict::Anomaly,
            ),
            verification_report(
                TargetKey::new(&other_provider_id, "codex", "gpt-other"),
                Verdict::Trusted,
            ),
        ] {
            crate::relay::model_verification::store::upsert_active(&db, &report)
                .expect("seed verification report");
        }

        (state, verifier, provider_id, other_provider_id, running)
    }

    #[tokio::test]
    async fn reset_tier_config_validation_failure_cancels_run_but_preserves_all_reports() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(false);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");

        let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
            .expect_err("missing key must reject reset");

        assert!(error.to_string().contains("密钥"));
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id.clone(), other_provider_id.clone()])
                .expect("list reports")
                .len(),
            3
        );
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id, other_provider_id])
                .expect("reports after late completion")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn reset_tier_config_save_failure_cancels_run_but_preserves_all_reports() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_reset BEFORE UPDATE ON providers
                     BEGIN SELECT RAISE(FAIL, 'reject reset'); END;",
            )
            .expect("install failure trigger");

        let error = reset_tier_config_in_state(&state, &provider_id, AppType::Codex)
            .expect_err("provider save must fail");

        assert!(matches!(error, AppError::Database(_)));
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id.clone(), other_provider_id.clone()])
                .expect("list reports")
                .len(),
            3
        );
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert_eq!(
            state
                .model_verification
                .list_results(&[provider_id, other_provider_id])
                .expect("reports after late completion")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn reset_tier_config_success_clears_only_target_scope_and_rejects_late_completion() {
        let (state, verifier, provider_id, other_provider_id, running) = reset_state(true);
        state
            .model_verification
            .start(running.clone())
            .await
            .expect("start run");

        reset_tier_config_in_state(&state, &provider_id, AppType::Codex).expect("reset succeeds");

        let rows = state
            .model_verification
            .list_results(&[provider_id.clone(), other_provider_id.clone()])
            .expect("list reports");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target.provider_id, other_provider_id);
        assert!(!state
            .db
            .get_user_edited("codex", &provider_id)
            .expect("edited flag"));
        let _ = verifier.complete(
            &running,
            verification_report(running.clone(), Verdict::Trusted),
        );
        tokio::task::yield_now().await;
        assert!(state
            .model_verification
            .list_results(&[provider_id])
            .expect("target reports")
            .is_empty());
    }

    /// ⭐ 恢复默认必须保住**每一个**带目录平台的 `modelCatalog`。
    ///
    /// 回归背景：PR #237 给 grok 补目录时只改了 persist 侧的平台名单、漏了 reset 侧
    /// ——「恢复默认」把 Claude / Gemini / Grok 的模型芯片清空到下次 provision。
    /// 名单唯源是 [`provision::model_catalog_apps`]，这条测试按平台全量遍历：
    /// 以后名单加平台，新平台自动被覆盖，不会再出现「persist 改了 reset 没跟」。
    #[test]
    fn reset_tier_config_keeps_the_model_catalog_for_every_catalog_app() {
        for (app_type, model_names) in [
            (AppType::Claude, vec!["claude-opus-5", "claude-sonnet-5"]),
            (AppType::Codex, vec!["gpt-5.6-codex", "gpt-5.6-mini"]),
            (AppType::Gemini, vec!["gemini-3-pro", "gemini-3-flash"]),
            (AppType::GrokBuild, vec!["grok-4.6", "grok-4.5"]),
        ] {
            let models: Vec<String> = model_names.iter().map(|s| s.to_string()).collect();
            let site = "https://catalog.example";
            let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
            let state = AppState::new(db.clone());
            let row_id = with_conn(&state, |conn| {
                creds::save_site(conn, site, "Catalog", "https://catalog.example/v1")
            })
            .expect("save site");
            with_conn(&state, |conn| {
                creds::save_credentials(
                    conn,
                    row_id,
                    creds::AccountIdentity {
                        id: 7,
                        label: "catalog@example.com",
                        login_identifier: "catalog@example.com",
                    },
                    "token",
                    None,
                    None,
                    creds::SessionEnvironment::default(),
                )
            })
            .expect("save credentials");

            let provider_id = provision::provider_id_for(site, Some(7), 1);
            let settings = provision::settings_config_with_models(
                &app_type,
                "sk-catalog",
                "Catalog·Pro",
                "https://catalog.example/v1",
                &models[0],
                Some(&models),
            )
            .expect("settings with catalog");
            // 前提：该平台的生成器真把目录写进了 settings（Gemini 只收 gemini-* 家族，
            // 所以每个平台用自家家族的模型名）。
            let before = models_from_settings(&settings);
            assert_eq!(
                before.len(),
                models.len(),
                "{} 的生成器没把目录写进 settings —— 测试前提不成立",
                app_type.as_str()
            );

            db.save_provider(
                app_type.as_str(),
                &Provider {
                    settings_config: settings,
                    ..seeded_owned(&provider_id, "Catalog·Pro", Some(site), 7)
                },
            )
            .expect("save provider");

            reset_tier_config_in_state(&state, &provider_id, app_type.clone())
                .expect("reset succeeds");

            let after = state
                .db
                .get_provider_by_id(&provider_id, app_type.as_str())
                .expect("read back")
                .expect("provider 还在")
                .settings_config;
            assert_eq!(
                models_from_settings(&after),
                before,
                "{} 恢复默认后 modelCatalog 必须原样保留",
                app_type.as_str()
            );
        }
    }

    /// ⭐ **默认路径下，删账号不许毁掉「别的平台」正在用的档位** —— 前端那道判据挡不住这一类。
    ///
    /// 这是 review 抓出的缺陷现场，复现路径：
    ///
    /// 1. `list_relays_impl` 吃 `app_type` ⇒ `RelayRow.tiers` 只含**当前 tab** 的档位；
    /// 2. 如果删除资格只按当前 tab 的档位判断，claude tab 可能看不到 codex 的当前项；
    /// 3. 而这个账号在 **codex** 下的档位正是 codex 的当前项 ⇒ 删下去把它清了，
    ///    `~/.codex/config.toml` 却还指着它。
    ///
    /// 所以闸必须在后端、必须扫全部 app。**会红的改法**：把
    /// `apps_using_this_accounts_tiers` 从只扫 `AppType::all()` 改成只扫某一个 app。
    #[test]
    fn removing_an_account_is_refused_while_another_app_still_uses_its_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        // 登录这一行 —— **必须有 `account_id`**：没有它的行派生不出 provider id、
        // 名下不可能有档位，守卫对那种行有意不拦（见
        // `an_untagged_row_is_not_blocked_by_another_accounts_current_tier`）。
        let row_id = with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "me@example.com",
                    login_identifier: "me@example.com",
                },
                "tok",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        // 这个账号在 codex 下的档位，且**它就是 codex 的当前项**。
        let codex_tier = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
        )
        .expect("seed codex tier");
        db.set_current_provider("codex", &codex_tier)
            .expect("set codex current");

        // 用户此刻停在 claude tab 上（那边这一行没有当前项）—— 前端会放行，后端必须拦。
        let err = remove_site_impl(&state, row_id, false, None)
            .expect_err("⭐ 名下有档位是别的平台的当前项时，删除必须失败");
        let msg = err.to_string();
        assert!(
            msg.contains("codex"),
            "文案必须点名是哪个平台 —— 用户要去那里切走，实际：{msg}"
        );
        assert!(
            msg.contains("BestApi · Pro"),
            "文案必须点名是哪个档位，实际：{msg}"
        );

        // 全有或全无：拦下之后**一条都不能少**，账号行也必须还在。
        assert!(
            db.get_provider_by_id(&codex_tier, "codex")
                .expect("query")
                .is_some(),
            "被拦下时那条档位必须完好 —— 半删会留下用户处置不了的孤儿记录"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_some(),
            "档位没删掉，账号行也不该删"
        );
    }

    /// 反面：没有任何平台在用它时，删除照常进行（连带清掉档位）。
    ///
    /// 这条与上一条成对 —— 只有上一条的话，把闸写成「无条件拒绝」也能过。
    #[test]
    fn removing_an_account_still_works_when_no_app_uses_its_tiers() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        let tier = provision::provider_id_for(site, None, 1);
        db.save_provider("codex", &seeded(&tier, "BestApi · Pro", Some(site)))
            .expect("seed");
        // **不设 current** —— 别的 provider 是当前项，或压根没有当前项。

        remove_site_impl(&state, row_id, false, None).expect("没人在用它时删除该成功");

        assert!(
            db.get_provider_by_id(&tier, "codex")
                .expect("query")
                .is_none(),
            "档位该被连带清掉"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_none(),
            "账号行该被删掉"
        );
    }

    /// 第三条出路：用户在前端弹窗里看着「Codex 正在用 xxx」按了确认（`force`）⇒
    /// 连在用的档位一起删干净。
    ///
    /// 这条与第一条成对 —— 没有它的话，把闸写成「在用就无条件拒绝（连 force 也拦）」
    /// 也能过前两条。钉住的语义：
    ///
    /// - force 放行后**全有或全无**：档位、账号行都得没了（不留孤儿记录）；
    /// - 被删的是 codex 的**当前项** —— 这条测试的内存库里**没有官方 seed**
    ///   （`init_default_official_providers` 只在真应用启动时跑），切官方必然失败
    ///   ⇒ 它同时钉住降级路径：**安置失败不阻断删除**，current 悬空自愈。
    ///   （正向「切回官方」那条没法单测 —— `ProviderService::switch` 会写真实
    ///   live 配置文件，codex/claude 没有测试沙箱；映射由
    ///   `official_seed_id_maps_text_apps_and_denies_codex_image` 把守。）
    #[test]
    fn forced_removal_deletes_even_while_another_app_uses_its_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestApi", "https://bestapi.store/v1")
        })
        .expect("save site");

        let row_id = with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "me@example.com",
                    login_identifier: "me@example.com",
                },
                "tok",
                None,
                None,
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let codex_tier = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &seeded_owned(&codex_tier, "BestApi · Pro", Some(site), 7),
        )
        .expect("seed codex tier");
        db.set_current_provider("codex", &codex_tier)
            .expect("set codex current");

        remove_site_impl(&state, row_id, true, None).expect("force 是用户知情后的选择，该放行");

        assert!(
            db.get_provider_by_id(&codex_tier, "codex")
                .expect("query")
                .is_none(),
            "force 该连当前项档位一起删掉"
        );
        assert!(
            with_conn(&state, |conn| creds::get(conn, row_id))
                .expect("query row")
                .is_none(),
            "账号行该被删掉"
        );
    }

    /// ⭐ **还没登录的行（`account_id` 为 `None`）不该被别人的档位拦住**。
    ///
    /// 第二路 review 抓出的：`belongs_to_account` 对 `None` 返回 `true`（"不按账号过滤"），
    /// 那对**删除**方向是对的（同站没记归属的旧档位该跟着清），但守卫方向反过来就成了
    /// 「把别人正在用的档位算成你的」。
    ///
    /// 这种行真实可达：`clear_credentials` 会把 `account_id` 置 `NULL`（站点换了后端
    /// 协议时走这条），而唯一索引把 `NULL` 视为互不相等 ⇒ 它与已登录的行并存。
    /// 症状是用户删一个**空行**时被告知「你名下还有档位正在使用中：B 的档位（codex）」，
    /// 而唯一出路是去 codex 把 B 切走。
    ///
    /// 会红的改法：去掉 `apps_using_this_accounts_tiers` 里那个 `account_id.is_some()`。
    #[test]
    fn an_untagged_row_is_not_blocked_by_another_accounts_current_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位是 codex 的当前项。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
            .expect("seed");
        db.set_current_provider("codex", &b_tier)
            .expect("set current");

        let state = AppState::new(db.clone());

        assert!(
            apps_using_this_accounts_tiers(&state, site, None).is_empty(),
            "⭐ 还没登录的行认不出归属 ⇒ 不该拦。它派生不出 provider id，\
                 名下本来就不可能有档位，漏拦没有代价；而误拦会让用户删不掉一个空行"
        );

        // 而删除方向的语义不变：`prune_stale_tiers` 传 `None` 时仍会清同站没记归属的档位。
        // 这条只是确认上面那个改动没顺手改掉 `belongs_to_account` 本身。
        let legacy = provision::provider_id_for(site, None, 5);
        db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
            .expect("seed legacy");
        let legacy_provider = db
            .get_provider_by_id(&legacy, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_account(&legacy_provider, site, None),
            "删除方向对 `None` 仍是「算是我的」—— 那是旧数据能被清掉的前提"
        );
    }

    /// ⭐ **登录态失效之后，那一行仍然带着它的档位、昵称和「已过期」这个状态。**
    ///
    /// 修之前 `check_session` 走的是 `clear_credentials`，它把 `account_id` 一起抹掉，
    /// 于是三件事同时静默出错（都不报任何错）：
    ///
    /// 1. `tiers_of_site` 对「行没有 account_id、档位有」判为不属于它
    ///    ⇒ **返回空 tiers**，界面退化成「没有可用分组 + 获取密钥」；
    /// 2. `session_expired()` 要求 `account_id.is_some()` ⇒ 变成 `false`
    ///    ⇒ 界面说「还没登录」，而用户明明登录过；
    /// 3. `account_label` 被清空 ⇒ 昵称没了。
    ///
    /// 而 sk 一把都没失效。用户看到的是「密钥没了」，然后去重建一遍。
    ///
    /// 会红的改法：把 `check_session` 里的 `clear_session` 换回 `clear_credentials`。
    #[test]
    fn an_expired_session_keeps_its_tiers_label_and_usable_status() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());

        let row_id = with_conn(&state, |conn| {
            creds::save_site(conn, site, "BestAPI", "https://bestapi.store")
        })
        .expect("save site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "tok",
                None,
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("save credentials");

        let tier_id = provision::provider_id_for(site, Some(7), 1);
        let settings_config = provision::settings_config_for(
            &AppType::Codex,
            "sk-valid",
            "Pro池",
            "https://bestapi.store/v1",
            "gpt-5.6-sol",
        )
        .expect("settings");
        db.save_provider(
            "codex",
            &Provider {
                settings_config,
                ..seeded_owned(&tier_id, "Pro池", Some(site), 7)
            },
        )
        .expect("seed tier");

        with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

        let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
        let row = rows.iter().find(|r| r.id == row_id).expect("行还在");

        assert!(
            matches!(row.status, RelayRowStatus::SessionExpiredUsable),
            "登录过 + 没 token + 没 refresh ⇒ 必须报「登录已过期」，而不是「还没登录」"
        );
        assert_eq!(row.account_label, "我的号", "昵称不该跟着会话一起没");
        assert_eq!(
            row.tiers.len(),
            1,
            "⭐ 分组与 sk 与网页登录态无关，不该从界面消失"
        );
        assert_eq!(row.tiers[0].provider_id, tier_id);
    }

    #[test]
    fn a_relay_with_a_managed_key_can_query_balance_without_a_session() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        let provider_id = provision::provider_id_for(site, None, 1);
        let settings = provision::settings_config_for(
            &AppType::Codex,
            "sk-test",
            "Pro池",
            "https://bestapi.store/v1",
            "gpt-5.6-sol",
        )
        .expect("settings");
        db.save_provider(
            "codex",
            &Provider {
                id: provider_id,
                name: "Pro池".into(),
                settings_config: settings,
                website_url: Some(site.into()),
                category: Some("aggregator".into()),
                created_at: Some(1),
                sort_index: Some(0),
                notes: None,
                meta: None,
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .expect("provider");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");
        assert!(matches!(row.status, RelayRowStatus::NotLoggedIn));
        assert!(row.can_query_balance);
        assert!(!row.can_refresh);
        assert!(
            relay_refresh_targets(&state, &AppType::Codex)
                .expect("refresh targets")
                .iter()
                .any(|(id, _, can_refresh)| *id == row_id && !can_refresh),
            "顶部全量刷新也要包含只能用 SK 查余额的账号"
        );
    }

    #[test]
    fn a_refreshable_session_is_not_reported_as_not_logged_in() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "expired-token",
                Some("refresh-token"),
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");

        assert!(
            !matches!(row.status, RelayRowStatus::NotLoggedIn),
            "refresh token 可自动续期时，后端不能要求用户重新登录"
        );
        assert!(row.can_refresh);
    }

    #[test]
    fn session_expired_usable_requires_an_extractable_managed_key() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db.clone());
        let row_id =
            with_conn(&state, |conn| creds::save_site(conn, site, "BestAPI", site)).expect("site");
        with_conn(&state, |conn| {
            creds::save_credentials(
                conn,
                row_id,
                creds::AccountIdentity {
                    id: 7,
                    label: "我的号",
                    login_identifier: "me@x.com",
                },
                "token",
                None,
                Some(1),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");

        let tier_id = provision::provider_id_for(site, Some(7), 1);
        db.save_provider(
            "codex",
            &Provider {
                id: tier_id,
                name: "坏配置".into(),
                settings_config: serde_json::json!({}),
                website_url: Some(site.into()),
                category: Some("aggregator".into()),
                created_at: Some(1),
                sort_index: Some(0),
                notes: None,
                meta: Some(managed_meta(&AppType::Codex, Some(7), None)),
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .expect("provider");
        with_conn(&state, |conn| creds::clear_session(conn, row_id)).expect("clear session");

        let row = list_relays_impl(&state, AppType::Codex)
            .expect("list")
            .into_iter()
            .find(|row| row.id == row_id)
            .expect("row");

        assert!(matches!(row.status, RelayRowStatus::SessionExpired));
        assert!(!row.can_query_balance);
    }
}
