use indexmap::IndexMap;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::commands::copilot::CopilotAuthState;
use crate::commands::xai_oauth::XaiOAuthState;
use crate::error::AppError;
use crate::events::{emit_provider_switched, UNIVERSAL_PROVIDER_SYNCED, USAGE_CACHE_UPDATED};
use crate::provider::{ClaudeDesktopMode, Provider};
use crate::services::{
    EndpointLatency, ProviderService, ProviderSortUpdate, SpeedtestService, SwitchResult,
};
use crate::store::AppState;
use std::str::FromStr;

// 常量定义
const TEMPLATE_TYPE_GITHUB_COPILOT: &str = "github_copilot";
const TEMPLATE_TYPE_TOKEN_PLAN: &str = "token_plan";
const TEMPLATE_TYPE_BALANCE: &str = "balance";
const TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION: &str = "official_subscription";
const COPILOT_UNIT_PREMIUM: &str = "requests";

/// 获取所有供应商
#[tauri::command]
pub fn get_providers(
    state: State<'_, AppState>,
    app: String,
) -> Result<IndexMap<String, Provider>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::list(state.inner(), app_type).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_current_provider(state: State<'_, AppState>, app: String) -> Result<String, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::current(state.inner(), app_type).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_provider(
    state: State<'_, AppState>,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] addToLive: Option<bool>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    add_provider_internal(state.inner(), app_type, provider, addToLive.unwrap_or(true))
        .map_err(|e| e.to_string())
}

fn add_provider_internal(
    state: &AppState,
    app_type: AppType,
    provider: Provider,
    add_to_live: bool,
) -> Result<bool, AppError> {
    // ⚠️ **新增一条 `loongport-*` id 的 provider 必须拒**（review 抓出这里一直没守卫）。
    //
    // 那些 id 由 `provision::provider_id_for` 从「站点 + 账号 + 分组」派生，只有
    // provision 有资格生成。手工造一条同前缀的记录会**伪装成托管档位**：
    // 它从普通 provider 列表里消失（前端按前缀过滤）、转而出现在运营商区里，
    // 而那一区的「恢复默认配置」会拿运营商的默认值把用户自己配的东西整份覆盖掉。
    //
    // 这个口子不是本轮引入的（`add` 从来就没拦），但手已经伸到这个文件、现在做得了
    // ⇒ 一并清掉（CLAUDE.md defer 准入闸的空间维度）。
    //
    // provision 自己不走这条命令（它直调 `state.db.save_provider`），所以拦这里
    // 不影响正常的档位生成。
    crate::operator::reject_if_managed(&provider.id)?;
    ProviderService::add(state, app_type, provider, add_to_live)
}

#[tauri::command]
pub fn update_provider(
    state: State<'_, AppState>,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] originalId: Option<String>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    update_provider_internal(state.inner(), app_type, originalId.as_deref(), provider)
        .map_err(|e| e.to_string())
}

fn update_provider_internal(
    state: &AppState,
    app_type: AppType,
    original_id: Option<&str>,
    provider: Provider,
) -> Result<bool, AppError> {
    // ## 托管档位**可以**改内容，但**不许改 id**
    //
    // 原来这里两头都拦（连内容编辑一起拒），理由写的是「手工改了下次 provision 就被
    // 覆盖，与其让用户白改一次不如当场指路」。**那个前提后来不成立了**：provision
    // 改成了「已存在的档位只换 sk、保住用户的编辑」（`provision::patch_api_key`），
    // 所以手工编辑现在是安全的、能留住的 —— 拦着它只是在挡一件已经做对了的事。
    //
    // 运营商区的「编辑配置」按钮走的正是这条命令（跳 cc-switch 的编辑页，
    // 那页支持全部字段，我们不重做）。用户点它之前会先看到一道警告：保存后这个档位
    // 归他自己维护，出问题用「恢复默认配置」退回来。
    //
    // ## 但**不许凭空造出一个托管 id**
    //
    // id 是托管判据本身（`provision::provider_id_for` 生成的前缀），所以：
    //
    // - 托管 → 普通 id：那条记录**脱管** —— provision 认不出它，于是给同一个分组
    //   再插一条新记录，用户会看到两个一模一样的档位，而旧那条永远清不掉。
    // - 普通 → 托管 id：**伪装成托管项** —— 它会出现在运营商区里，
    //   而「恢复默认配置」会拿运营商的默认值把用户自己配的东西整份覆盖掉。
    //
    // ⚠️ **判据不能是「id 变了没有」** —— review 抓出那样有个绕过口子：
    // `original_id` 传 `None` 时 `ProviderService::update` 会拿 `provider.id`
    // 自己当原 id（`services/provider/mod.rs:2608`），于是「不是改名」成立、
    // 守卫不介入，而 `save_provider` 是 **upsert** ⇒ 一条自选的 `loongport-*`
    // 记录被凭空写进库里。
    //
    // 正确判据是**这个托管 id 得对应一条已经存在的托管记录**：
    // 就地编辑（id 早在库里）放行，凭空造一个新的托管 id 拒掉。
    // 这同时覆盖了上面两种改名 —— 不必再单独判「改没改名」。
    if crate::operator::is_managed(&provider.id)
        && state
            .db
            .get_provider_by_id(&provider.id, app_type.as_str())?
            .is_none()
    {
        return Err(AppError::Message(
            "不能把供应商改成 LoongPort 托管档位的 id —— 那个 id 由 LoongPort 生成".to_string(),
        ));
    }
    // 反向：把**已存在的**托管记录改成别的 id（脱管）。这条仍按老判据拦。
    if let Some(old) = original_id.filter(|old| *old != provider.id) {
        crate::operator::reject_if_managed(old)?;
    }
    ProviderService::update(state, app_type, original_id, provider)
}

#[tauri::command]
pub fn delete_provider(
    state: State<'_, AppState>,
    app: String,
    id: String,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    delete_provider_internal(state.inner(), app_type, &id).map_err(|e| e.to_string())
}

fn delete_provider_internal(
    state: &AppState,
    app_type: AppType,
    id: &str,
) -> Result<bool, AppError> {
    // 删掉档位不会让服务端那把 sk 消失，只会让它下次 provision 又原样冒出来（id 由
    // 站点 + 分组派生、是稳定的）。真要清理得从运营商区那条「删除站点」走。
    crate::operator::reject_if_managed(id)?;
    ProviderService::delete(state, app_type, id).map(|_| true)
}

#[tauri::command]
pub fn remove_provider_from_live_config(
    state: tauri::State<'_, AppState>,
    app: String,
    id: String,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::remove_from_live_config(state.inner(), app_type, &id)
        .map(|_| true)
        .map_err(|e| e.to_string())
}

fn switch_provider_internal(
    state: &AppState,
    app_type: AppType,
    id: &str,
) -> Result<SwitchResult, AppError> {
    // 切托管档位必须走 `operator_switch_tier` —— 只有它编排「退出 ChatGPT → 切换 → 重开」。
    // 从这条通用命令切进去的结果是配置改了、ChatGPT 还拿着旧分组的 sk 在跑，界面上却显示
    // 切换成功，用户无从察觉。
    //
    // 守卫落在这一层而不是 `ProviderService::switch`：那是上游代码，且 `operator_switch_tier`
    // 正当地要调它。
    crate::operator::reject_if_managed(id)?;
    ProviderService::switch(state, app_type, id)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub fn switch_provider_test_hook(
    state: &AppState,
    app_type: AppType,
    id: &str,
) -> Result<SwitchResult, AppError> {
    switch_provider_internal(state, app_type, id)
}

/// 切换供应商。
///
/// ## `quit_chatgpt`：切 codex 的供应商时也要退 ChatGPT
///
/// **不只是 LoongPort 的托管档位需要这个。** ChatGPT 桌面版自带一份 codex 核心、与命令行
/// codex 共用同一个 `~/.codex`，所以它在跑的时候切**任何** codex 供应商（包括 cc-switch
/// 自带的那些）都有同一个问题：它启动时读了旧的 `config.toml`，不重启就仍连旧的；
/// 而且**它退出时会回写那个文件**，可能把我们刚写的覆盖掉。
///
/// 原来只有 `operator_switch_tier` 编排了「退 → 切 → 重开」，于是从 provider 页切
/// cc-switch 自带的供应商时没有这层保护 —— 维护者实测指出的正是这一点。
///
/// 编排复用 [`crate::operator::chatgpt_app::around`]（与切档位共用同一份实现，
/// 不复制第二遍）。`abort_on_unconfirmed_exit = false`：切供应商只写 `config.toml`，
/// 退不掉也照常切 + 提示手动重启。
///
/// **`None` = 不碰 ChatGPT**，这是给托盘快切 / deeplink 导入 / 项目快照那些既有调用点留的
/// 默认行为（它们没有弹确认框的机会，而未经用户同意就关掉他正开着的 app 是不能接受的）。
/// 只有前端在弹过确认框、用户同意之后才传 `Some(true)`。
#[tauri::command]
pub async fn switch_provider(
    app_handle: tauri::AppHandle,
    app: String,
    id: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    // 与 `operator_switch_tier` 同一条判据：**只有 codex 才需要管 ChatGPT**
    // （那个 app 只读 `~/.codex`，切 claude / gemini 时去退它是扰民 ——
    // 关掉用户正开着的、与本次切换毫无关系的对话）。
    let quit_chatgpt = quit_chatgpt.unwrap_or(false) && matches!(app_type, AppType::Codex);

    let emit_handle = app_handle.clone();
    let emit_app_type = app_type.clone();
    let emit_id = id.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;

        let switch_once = || switch_provider_internal(state.inner(), app_type.clone(), &id);

        if !quit_chatgpt {
            return switch_once().map_err(|e| e.to_string());
        }

        let (mut switched, chatgpt) =
            crate::operator::chatgpt_app::around(false, switch_once).map_err(|e| e.to_string())?;
        // ChatGPT 那边的非致命问题（平台没实现自动退出、重开失败）要一起带给用户 ——
        // 前端已经在 toast 里逐条显示 warnings。
        switched.warnings.extend(chatgpt.warnings);
        Ok(switched)
    })
    .await
    .map_err(|e| format!("供应商切换任务执行失败: {e}"))?;

    // 切成功了就广播 —— **不只给 provider 页看**，见 `emit_provider_switched` 的文档。
    if result.is_ok() {
        emit_provider_switched(&emit_handle, &emit_app_type, &emit_id);
    }
    result
}

/// 三条通用 provider 命令撞到 LoongPort 托管档位时必须报错，而不是照做。
///
/// 只测「守卫拦不拦」这一层：守卫在任何 DB / 文件动作之前返回，所以给一个内存库的 AppState
/// 就够了 —— 反过来说，普通 id 那条路径这里**不测**（它要真实 HOME 与 live 配置，
/// 已由 `tests/provider_commands.rs` 的集成测试覆盖）。
#[cfg(test)]
mod managed_guard_tests {
    use super::*;
    use crate::database::Database;
    use std::sync::Arc;

    fn empty_state() -> AppState {
        AppState::new(Arc::new(Database::memory().expect("in-memory database")))
    }

    /// 库里**已经有**那条托管档位的 state。
    ///
    /// 就地编辑那几条测试必须用它：新守卫的判据是「这个托管 id 对应一条已存在的
    /// 记录吗」，空库里当然对应不上 —— 用 `empty_state` 测就会把「正常编辑」
    /// 和「凭空造一个托管 id」混为一谈（那正是这道守卫要区分的两件事）。
    fn state_with_managed_tier(id: &str) -> AppState {
        let state = empty_state();
        let existing = Provider::with_id(
            id.to_string(),
            "provision 生成的名字".to_string(),
            serde_json::json!({"auth": {"OPENAI_API_KEY": "sk-orig"}}),
            None,
        );
        state
            .db
            .save_provider(AppType::Codex.as_str(), &existing)
            .expect("预置托管档位");
        state
    }

    /// 真的调生成器拿 id，而不是手写一个 `loongport-xxx` 字面量：
    /// 这样前缀真变了的那天，测试跟着生成器走、守卫失配才会被别的断言抓到。
    fn managed_id() -> String {
        crate::operator::provision::provider_id_for("https://bestapi.store", Some(1), 42)
    }

    fn assert_managed_guard_error(err: &AppError) {
        let text = err.to_string();
        assert!(
            text.contains("LoongPort"),
            "错误必须是托管守卫那条（指路到运营商区），实际: {text}"
        );
    }

    #[test]
    fn switch_provider_rejects_managed_tier() {
        let err = switch_provider_internal(&empty_state(), AppType::Codex, &managed_id())
            .expect_err("切托管档位必须被拦");
        assert_managed_guard_error(&err);
    }

    /// ⭐ **改托管档位的内容不再被拦**（2026-08-03 放开）。
    ///
    /// 旧行为是连内容编辑一起拒，理由是「手工改了下次 provision 就被覆盖」。
    /// 那个前提后来不成立了：provision 改成「已存在的档位只换 sk、保住用户的编辑」
    /// （`provision::patch_api_key`）—— 于是那道守卫变成在挡一件已经安全的事。
    ///
    /// 运营商区的「编辑配置」按钮走的就是这条命令。它红了说明守卫被改回原样，
    /// 而那会让那个按钮的保存**静默失败**（用户改完点保存，收到一条「请在供应商页
    /// 顶部的运营商区操作」—— 而他就在那一区里）。
    ///
    /// 断言的是**没被守卫拦下**，不是整体成功：这里的 state 是空库，
    /// 所以 `ProviderService::update` 自己会因为「provider 不存在 / 缺 auth 配置」
    /// 报错。判据是那条错误**不是**守卫那条指路文案。
    #[test]
    fn update_provider_allows_editing_a_managed_tier_in_place() {
        let id = managed_id();
        let provider = Provider::with_id(
            id.clone(),
            "用户改的名字".to_string(),
            serde_json::json!({"auth": {"OPENAI_API_KEY": "sk-1"}}),
            None,
        );
        // id 不变（`original_id` 传的就是同一个）⇒ 不是改名 ⇒ 守卫不该介入。
        let state = state_with_managed_tier(&id);
        let result = update_provider_internal(&state, AppType::Codex, Some(id.as_str()), provider);

        if let Err(e) = result {
            let text = e.to_string();
            assert!(
                !text.contains("LoongPort"),
                "内容编辑被托管守卫拦下了 —— 「编辑配置」那个按钮会保存失败。实际: {text}"
            );
        }
    }

    /// `original_id` 为 `None`（调用方没说改没改名）时**也不该拦**。
    ///
    /// 上游的编辑页在 id 未变时就是这么传的（`App.tsx` 的 `handleEditProvider`
    /// 只在改过 id 时给 `originalId`）—— 拿 `None` 当「可能改名」来拦，
    /// 等于把最常见的那条编辑路径又堵回去了。
    #[test]
    fn update_provider_allows_a_managed_tier_when_original_id_is_omitted() {
        let id = managed_id();
        let provider = Provider::with_id(
            id.clone(),
            "改个名".to_string(),
            serde_json::json!({"auth": {"OPENAI_API_KEY": "sk-1"}}),
            None,
        );
        let state = state_with_managed_tier(&id);
        if let Err(e) = update_provider_internal(&state, AppType::Codex, None, provider) {
            let text = e.to_string();
            assert!(
                !text.contains("LoongPort"),
                "originalId 省略时被误当成改名拦下了。实际: {text}"
            );
        }
    }

    /// ⭐ **编辑一个「当前」生图档位并保存必须成功。**
    ///
    /// ## 这条闸守的是实测漏掉的一条路径
    ///
    /// 生图栏没有 live 配置，所以 `write_live_snapshot` 对它返回 `Err`。而
    /// `ProviderService::update` 在「这条正是当前项」时会去写 live ⇒ 保存报错，
    /// **可 DB 里已经存好了** ⇒ 界面提示「保存失败」而改动其实生效了，用户再点
    /// 一次还是报错，永远得不到成功的反馈。
    ///
    /// 判断收在 `write_live_with_common_config`（全部写 live 路径的唯一收口）。
    /// 这条闸从命令层验它真的通了 —— `update_provider_allows_editing_a_managed_tier_in_place`
    /// 抓不到：它对 `Err` 只断言「文案不含 LoongPort」，而这个失败的文案是
    /// 「生图档位不写入任何 CLI 配置…」。
    #[test]
    fn editing_the_current_image_tier_saves_without_error() {
        let id = managed_id();
        let state = empty_state();
        let existing = Provider::with_id(
            id.clone(),
            "生图档".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-orig" },
                "config": "model = \"gpt-image-2\"\n",
            }),
            None,
        );
        state
            .db
            .save_provider(AppType::CodexImage.as_str(), &existing)
            .expect("预置生图档位");
        // **设成当前项** —— 那正是触发写 live 的条件。
        state
            .db
            .set_current_provider(AppType::CodexImage.as_str(), &id)
            .expect("设当前项");

        let edited = Provider::with_id(
            id.clone(),
            "我改的名字".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-orig" },
                "config": "model = \"gpt-image-2\"\nmodel_reasoning_effort = \"low\"\n",
            }),
            None,
        );
        update_provider_internal(&state, AppType::CodexImage, Some(id.as_str()), edited)
            .expect("编辑当前生图档位必须能保存 —— 它没有 live 配置，写 live 该是空操作");
    }

    /// ⭐ **review 抓出的绕过**：`originalId` 省略 + 自选一个托管 id ⇒ 凭空造出托管项。
    ///
    /// 初版判据是「id 变了没有」（`original_id.is_some_and(|old| old != provider.id)`），
    /// 而 `original_id` 传 `None` 时 `ProviderService::update` 会拿 `provider.id`
    /// 自己当原 id（`services/provider/mod.rs:2608`）⇒「不是改名」成立 ⇒ 守卫不介入
    /// ⇒ `save_provider` 是 upsert ⇒ 一条 `loongport-*` 记录被写进库。
    ///
    /// 后果：那条记录从普通 provider 列表里消失（前端按前缀过滤）、转而出现在
    /// 运营商区里，而那一区的「恢复默认配置」会拿运营商的默认值把用户自己配的
    /// 东西整份覆盖掉。
    ///
    /// 现在的判据是「这个托管 id 得对应一条**已存在**的记录」，所以空库里必拒。
    #[test]
    fn update_provider_rejects_inventing_a_managed_id_out_of_thin_air() {
        let provider = Provider::with_id(
            managed_id(),
            "凭空造的".to_string(),
            serde_json::json!({"auth": {"OPENAI_API_KEY": "sk-1"}}),
            None,
        );
        // `original_id = None`（初版正是这条路绕过去的），库里也没有这个 id。
        let err = update_provider_internal(&empty_state(), AppType::Codex, None, provider)
            .expect_err("凭空造一个托管 id 必须被拦");
        assert_managed_guard_error(&err);
    }

    /// `add_provider` 也不许造托管 id。
    ///
    /// 这个口子**不是本轮引入的** —— `add` 从来就没有守卫，所以上面那条绕过就算堵了，
    /// 走 `add_provider` 仍能一步到位造出伪装的托管项。手已经伸到这个文件、现在
    /// 做得了 ⇒ 一并清（CLAUDE.md defer 准入闸的空间维度）。
    #[test]
    fn add_provider_rejects_a_managed_id() {
        let provider = Provider::with_id(
            managed_id(),
            "伪装尝试".to_string(),
            serde_json::json!({"auth": {"OPENAI_API_KEY": "sk-1"}}),
            None,
        );
        let err = add_provider_internal(&empty_state(), AppType::Codex, provider, false)
            .expect_err("新增托管 id 必须被拦");
        assert_managed_guard_error(&err);
    }

    /// 反向那个口子：把**普通** provider 改成托管 id ⇒ 伪装成托管项。
    ///
    /// 后果不是显示错乱那么轻：它会出现在运营商区里，而那一区的
    /// 「恢复默认配置」会拿运营商的默认值把用户自己配的东西**整份覆盖**。
    #[test]
    fn update_provider_rejects_renaming_a_plain_provider_into_a_managed_id() {
        let provider = Provider::with_id(
            managed_id(),
            "伪装尝试".to_string(),
            serde_json::json!({}),
            None,
        );
        let err =
            update_provider_internal(&empty_state(), AppType::Codex, Some("custom-1"), provider)
                .expect_err("把普通 provider 改成托管 id 必须被拦");
        assert_managed_guard_error(&err);
    }

    #[test]
    fn update_provider_rejects_renaming_managed_tier_to_plain_id() {
        // `originalId` 是托管的、新 id 是普通的 —— 只判新 id 就会让托管项被改名脱管。
        let provider = Provider::with_id(
            "custom-escaped".to_string(),
            "脱管尝试".to_string(),
            serde_json::json!({}),
            None,
        );
        let managed = managed_id();
        let err = update_provider_internal(
            &empty_state(),
            AppType::Codex,
            Some(managed.as_str()),
            provider,
        )
        .expect_err("把托管档位改名成普通 id 必须被拦");
        assert_managed_guard_error(&err);
    }

    #[test]
    fn delete_provider_rejects_managed_tier() {
        let err = delete_provider_internal(&empty_state(), AppType::Codex, &managed_id())
            .expect_err("删托管档位必须被拦");
        assert_managed_guard_error(&err);
    }
}

fn import_default_config_internal(state: &AppState, app_type: AppType) -> Result<bool, AppError> {
    if matches!(app_type, AppType::GrokBuild) {
        // 官方登录态（live 语法合法且无自定义模型表）+ 用户手动导入：
        // 导入的正确结果是让 Grok Official 成为当前供应商，而非报错。
        // 只挂在命令层 = 只有手动动作可达；启动自动导入走 service 层、
        // 官方态照旧报错静默跳过，删掉的官方条目不会被重启复活
        //（全项目惯例：启动自动导入只产出 default，从不产出官方条目）。
        if let Ok(settings) = crate::grok_config::read_grok_live_settings() {
            let config = settings
                .get("config")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if crate::grok_config::is_official_live_config(config) {
                state.db.ensure_official_seed_by_id(
                    crate::database::GROKBUILD_OFFICIAL_PROVIDER_ID,
                    AppType::GrokBuild,
                )?;
                state.db.set_current_provider(
                    app_type.as_str(),
                    crate::database::GROKBUILD_OFFICIAL_PROVIDER_ID,
                )?;
                crate::settings::set_current_provider(
                    &app_type,
                    Some(crate::database::GROKBUILD_OFFICIAL_PROVIDER_ID),
                )?;
                return Ok(true);
            }
        }

        // Safety net: 与 claude-desktop 导入同语义 —— 用户主动点导入是"重新
        // 整理该表"的隐式信号，把官方入口补回来。覆盖导入必然失败的场景
        //（live 文件缺失 / TOML 语法错误 / 残缺的自定义配置），避免
        // "报错 + 空列表"死胡同。失败只 warn，不影响导入主流程。
        if let Err(e) = state.db.ensure_official_seed_by_id(
            crate::database::GROKBUILD_OFFICIAL_PROVIDER_ID,
            AppType::GrokBuild,
        ) {
            log::warn!("Failed to ensure grokbuild-official seed during import: {e}");
        }
    }

    let imported = ProviderService::import_default_config(state, app_type.clone())?;

    if imported {
        // Extract common config snippet (mirrors old startup logic in lib.rs)
        if state
            .db
            .should_auto_extract_config_snippet(app_type.as_str())?
        {
            match ProviderService::extract_common_config_snippet(state, app_type.clone()) {
                Ok(snippet) if !snippet.is_empty() && snippet != "{}" => {
                    let _ = state
                        .db
                        .set_config_snippet(app_type.as_str(), Some(snippet));
                    let _ = state
                        .db
                        .set_config_snippet_cleared(app_type.as_str(), false);
                }
                _ => {}
            }
        }

        ProviderService::migrate_legacy_common_config_usage_if_needed(state, app_type.clone())?;
    }

    Ok(imported)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub fn import_default_config_test_hook(
    state: &AppState,
    app_type: AppType,
) -> Result<bool, AppError> {
    import_default_config_internal(state, app_type)
}

#[tauri::command]
pub fn import_default_config(state: State<'_, AppState>, app: String) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    import_default_config_internal(&state, app_type).map_err(Into::into)
}

#[tauri::command]
pub async fn get_claude_desktop_status(
    state: State<'_, AppState>,
) -> Result<crate::claude_desktop_config::ClaudeDesktopStatus, String> {
    let proxy_running = state.proxy_service.is_running().await;
    crate::claude_desktop_config::get_status(state.db.as_ref(), proxy_running)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_claude_desktop_default_routes(
) -> Vec<crate::claude_desktop_config::ClaudeDesktopDefaultRoute> {
    crate::claude_desktop_config::default_proxy_routes()
}

#[tauri::command]
pub fn import_claude_desktop_providers_from_claude(
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let claude_providers = state
        .db
        .get_all_providers(AppType::Claude.as_str())
        .map_err(|e| e.to_string())?;
    let existing_ids = state
        .db
        .get_provider_ids(AppType::ClaudeDesktop.as_str())
        .map_err(|e| e.to_string())?;

    let mut imported = 0usize;
    for provider in claude_providers.values() {
        if existing_ids.contains(&provider.id) {
            continue;
        }

        let mut desktop_provider = provider.clone();
        desktop_provider.in_failover_queue = false;
        let meta = desktop_provider.meta.get_or_insert_with(Default::default);

        if crate::claude_desktop_config::is_compatible_direct_provider(provider)
            && claude_provider_models_are_claude_safe(provider)
        {
            meta.claude_desktop_mode = Some(ClaudeDesktopMode::Direct);
        } else if let Some(routes) = suggested_claude_desktop_routes(provider) {
            meta.claude_desktop_mode = Some(ClaudeDesktopMode::Proxy);
            meta.claude_desktop_model_routes = routes;
        } else {
            continue;
        }

        state
            .db
            .save_provider(AppType::ClaudeDesktop.as_str(), &desktop_provider)
            .map_err(|e| e.to_string())?;
        imported += 1;
    }

    // Safety net: 用户可能手动删除过 claude-desktop-official seed。
    // 用户主动点 import 是"重新整理 ClaudeDesktop 表"的隐式信号，把官方入口补回来。
    // 失败只 warn，不影响 imported 主流程；imported 计数语义保持纯净。
    if let Err(e) = state.db.ensure_official_seed_by_id(
        crate::database::CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
        AppType::ClaudeDesktop,
    ) {
        log::warn!("Failed to ensure claude-desktop-official seed during import: {e}");
    }

    Ok(imported)
}

#[tauri::command]
pub fn ensure_claude_desktop_official_provider(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .db
        .ensure_official_seed_by_id(
            crate::database::CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
            AppType::ClaudeDesktop,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ensure_codex_official_provider(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .db
        .ensure_official_seed_by_id(crate::database::CODEX_OFFICIAL_PROVIDER_ID, AppType::Codex)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ensure_grokbuild_official_provider(state: State<'_, AppState>) -> Result<bool, String> {
    state
        .db
        .ensure_official_seed_by_id(
            crate::database::GROKBUILD_OFFICIAL_PROVIDER_ID,
            AppType::GrokBuild,
        )
        .map_err(|e| e.to_string())
}

fn claude_provider_models_are_claude_safe(provider: &Provider) -> bool {
    let Some(env) = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())
    else {
        return true;
    };

    [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
    ]
    .into_iter()
    .filter_map(|key| env.get(key).and_then(|value| value.as_str()))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .all(crate::claude_desktop_config::is_claude_safe_model_id)
}

pub(crate) fn suggested_claude_desktop_routes(
    provider: &Provider,
) -> Option<std::collections::HashMap<String, crate::provider::ClaudeDesktopModelRoute>> {
    let env = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())?;
    let mut routes = std::collections::HashMap::new();
    let supports_1m_default = !matches!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.provider_type.as_deref()),
        Some("github_copilot") | Some("codex_oauth") | Some("xai_oauth")
    );

    fn add_route(
        routes: &mut std::collections::HashMap<String, crate::provider::ClaudeDesktopModelRoute>,
        env: &serde_json::Map<String, serde_json::Value>,
        route_key: &str,
        env_key: &str,
        supports_1m_default: bool,
    ) {
        let Some(raw_model) = env
            .get(env_key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };

        // Claude 端 env 值可能带 [1M] 后缀；Claude Desktop schema 不接受后缀，
        // 改用 supports1m 字段表达 1M 能力。在 import 边界做单向翻译。
        let marker = crate::claude_desktop_config::ONE_M_CONTEXT_MARKER.as_bytes();
        let raw_bytes = raw_model.as_bytes();
        let has_1m_marker = raw_bytes.len() >= marker.len()
            && raw_bytes[raw_bytes.len() - marker.len()..].eq_ignore_ascii_case(marker);
        let stripped_model: &str = if has_1m_marker {
            raw_model[..raw_model.len() - marker.len()].trim_end()
        } else {
            raw_model
        };
        if stripped_model.is_empty() {
            return;
        }
        let effective_supports_1m = supports_1m_default || has_1m_marker;
        let explicit_label_override = env
            .get(&format!("{env_key}_NAME"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let label_override = explicit_label_override.clone().or_else(|| {
            (!crate::claude_desktop_config::is_claude_safe_model_id(stripped_model))
                .then(|| stripped_model.to_string())
        });

        // 何时覆盖既有 label_override：原本为空 / 这次来的是 explicit _NAME /
        // 既有值只是 stripped_model 派生的占位（被 explicit 或更具体的值挤掉）。
        let should_overwrite = |existing: Option<&str>| {
            existing.is_none()
                || explicit_label_override.is_some()
                || existing == Some(stripped_model)
        };

        let merge_into = |existing: &mut crate::provider::ClaudeDesktopModelRoute| {
            let merged = existing.supports_1m.unwrap_or(false) || effective_supports_1m;
            existing.supports_1m = Some(merged);
            if should_overwrite(existing.label_override.as_deref()) {
                existing.label_override = label_override.clone();
            }
        };

        if let Some(existing) = routes
            .values_mut()
            .find(|existing| existing.model == stripped_model)
        {
            merge_into(existing);
            return;
        }

        routes
            .entry(route_key.to_string())
            .and_modify(merge_into)
            .or_insert_with(|| crate::provider::ClaudeDesktopModelRoute {
                model: stripped_model.to_string(),
                label_override,
                supports_1m: Some(effective_supports_1m),
            });
    }

    for spec in crate::claude_desktop_config::DEFAULT_PROXY_ROUTES {
        add_route(
            &mut routes,
            env,
            spec.route_id,
            spec.env_key,
            supports_1m_default,
        );
    }

    // 三个 default env_key 全空时用 ANTHROPIC_MODEL 派生兜底路由。
    if routes.is_empty() {
        let primary_route = crate::claude_desktop_config::DEFAULT_PROXY_ROUTES[0].route_id;
        add_route(
            &mut routes,
            env,
            primary_route,
            "ANTHROPIC_MODEL",
            supports_1m_default,
        );
    }

    (!routes.is_empty()).then_some(routes)
}

#[allow(non_snake_case)]
#[tauri::command]
pub async fn queryProviderUsage(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    copilot_state: State<'_, CopilotAuthState>,
    xai_state: State<'_, XaiOAuthState>,
    #[allow(non_snake_case)] providerId: String, // 使用 camelCase 匹配前端
    app: String,
) -> Result<crate::provider::UsageResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    // inner 可能以两种形式失败：
    //   1) 返回 Ok(UsageResult { success: false, .. }) —— 确定性失败（401、脚本
    //      报错、未知供应商等）。写进 UsageCache 并刷新托盘，让
    //      format_script_summary 的 success 守卫生效、suffix 自然消失。
    //   2) 返回 Err(String) —— 瞬时传输失败（网络/超时）及 DB/Copilot fetch 等。
    //      不写失败快照、不 emit：保留上一份托盘快照，与前端 react-query reject
    //      保留上次 data 的语义一致；否则失败快照会经 useUsageCacheBridge 盲写
    //      回 query 缓存，抹掉 reject 本该保留的旧值。
    let inner = query_provider_usage_inner(
        &state,
        &copilot_state,
        &xai_state,
        app_type.clone(),
        &providerId,
    )
    .await;
    if let Ok(snapshot) = &inner {
        let payload = serde_json::json!({
            "kind": "script",
            "appType": app_type.as_str(),
            "providerId": &providerId,
            "data": snapshot,
        });
        if let Err(e) = app_handle.emit(USAGE_CACHE_UPDATED, payload) {
            log::error!("emit {USAGE_CACHE_UPDATED} (script) 失败: {e}");
        }
        state
            .usage_cache
            .put_script(app_type, providerId, snapshot.clone());
        crate::tray::schedule_tray_refresh(&app_handle);
    }
    inner
}

/// Resolve `(base_url, api_key)` for native usage queries, delegating to the
/// per-app resolver on `Provider`. Missing provider → empty credentials.
fn resolve_native_credentials(app_type: &AppType, provider: Option<&Provider>) -> (String, String) {
    provider
        .map(|p| p.resolve_usage_credentials(app_type))
        .unwrap_or_default()
}

fn resolve_coding_plan_credentials(
    app_type: &AppType,
    provider: Option<&Provider>,
    usage_script: Option<&crate::provider::UsageScript>,
) -> (String, String) {
    let is_zenmux = usage_script
        .and_then(|s| s.coding_plan_provider.as_deref())
        .map(|provider| provider.eq_ignore_ascii_case("zenmux"))
        .unwrap_or(false);

    if !is_zenmux {
        return resolve_native_credentials(app_type, provider);
    }

    let script_base_url = usage_script
        .and_then(|s| s.base_url.as_deref())
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let script_api_key = usage_script
        .and_then(|s| s.api_key.as_deref())
        .unwrap_or("")
        .to_string();

    if !script_base_url.is_empty() && !script_api_key.is_empty() {
        return (script_base_url, script_api_key);
    }

    let native = resolve_native_credentials(app_type, provider);
    if !native.0.is_empty() && !native.1.is_empty() {
        native
    } else {
        (script_base_url, script_api_key)
    }
}

async fn query_provider_usage_inner(
    state: &AppState,
    copilot_state: &CopilotAuthState,
    xai_state: &XaiOAuthState,
    app_type: AppType,
    provider_id: &str,
) -> Result<crate::provider::UsageResult, String> {
    // 从数据库读取供应商信息，检查特殊模板类型
    let providers = state
        .db
        .get_all_providers(app_type.as_str())
        .map_err(|e| format!("Failed to get providers: {e}"))?;
    let provider = providers.get(provider_id);
    let usage_script = provider
        .and_then(|p| p.meta.as_ref())
        .and_then(|m| m.usage_script.as_ref());
    let template_type = usage_script
        .and_then(|s| s.template_type.as_deref())
        .unwrap_or("");

    // ── GitHub Copilot 专用路径 ──
    if template_type == TEMPLATE_TYPE_GITHUB_COPILOT {
        let copilot_account_id = provider
            .and_then(|p| p.meta.as_ref())
            .and_then(|m| m.managed_account_id_for(TEMPLATE_TYPE_GITHUB_COPILOT));

        let auth_manager = copilot_state.0.read().await;
        let usage = match copilot_account_id.as_deref() {
            Some(account_id) => auth_manager
                .fetch_usage_for_account(account_id)
                .await
                .map_err(|e| format!("Failed to fetch Copilot usage: {e}"))?,
            None => auth_manager
                .fetch_usage()
                .await
                .map_err(|e| format!("Failed to fetch Copilot usage: {e}"))?,
        };
        let premium = &usage.quota_snapshots.premium_interactions;
        let used = premium.entitlement - premium.remaining;

        return Ok(crate::provider::UsageResult {
            success: true,
            data: Some(vec![crate::provider::UsageData {
                plan_name: Some(usage.copilot_plan),
                remaining: Some(premium.remaining as f64),
                total: Some(premium.entitlement as f64),
                used: Some(used as f64),
                unit: Some(COPILOT_UNIT_PREMIUM.to_string()),
                is_valid: Some(true),
                invalid_message: None,
                extra: Some(format!("Reset: {}", usage.quota_reset_date)),
            }]),
            error: None,
        });
    }

    // ── Coding Plan 专用路径 ──
    if template_type == TEMPLATE_TYPE_TOKEN_PLAN {
        let (base_url, api_key) =
            resolve_coding_plan_credentials(&app_type, provider, usage_script);

        // 火山方舟用账号 AK/SK 签名查询用量（存于 usage_script，与推理 api_key 分离）；
        // 其他供应商为 None，service 层沿用 api_key。
        let access_key_id = usage_script.and_then(|s| s.access_key_id.clone());
        let secret_access_key = usage_script.and_then(|s| s.secret_access_key.clone());
        // 智谱团队版：显式 provider 标识 + 组织/项目 ID（与个人版智谱 base_url 相同，
        // 靠 coding_plan_provider == "zhipu_team" 在 service 层路由）。
        let coding_plan_provider = usage_script.and_then(|s| s.coding_plan_provider.clone());
        let team_organization_id = usage_script.and_then(|s| s.team_organization_id.clone());
        let team_project_id = usage_script.and_then(|s| s.team_project_id.clone());

        let quota = crate::services::coding_plan::get_coding_plan_quota(
            &base_url,
            &api_key,
            access_key_id.as_deref(),
            secret_access_key.as_deref(),
            coding_plan_provider.as_deref(),
            team_organization_id.as_deref(),
            team_project_id.as_deref(),
        )
        .await
        .map_err(|e| format!("Failed to query coding plan: {e}"))?;

        // 将 SubscriptionQuota 转换为 UsageResult
        if !quota.success {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: quota.error,
            });
        }

        // ZenMux 的 tier 携带 USD 额度信息，需要编码为 JSON extra
        let has_usd = quota
            .tiers
            .first()
            .map(|t| t.used_value_usd.is_some())
            .unwrap_or(false);
        let plan_label = quota
            .credential_message
            .as_deref()
            .and_then(|msg| msg.split(' ').next())
            .map(|tier| format!("ZenMux·{}", tier.to_uppercase()));
        let mut first_tier = true;

        let data: Vec<crate::provider::UsageData> = quota
            .tiers
            .iter()
            .map(|tier| {
                let total = 100.0;
                let used = tier.utilization;
                let remaining = total - used;
                let extra = if has_usd {
                    let mut extra_json = serde_json::json!({
                        "resetsAt": tier.resets_at,
                    });
                    if let Some(v) = tier.used_value_usd {
                        extra_json["usedValueUsd"] = serde_json::json!(v);
                    }
                    if let Some(v) = tier.max_value_usd {
                        extra_json["maxValueUsd"] = serde_json::json!(v);
                    }
                    if first_tier {
                        if let Some(ref label) = plan_label {
                            extra_json["planLabel"] = serde_json::json!(label);
                        }
                        first_tier = false;
                    }
                    Some(extra_json.to_string())
                } else {
                    tier.resets_at.clone()
                };
                crate::provider::UsageData {
                    plan_name: Some(tier.name.clone()),
                    remaining: Some(remaining),
                    total: Some(total),
                    used: Some(used),
                    unit: Some("%".to_string()),
                    is_valid: Some(true),
                    invalid_message: None,
                    extra,
                }
            })
            .collect();

        return Ok(crate::provider::UsageResult {
            success: true,
            data: if data.is_empty() { None } else { Some(data) },
            error: None,
        });
    }

    // ── 官方余额查询路径 ──
    if template_type == TEMPLATE_TYPE_BALANCE {
        // 按 app 区分的凭据存储格式提取 Base URL 与 API Key
        let (base_url, api_key) = resolve_native_credentials(&app_type, provider);

        return crate::services::balance::get_balance(&base_url, &api_key)
            .await
            .map_err(|e| format!("Failed to query balance: {e}"));
    }

    // ── 官方订阅额度查询路径 ──
    if template_type == TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION {
        if !usage_script.map(|s| s.enabled).unwrap_or(false) {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: Some("Usage query is disabled".to_string()),
            });
        }

        // xAI OAuth 托管供应商的额度属绑定的 SuperGrok 账号，而非所在 app 的
        // CLI 凭据（对 codex/claude 而言 CLI 凭据是 ChatGPT/Claude 订阅，跨了
        // 订阅体系，查出来的数字张冠李戴）。
        let quota = if provider.map(Provider::is_xai_oauth).unwrap_or(false) {
            let account_id = provider
                .and_then(|p| p.meta.as_ref())
                .and_then(|m| m.managed_account_id_for("xai_oauth"));
            crate::commands::xai_oauth::query_xai_oauth_quota_for(xai_state, account_id).await?
        } else {
            crate::services::subscription::get_subscription_quota(app_type.as_str())
                .await
                .map_err(|e| format!("Failed to query subscription quota: {e}"))?
        };

        if !quota.success {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: quota.error.or(quota.credential_message),
            });
        }

        let data: Vec<crate::provider::UsageData> = quota
            .tiers
            .iter()
            .map(|tier| crate::provider::UsageData {
                plan_name: Some(tier.name.clone()),
                remaining: Some(100.0 - tier.utilization),
                total: Some(100.0),
                used: Some(tier.utilization),
                unit: Some("%".to_string()),
                is_valid: Some(true),
                invalid_message: None,
                extra: tier.resets_at.clone(),
            })
            .collect();

        return Ok(crate::provider::UsageResult {
            success: true,
            data: if data.is_empty() { None } else { Some(data) },
            error: None,
        });
    }

    // ── 通用 JS 脚本路径 ──
    ProviderService::query_usage(state, app_type, provider_id)
        .await
        .map_err(|e| e.to_string())
}

#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn testUsageScript(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
    app: String,
    #[allow(non_snake_case)] scriptCode: String,
    timeout: Option<u64>,
    #[allow(non_snake_case)] apiKey: Option<String>,
    #[allow(non_snake_case)] baseUrl: Option<String>,
    #[allow(non_snake_case)] accessToken: Option<String>,
    #[allow(non_snake_case)] userId: Option<String>,
    #[allow(non_snake_case)] templateType: Option<String>,
) -> Result<crate::provider::UsageResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::test_usage_script(
        state.inner(),
        app_type,
        &providerId,
        &scriptCode,
        timeout.unwrap_or(10),
        apiKey.as_deref(),
        baseUrl.as_deref(),
        accessToken.as_deref(),
        userId.as_deref(),
        templateType.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_live_provider_settings(app: String) -> Result<serde_json::Value, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::read_live_settings(app_type).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_api_endpoints(
    urls: Vec<String>,
    #[allow(non_snake_case)] timeoutSecs: Option<u64>,
) -> Result<Vec<EndpointLatency>, String> {
    SpeedtestService::test_endpoints(urls, timeoutSecs)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_custom_endpoints(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
) -> Result<Vec<crate::settings::CustomEndpoint>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::get_custom_endpoints(state.inner(), app_type, &providerId)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_custom_endpoint(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::add_custom_endpoint(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_custom_endpoint(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::remove_custom_endpoint(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_endpoint_last_used(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::update_endpoint_last_used(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_providers_sort_order(
    state: State<'_, AppState>,
    app: String,
    updates: Vec<ProviderSortUpdate>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::update_sort_order(state.inner(), app_type, updates).map_err(|e| e.to_string())
}

use crate::provider::UniversalProvider;
use std::collections::HashMap;
use tauri::AppHandle;

#[derive(Clone, serde::Serialize)]
pub struct UniversalProviderSyncedEvent {
    pub action: String,
    pub id: String,
}

fn emit_universal_provider_synced(app: &AppHandle, action: &str, id: &str) {
    let _ = app.emit(
        UNIVERSAL_PROVIDER_SYNCED,
        UniversalProviderSyncedEvent {
            action: action.to_string(),
            id: id.to_string(),
        },
    );
}

#[tauri::command]
pub fn get_universal_providers(
    state: State<'_, AppState>,
) -> Result<HashMap<String, UniversalProvider>, String> {
    ProviderService::list_universal(state.inner()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_universal_provider(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<UniversalProvider>, String> {
    ProviderService::get_universal(state.inner(), &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn upsert_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: UniversalProvider,
) -> Result<bool, String> {
    let id = provider.id.clone();
    let result =
        ProviderService::upsert_universal(state.inner(), provider).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "upsert", &id);

    Ok(result)
}

#[tauri::command]
pub fn delete_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    let result =
        ProviderService::delete_universal(state.inner(), &id).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "delete", &id);

    Ok(result)
}

#[tauri::command]
pub fn sync_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    let result =
        ProviderService::sync_universal_to_apps(state.inner(), &id).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "sync", &id);

    Ok(result)
}

#[tauri::command]
pub fn import_opencode_providers_from_live(state: State<'_, AppState>) -> Result<usize, String> {
    crate::services::provider::import_opencode_providers_from_live(state.inner())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_opencode_live_provider_ids() -> Result<Vec<String>, String> {
    crate::opencode_config::get_providers()
        .map(|providers| providers.keys().cloned().collect())
        .map_err(|e| e.to_string())
}

// ============================================================================
// OpenClaw 专属命令 → 已迁移至 commands/openclaw.rs
// ============================================================================

#[cfg(test)]
mod import_claude_desktop_tests {
    use super::suggested_claude_desktop_routes;
    use crate::provider::{Provider, ProviderMeta};
    use serde_json::json;

    fn make_provider(env: serde_json::Value, provider_type: Option<&str>) -> Provider {
        let mut p = Provider::with_id(
            "test-claude".to_string(),
            "Test".to_string(),
            json!({ "env": env }),
            None,
        );
        if let Some(pt) = provider_type {
            p.meta = Some(ProviderMeta {
                provider_type: Some(pt.to_string()),
                ..ProviderMeta::default()
            });
        }
        p
    }

    #[test]
    fn route_strips_1m_suffix_and_sets_supports_1m() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-5-20250929[1M]",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        let r = routes.get("claude-sonnet-5").expect("sonnet route present");
        assert_eq!(r.model, "claude-sonnet-4-5-20250929");
        assert!(
            !r.model.to_ascii_lowercase().contains("[1m]"),
            "model must not contain [1m] suffix"
        );
        assert_eq!(r.label_override, None);
        assert_eq!(r.supports_1m, Some(true));
    }

    #[test]
    fn route_preserves_model_without_suffix() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "kimi-k2",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        let r = routes.get("claude-sonnet-5").expect("sonnet route present");
        assert_eq!(r.model, "kimi-k2");
        assert_eq!(r.label_override.as_deref(), Some("kimi-k2"));
        // 默认 provider_type 缺省 → supports_1m_default = true
        assert_eq!(r.supports_1m, Some(true));
    }

    #[test]
    fn route_uses_claude_code_model_name_as_label_override() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "kimi-k2",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Kimi K2",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        let r = routes.get("claude-sonnet-5").expect("sonnet route present");
        assert_eq!(r.model, "kimi-k2");
        assert_eq!(r.label_override.as_deref(), Some("Kimi K2"));
    }

    #[test]
    fn route_1m_suffix_overrides_provider_type_default() {
        // github_copilot 默认 supports_1m_default = false，但 [1M] 后缀应强制 true
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "gpt-5-codex[1M]",
            }),
            Some("github_copilot"),
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        let r = routes.get("claude-sonnet-5").expect("sonnet route present");
        assert_eq!(r.model, "gpt-5-codex");
        assert_eq!(r.label_override.as_deref(), Some("gpt-5-codex"));
        assert_eq!(r.supports_1m, Some(true));
    }

    #[test]
    fn route_github_copilot_without_suffix_keeps_false() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "gpt-5-codex",
            }),
            Some("github_copilot"),
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        let r = routes.get("claude-sonnet-5").expect("sonnet route present");
        assert_eq!(r.model, "gpt-5-codex");
        assert_eq!(r.label_override.as_deref(), Some("gpt-5-codex"));
        assert_eq!(r.supports_1m, Some(false));
    }

    #[test]
    fn same_upstream_across_three_aliases_merges_to_one_route() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "MiniMax-M2",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "MiniMax-M2",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "MiniMax-M2",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        assert_eq!(routes.len(), 1, "three aliases → one merged route");
        let r = routes.get("claude-sonnet-5").expect("merged route present");
        assert_eq!(r.model, "MiniMax-M2");
        assert_eq!(r.label_override.as_deref(), Some("MiniMax-M2"));
    }

    #[test]
    fn same_upstream_with_partial_1m_marker_takes_or_aggregation() {
        // sonnet 带 [1M]，opus/haiku 不带 → 合并后 supports_1m == Some(true)
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "MiniMax-M2[1M]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "MiniMax-M2",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "MiniMax-M2",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        assert_eq!(routes.len(), 1);
        let r = routes.get("claude-sonnet-5").expect("merged route present");
        assert_eq!(r.supports_1m, Some(true));
    }

    #[test]
    fn different_upstream_models_produce_separate_routes() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "GLM-4.6",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "GLM-4-Air",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "GLM-4-Flash",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        assert_eq!(routes.len(), 3);
        assert_eq!(routes.get("claude-sonnet-5").unwrap().model, "GLM-4.6");
        assert_eq!(routes.get("claude-opus-5").unwrap().model, "GLM-4-Air");
        assert_eq!(routes.get("claude-haiku-4-5").unwrap().model, "GLM-4-Flash");
        assert_eq!(
            routes
                .get("claude-sonnet-5")
                .unwrap()
                .label_override
                .as_deref(),
            Some("GLM-4.6")
        );
    }

    #[test]
    fn anthropic_model_fallback_only_triggers_when_empty() {
        // 三个 default env_key 都不填，仅 ANTHROPIC_MODEL
        let p = make_provider(
            json!({
                "ANTHROPIC_MODEL": "kimi-k2",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        assert_eq!(routes.len(), 1);
        let r = routes
            .get("claude-sonnet-5")
            .expect("fallback route present");
        assert_eq!(r.model, "kimi-k2");
        assert_eq!(r.label_override.as_deref(), Some("kimi-k2"));
    }

    #[test]
    fn existing_claude_prefix_not_duplicated() {
        let p = make_provider(
            json!({
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-5-20250929",
            }),
            None,
        );
        let routes = suggested_claude_desktop_routes(&p).expect("routes built");
        assert!(routes.contains_key("claude-sonnet-5"));
        assert!(!routes.contains_key("claude-claude-sonnet-4-5-20250929"));
        assert_eq!(
            routes.get("claude-sonnet-5").expect("route").label_override,
            None
        );
    }
}

#[cfg(test)]
mod native_query_credentials_tests {
    use super::{resolve_coding_plan_credentials, resolve_native_credentials};
    use crate::app_config::AppType;
    use crate::provider::{Provider, UsageScript};
    use serde_json::json;

    fn usage_script(
        coding_plan_provider: Option<&str>,
        base_url: Option<&str>,
        api_key: Option<&str>,
    ) -> UsageScript {
        UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: Some(10),
            api_key: api_key.map(str::to_string),
            base_url: base_url.map(str::to_string),
            access_token: None,
            user_id: None,
            template_type: Some("token_plan".to_string()),
            auto_query_interval: None,
            coding_plan_provider: coding_plan_provider.map(str::to_string),
            access_key_id: None,
            secret_access_key: None,
            team_organization_id: None,
            team_project_id: None,
        }
    }

    #[test]
    fn delegates_to_provider_for_codex() {
        let provider = Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            json!({
                "auth": { "OPENAI_API_KEY": "sk-codex" },
                "config": "model_provider = \"deepseek\"\n\
                           [model_providers.deepseek]\n\
                           base_url = \"https://api.deepseek.com\"\n",
            }),
            None,
        );
        let (base_url, api_key) = resolve_native_credentials(&AppType::Codex, Some(&provider));
        assert_eq!(base_url, "https://api.deepseek.com");
        assert_eq!(api_key, "sk-codex");
    }

    #[test]
    fn missing_provider_yields_empty() {
        let (base_url, api_key) = resolve_native_credentials(&AppType::Codex, None);
        assert!(base_url.is_empty());
        assert!(api_key.is_empty());
    }

    #[test]
    fn zenmux_coding_plan_uses_script_credentials_first() {
        let provider = Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.zenmux.example/v1",
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider"
                }
            }),
            None,
        );
        let script = usage_script(
            Some("zenmux"),
            Some("https://script.zenmux.example/api/usage/"),
            Some("sk-script"),
        );

        let (base_url, api_key) =
            resolve_coding_plan_credentials(&AppType::Claude, Some(&provider), Some(&script));

        assert_eq!(base_url, "https://script.zenmux.example/api/usage");
        assert_eq!(api_key, "sk-script");
    }

    #[test]
    fn zenmux_coding_plan_falls_back_to_provider_credentials() {
        let provider = Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.zenmux.example/v1",
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider"
                }
            }),
            None,
        );
        let script = usage_script(Some("zenmux"), Some("https://script.zenmux.example"), None);

        let (base_url, api_key) =
            resolve_coding_plan_credentials(&AppType::Claude, Some(&provider), Some(&script));

        assert_eq!(base_url, "https://provider.zenmux.example/v1");
        assert_eq!(api_key, "sk-provider");
    }
}
