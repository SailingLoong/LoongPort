//! 档位 provision 管线：协议分派、候选组装、落库与 prune、live 刷新。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::provision;
use crate::relay::site_config;

/// 备好密钥的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionSummary {
    pub tiers: Vec<TierInfo>,
    /// 失败的分组与原因。**不为空也不代表整体失败** —— 成功的那些照样能用。
    pub failures: Vec<FailureInfo>,
    /// 这次新建了几把 sk（其余是认领到的已有 Key）。
    ///
    /// 给用户看的：第二次进来应该是 0（全部认领到），若每次都在新建，说明认领逻辑有问题
    /// 正在给他账号里堆垃圾 Key。
    pub keys_created: usize,
    /// Imported non-managed providers removed because LoongPort now owns the same credential.
    pub merged_providers: Vec<MergedProviderInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedProviderInfo {
    pub name: String,
    pub app_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureInfo {
    pub group_name: String,
    pub reason: String,
}

#[derive(Clone)]
pub(crate) struct ManagedProvisionCandidate {
    pub(crate) provider_id: String,
    pub(crate) app_type: AppType,
    pub(crate) group_id: String,
    pub(crate) group_name: String,
    pub(crate) rate_multiplier: Option<f64>,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) models: Option<Vec<String>>,
    pub(crate) roles: Option<provision::ClaudeRoleModels>,
    pub(crate) allow_image_generation: Option<bool>,
    pub(crate) api_base_url: String,
}

#[derive(Default)]
pub(crate) struct ManagedProvisionBatch {
    pub(crate) account_id: Option<i64>,
    /// 登录/添加时按约定路径探测到的站点声明（relay/site_config.rs）。
    /// `None` = 站长没放文件或拉取失败（探测语义，静默降级）。
    pub(crate) site_declaration: Option<crate::relay::site_config::SiteDeclaredConfig>,
    pub(crate) candidates: Vec<ManagedProvisionCandidate>,
    /// Upstream-observed `(app_type, provider_id)` pairs that stale pruning must retain.
    pub(crate) observed_keep: std::collections::HashSet<(String, String)>,
    pub(crate) failures: Vec<FailureInfo>,
    pub(crate) keys_created: usize,
}

pub(crate) fn newapi_app_types() -> [AppType; 3] {
    [AppType::Claude, AppType::Codex, AppType::Gemini]
}

/// 把「一个 new-api 分组被观察到了」这个事实写进 keep 白名单，**按分类只保真实槽位**。
///
/// keep 是 `prune_stale_tiers` 的白名单：不在里面的托管档位会被清掉。
///
/// - 分类已知（`models = Some(非空目录)`）：只保候选真正会落的槽位 —— 纯生图分组
///   只占生图栏，它历史上被扇出到 claude/codex/gemini 的旧投影随这次 provision 一并
///   清掉（那是「生图模型被写成聊天模型」的必 404 档位）。
/// - 分类未知（`models = None`：目录拉不到、或分组对账没走完）：保全四个槽位 ——
///   keep 的语义是「观察到了就别删」，宁可留旧档也不误删。
///
/// ⚠️ `Some` 必须非空：空目录的 [`provision::is_pure_image_group`] 是真空真，
/// 会把「读不出目录」误判成「纯生图」（见该函数文档的闸说明）。
pub(crate) fn newapi_keep_insert(
    keep: &mut std::collections::HashSet<(String, String)>,
    site_origin: &str,
    account_id: i64,
    identity: &newapi::GroupIdentity,
    models: Option<&[String]>,
) {
    let provider_id = provision::newapi_provider_id_for(site_origin, account_id, &identity.0);
    let app_types: Vec<AppType> = match models {
        Some(models) if provision::is_pure_image_group(models) => vec![AppType::CodexImage],
        Some(_) => newapi_app_types().into(),
        // 分类未知：保全四个槽位（上面那份名单 + 生图栏），不再手写第二遍。
        None => {
            let mut slots = newapi_app_types().to_vec();
            slots.push(AppType::CodexImage);
            slots
        }
    };
    for app_type in app_types {
        keep.insert((app_type.as_str().to_string(), provider_id.clone()));
    }
}

pub(crate) fn newapi_candidates_for_group(
    site_origin: &str,
    account_id: i64,
    group: &newapi_provision::ReconciledGroup,
    models: &[String],
    tables: &provision::ModelSelectionTables,
) -> Vec<ManagedProvisionCandidate> {
    let provider_id = provision::newapi_provider_id_for(site_origin, account_id, &group.identity.0);
    // 纯生图分组只出生图候选，不扇出到三个聊天栏：把生图模型写成聊天模型是**调用必
    // 404** 的档位（claude 拿 nano-banana-2 当 ANTHROPIC_MODEL、gemini 拿 gpt-image-2
    // 当 GEMINI_MODEL —— 2026-09-05 真实 new-api 站点的生图分组实测踩中）。判据与 sub2api 路径的
    // `image_tier_app_type` 同源：[`provision::is_pure_image_group`]。
    let app_types: Vec<AppType> = if provision::is_pure_image_group(models) {
        vec![AppType::CodexImage]
    } else {
        newapi_app_types().into()
    };
    app_types
        .into_iter()
        .map(|app_type| {
            let picked = provision::pick_tier_models_with(&app_type, Some(models), tables);
            ManagedProvisionCandidate {
                provider_id: provider_id.clone(),
                app_type,
                group_id: group.identity.0.clone(),
                group_name: group.name.clone(),
                rate_multiplier: group.rate_multiplier,
                api_key: group.api_key.clone(),
                model: picked.main,
                models: Some(models.to_vec()),
                roles: picked.claude_roles,
                allow_image_generation: None,
                // NewAPI exposes one OpenAI-compatible root. Per-app suffixes are projected by
                // `api::base_url_for`, so no persisted sub2api base belongs here.
                api_base_url: String::new(),
            }
        })
        .collect()
}

pub(crate) fn normalize_newapi_model_catalog(models: Option<Vec<String>>) -> Option<Vec<String>> {
    models
        .map(provision::normalize_model_names)
        .filter(|models| !models.is_empty())
}

fn newapi_reconcile_stage(stage: newapi_provision::ReconcileStage) -> &'static str {
    match stage {
        newapi_provision::ReconcileStage::Create => "token_create",
        newapi_provision::ReconcileStage::Relist => "token_relist",
        newapi_provision::ReconcileStage::Reveal => "token_reveal",
        newapi_provision::ReconcileStage::DeleteStale => "token_delete_stale",
    }
}

pub(crate) async fn provision_backend(
    op: &creds::Relay,
    browser_fallback: Option<api::BrowserApiFallback>,
) -> Result<ManagedProvisionBatch, AppError> {
    // 一次 provision 解析一次选型表（内置 + 远端覆盖），两个 backend 共用 ——
    // sub2api 与 newapi 的选型纪律必须来自同一份数据（尺子 1.4：一个事实一个 owner）。
    let tables = provision::ModelSelectionTables::resolve();
    // 站点声明探测与 backend 无关（约定路径是 LoongPort 的约定，newapi 站长同样能放）：
    // 404/失败/格式不认都是 None，绝不打断登录主流程。
    let site_declaration = crate::relay::site_config::fetch_site_declaration(&op.site_origin).await;
    if site_declaration.is_some() {
        log::info!(
            "{}",
            crate::diagnostics::DiagnosticEvent::new("relay.site_config", "declaration_found")
                .field_display("site", crate::url_for_log(&op.site_origin))
        );
    }
    match op.backend_kind {
        discovery::BackendKind::Sub2Api => {
            let mut client = api::Client::new(
                &op.site_origin,
                &op.auth_token,
                op.account_id,
                op.user_agent.as_deref(),
                op.cf_clearance.as_deref(),
            )?;
            // 登录后自动备 key 时登录窗还开着：站点被指纹级防护拦下（403 HTML）时，
            // 由登录窗代拉（见 `browser_api_fallback`）。测试等无 UI 上下文传 `None`。
            if let Some(fallback) = browser_fallback {
                client = client.with_browser_fallback(fallback);
            }
            let mut result = provision::provision(&client, &tables).await?;
            provision::sort_tiers(&mut result.tiers);
            let keys_created = result
                .tiers
                .iter()
                .filter(|targeted| targeted.tier.key_was_created)
                .count();
            let candidates = result
                .tiers
                .into_iter()
                .map(|targeted| {
                    let tier = targeted.tier;
                    ManagedProvisionCandidate {
                        provider_id: provision::provider_id_for(
                            &op.site_origin,
                            op.account_id,
                            tier.group_id,
                        ),
                        app_type: targeted.app_type,
                        group_id: tier.group_id.to_string(),
                        group_name: tier.group_name,
                        rate_multiplier: Some(tier.rate_multiplier),
                        api_key: tier.api_key,
                        model: tier.model,
                        models: tier.models,
                        roles: tier.roles,
                        allow_image_generation: Some(tier.allow_image_generation),
                        api_base_url: op.api_base_url.clone(),
                    }
                })
                .collect();
            Ok(ManagedProvisionBatch {
                account_id: op.account_id,
                site_declaration,
                candidates,
                observed_keep: std::collections::HashSet::new(),
                failures: result
                    .failures
                    .into_iter()
                    .map(|(group_name, reason)| FailureInfo { group_name, reason })
                    .collect(),
                keys_created,
            })
        }
        discovery::BackendKind::NewApi => {
            let client = newapi::NewApiClient::with_optional_account_id(
                &op.site_origin,
                &op.auth_token,
                op.account_id,
            )?;
            let account = client.account().await?;
            if op.account_id.is_some() && op.account_id != Some(account.id) {
                return Err(AppError::Config(
                    "NewAPI 登录态所属账号与本地中转站账号不一致，请重新登录".into(),
                ));
            }
            let result = newapi_provision::reconcile_for_account(&client, account.id).await?;

            let mut batch = ManagedProvisionBatch {
                account_id: Some(result.account_id),
                site_declaration,
                candidates: Vec::new(),
                // 分类感知的槽位在下面的分组循环里逐个补（`newapi_keep_insert`），
                // 不再一次性保三个聊天栏 —— 纯生图分组只保生图栏。
                observed_keep: std::collections::HashSet::new(),
                failures: result
                    .failures
                    .into_iter()
                    .map(|failure| FailureInfo {
                        group_name: failure
                            .group_identity
                            .map(|identity| identity.0)
                            .unwrap_or_else(|| "NewAPI token cleanup".into()),
                        reason: format!(
                            "{}: {}",
                            newapi_reconcile_stage(failure.stage),
                            failure.reason
                        ),
                    })
                    .collect(),
                keys_created: result.tokens_created,
            };

            // 对账没走完的分组（建/列/揭示密钥失败，拿不到 sk 也拉不了目录）：
            // 分类未知，保全四个槽位 —— 观察到了就别删。
            let reconciled: std::collections::HashSet<newapi::GroupIdentity> = result
                .groups
                .iter()
                .map(|group| group.identity.clone())
                .collect();
            for identity in &result.observed_groups {
                if !reconciled.contains(identity) {
                    newapi_keep_insert(
                        &mut batch.observed_keep,
                        &op.site_origin,
                        result.account_id,
                        identity,
                        None,
                    );
                }
            }

            for group in result.groups {
                let models = match api::list_models(&op.site_origin, &group.api_key).await {
                    Ok(models) => match normalize_newapi_model_catalog(models) {
                        Some(models) => models,
                        None => {
                            batch.failures.push(FailureInfo {
                                group_name: group.name,
                                reason: "model_catalog: /v1/models 未返回可用模型目录".into(),
                            });
                            // 目录拉不到 = 分类未知：保全四个槽位，别把旧档误删。
                            newapi_keep_insert(
                                &mut batch.observed_keep,
                                &op.site_origin,
                                result.account_id,
                                &group.identity,
                                None,
                            );
                            continue;
                        }
                    },
                    Err(error) => {
                        batch.failures.push(FailureInfo {
                            group_name: group.name,
                            reason: format!("model_catalog: {error}"),
                        });
                        newapi_keep_insert(
                            &mut batch.observed_keep,
                            &op.site_origin,
                            result.account_id,
                            &group.identity,
                            None,
                        );
                        continue;
                    }
                };
                newapi_keep_insert(
                    &mut batch.observed_keep,
                    &op.site_origin,
                    result.account_id,
                    &group.identity,
                    Some(&models),
                );
                batch.candidates.extend(newapi_candidates_for_group(
                    &op.site_origin,
                    result.account_id,
                    &group,
                    &models,
                    &tables,
                ));
            }
            Ok(batch)
        }
    }
}

/// 拉分组、为每组备好 sk、写成 provider 记录 —— provision 的唯一入口
/// （`relay_refresh` 命令在探活后调它；曾经的独立 `relay_provision` 命令前端零调用，
/// 2026-09-07 删除，见 git 历史）。
///
/// ## 一次探全部平台，各归各的 tab（2026-08-03 改）
///
/// **不吃 `app` 参数** —— 每个分组落到哪个 CLI 由它自己的 `platform` 决定
/// （`openai → codex`、`anthropic → claude`、`gemini → gemini`、`grok → grokbuild`），
/// 见 [`provision::provision`]。用户在任何一个 tab 登录一次，全部平台的档位都备好了。
///
/// ### 为什么去掉那个参数（它曾经是 bug 的根源）
///
/// 原来签名吃 `app`，于是「拉哪些分组」和「写成什么形状」都由调用方决定 ——
/// 在 claude tab 点「获取密钥」时**拉的是 openai 分组、却写成 claude 的配置形状**
/// （openai 的 sk 配在 `ANTHROPIC_BASE_URL` 上，调用必失败），
/// 而用户看到的是「claude 页出现了 chatgpt 的分组」。
///
/// 根因是把「分组属于哪个 CLI」的决定权交给了调用方，而那是分组自身的属性。
/// 现在 `provision` 返回 [`provision::TargetedTier`]（分组 + 它该落到的 app_type），
/// 调用方不需要知道 platform 映射规则 —— 那才是低耦合。
///
/// 认不出配置形状的 CLI（`settings_config_for` 返回 `None`）在循环里跳过并计入
/// `failures`，不让整批失败。
///
/// ## `relay_id`：显式指定作用于哪个中转站（2026-08-03 加）
///
/// 原来它只吃 `AppHandle`，靠 `creds::load()` 读「`is_current = 1` 的那一行」。
/// 于是多行并列的页面必须先 `set_current(id)` 才能让它作用到对的账号上
/// （前端那个 `focusRelay`）—— 而 `is_current` 是**全局单例状态**：
///
/// 两个中转站同时 provision 时，B 的 `set_current(B)` 会改掉 A 那次操作的目标，
/// A 后续的 balance / refresh 全串到 B 上。前端当时是用「任一操作进行中就禁用所有行」
/// 兜住的 —— **那是拿全局禁用换正确性，修的是症状**：中转站之间本来毫无依赖，
/// 用户点 A 的按钮却发现 B、C 的按钮全灰了。
///
/// 现在把目标变成参数，全局状态不再参与定位 ⇒ 各行真正独立、可并发。
/// 这也正是「中转站（登录态）一个模块、分组（sk）一个模块」该有的样子：
/// 分组操作显式说明「给哪个中转站」，而不是去读一个由 UI 顺手改掉的全局变量。
pub(crate) async fn refresh_relay_provision(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
) -> Result<ProvisionSummary, AppError> {
    let op = usable_relay(app_handle, relay_id).await?;
    let op = backfill_account_identity(app_handle, op).await;
    provision_relay(app_handle, &op).await
}

async fn provision_relay(
    app_handle: &tauri::AppHandle,
    op: &creds::Relay,
) -> Result<ProvisionSummary, AppError> {
    let batch = provision_backend(op, Some(browser_api_fallback(app_handle))).await?;
    let state = app_handle.state::<AppState>();
    let result = persist_provision_batch(state.inner(), op, batch);
    mark_pricing_after_success(state.inner(), op.id, chrono::Utc::now().timestamp(), result)
}

pub(crate) fn mark_pricing_after_success<T>(
    state: &AppState,
    relay_id: i64,
    synced_at: i64,
    result: Result<T, AppError>,
) -> Result<T, AppError> {
    let value = result?;
    with_conn(state, |conn| {
        creds::mark_pricing_synced(conn, relay_id, synced_at)
    })?;
    Ok(value)
}

pub(crate) fn persist_provision_batch(
    state: &AppState,
    op: &creds::Relay,
    mut batch: ManagedProvisionBatch,
) -> Result<ProvisionSummary, AppError> {
    let mut tiers = Vec::new();
    let mut merged_providers = Vec::new();
    // NewAPI fills this from the complete upstream inventory before reveal/model/write.
    // sub2api candidates are inserted before their local write for the same retention property.
    let mut keep = std::mem::take(&mut batch.observed_keep);
    let mut refresh_live: Vec<AppType> = Vec::new();
    for (idx, candidate) in batch.candidates.into_iter().enumerate() {
        let app_type = &candidate.app_type;
        let provider_id = candidate.provider_id.clone();
        // 先取出来：`candidate.group_name` 下面会被 move 进 failures，
        // 而倍率在那之后还要用。
        let rate_multiplier = candidate.rate_multiplier;
        let display_name = provision::provider_display_name(&op.site_name, &candidate.group_name);
        keep.insert((app_type.as_str().to_string(), provider_id.clone()));

        let base_url = api::base_url_for(app_type, &op.site_origin, &candidate.api_base_url);
        // 下面这串分支是「带目录平台」的**生成器形状分派**（claude/gemini 走
        // roles+models、codex/grokbuild 走 models）——「哪些平台带目录」这个名单
        // 的事实唯源是 [`provision::model_catalog_apps`]，加平台时两边一起动
        //（reset 侧与回归测试都按那份名单对齐）。
        let defaults = if matches!(app_type, AppType::Claude) {
            provision::settings_config_with_roles_and_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.roles.clone(),
                candidate.models.as_deref(),
                provision::ProvisionStyle::default(),
            )
        } else if matches!(app_type, AppType::Codex) {
            provision::settings_config_with_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.models.as_deref(),
            )
        } else if matches!(app_type, AppType::Gemini) {
            // gemini 同样落模型目录（只收 gemini-* 家族，见生成侧注释）
            provision::settings_config_with_roles_and_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                None,
                candidate.models.as_deref(),
                provision::ProvisionStyle::default(),
            )
        } else if matches!(app_type, AppType::GrokBuild) {
            // grokbuild 同样落模型目录（收全部文本模型，见生成侧注释）——
            // 主界面「支持的模型」芯片与托盘「模型」子菜单都从它取
            provision::settings_config_with_models(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
                candidate.models.as_deref(),
            )
        } else {
            provision::settings_config_for(
                app_type,
                &candidate.api_key,
                &display_name,
                &base_url,
                &candidate.model,
            )
        };
        let Some(defaults) = defaults else {
            batch.failures.push(FailureInfo {
                group_name: candidate.group_name,
                reason: format!("{}: 还不能生成配置", app_type.as_str()),
            });
            continue;
        };

        let existing = state
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .ok()
            .flatten();
        // 旧档位上已有的「站点推荐配置」标注：重放路径不重新应用声明（见下），
        // 但配置里还带着当初声明的参数，标注不能丢。
        let existing_site_declared = existing
            .as_ref()
            .and_then(|old| old.meta.as_ref())
            .and_then(|meta| meta.site_declared_origin.clone());
        let is_first_import = existing.is_none();
        let user_edited = match state.db.get_user_edited(app_type.as_str(), &provider_id) {
            Ok(user_edited) => user_edited,
            Err(error) => {
                batch.failures.push(FailureInfo {
                    group_name: candidate.group_name,
                    reason: format!("{}: 读取用户编辑标记失败: {error}", app_type.as_str()),
                });
                continue;
            }
        };

        let mut settings_config = match existing {
            Some(old) => {
                if user_edited {
                    let mut kept = old.settings_config;
                    if provision::patch_api_key(&mut kept, app_type, &candidate.api_key) {
                        kept
                    } else {
                        log::warn!("{display_name} 的配置里找不到放密钥的位置，已重置为默认配置");
                        defaults
                    }
                } else {
                    preserve_supported_model(app_type, defaults, &old.settings_config)
                }
            }
            None => defaults,
        };

        // 站点声明段（relay/site_config.rs，上游提案 Wei-Shaw/sub2api#6518 先行落地）：
        // **只在首次导入时自动应用**——开箱即站长版默认（deny-list 拦执行面，端点与
        // 凭证不开放覆盖）。重放路径（非用户编辑的 defaults 重算）刻意不应用：
        // `preserve_supported_model` 保住的可能是省心模式选下的模型，每次 provision
        // 都拿站长默认覆盖它会让自动选档失效；站长更新后的同步走手动输入命令
        // （用户显式动作）。同源校验失败按「没有声明」处理，不阻断建档。
        let mut site_declared_origin: Option<String> = None;
        if is_first_import {
            if let Some(declared) = &batch.site_declaration {
                if site_config::validate_same_origin(&declared.site_origin, &op.site_origin).is_ok()
                {
                    if let Some(platform) = platform_map::platform_for_app(app_type) {
                        if let Some(segment) = declared.segment_for(platform) {
                            match site_config::apply_segment_to_app(
                                app_type,
                                segment,
                                &mut settings_config,
                            ) {
                                Ok(true) => {
                                    site_declared_origin = Some(declared.site_origin.clone());
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    log::warn!(
                                        "{display_name} 的站点声明段应用失败（按内置默认继续）: {error}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();

        let provider = Provider {
            id: provider_id.clone(),
            name: display_name.clone(),
            settings_config,
            website_url: Some(op.site_origin.clone()),
            // aggregator 而不是 official：official 那条分类会触发一批只对官方订阅成立的
            // 逻辑（stale auth 清理、统一会话桶注入）。
            category: Some("aggregator".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(idx),
            notes: None,
            meta: {
                let mut meta = managed_meta(
                    app_type,
                    batch.account_id,
                    Some(crate::provider::LoongportGroupIdentity {
                        id: candidate.group_id.clone(),
                        name: candidate.group_name.clone(),
                    }),
                );
                meta.site_declared_origin = site_declared_origin.or(existing_site_declared);
                Some(meta)
            },
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };

        if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
            batch.failures.push(FailureInfo {
                group_name: candidate.group_name,
                reason: format!(
                    "{}: 保存档位 {display_name} 失败: {error}",
                    app_type.as_str()
                ),
            });
            continue;
        }

        // 倍率落库。**这是它唯一的写入点** —— 「刷新倍率」= 重新 provision，
        // 界面上就是「顶部刷新 / 更新可用分组 / 登录成功」那几下。
        //
        // 曾经它一个字都不存（`list_tiers_impl` 恒返回 `None`），靠一条独立命令在
        // 每次 reload 后**每个档位打一次 HTTP** 去补 —— 而 reload 挂在每个动作后面，
        // 于是切一次档位就把全部档位的倍率重查一遍。倍率是服务端定价，不是实时量。
        //
        // 写失败只 warn：档位已经存对了，不该因为一个显示值让「获取密钥」整个报失败。
        if let Err(error) =
            state
                .db
                .set_tier_rate_multiplier(app_type.as_str(), &provider_id, rate_multiplier)
        {
            log::warn!("记录档位 {provider_id} 的倍率失败（只影响显示）: {error}");
        }

        let merged_current = match provider_fingerprint::remove_unmanaged_duplicates(
            state.db.as_ref(),
            app_type,
            &provider,
        ) {
            Ok(merged) => merged,
            Err(error) => {
                batch.failures.push(FailureInfo {
                    group_name: candidate.group_name.clone(),
                    reason: format!("{}: 收编重复 provider 失败: {error}", app_type.as_str()),
                });
                Vec::new()
            }
        };
        let mut is_current = current == provider_id;
        if !merged_current.is_empty() {
            log::info!(
                "收编 {} 个重复的 {} provider：{}",
                merged_current.len(),
                app_type.as_str(),
                merged_current
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            );
            merged_providers.extend(merged_current.iter().map(|merged| MergedProviderInfo {
                name: merged.name.clone(),
                app_id: app_type.as_str().to_string(),
            }));
            if merged_current.iter().any(|m| m.was_current) {
                is_current = true;
                if !refresh_live.contains(app_type) {
                    refresh_live.push(app_type.clone());
                }
            }
        }

        if is_current && !refresh_live.contains(app_type) {
            refresh_live.push(app_type.clone());
        }

        tiers.push(TierInfo {
            is_current,
            provider_id,
            // **这条分组自己的 app_type**，不是调用方给的 —— 这一整段循环的前提就是
            // 「一次 provision 探全部平台」，写错会让前端把别的平台的档位算成自己的。
            app_id: app_type.as_str().to_string(),
            group_name: candidate.group_name,
            display_name,
            model: provision::selected_model(app_type, &provider.settings_config)
                .unwrap_or_default(),
            models: models_from_settings(&provider.settings_config),
            rate_multiplier,
            can_verify_models: verification_target::supports_app_type(app_type),
            user_edited: Some(user_edited),
            allow_image_generation: candidate.allow_image_generation,
            // provision 刚建档/重放完，标注以库里的 meta 为准（首次导入应用过
            // 声明的档位这里立刻就有值，前端不用等下一次 listRelays）。
            site_declared_origin: provider
                .meta
                .as_ref()
                .and_then(|meta| meta.site_declared_origin.clone()),
        });
    }

    refresh_live_for_current_tiers(state, &refresh_live);

    let removed = prune_stale_tiers(state, &op.site_origin, batch.account_id, &keep)?;
    if removed > 0 {
        log::info!("清理了 {removed} 个不再存在的档位（{}）", op.site_origin);
    }

    // 生图工具跟着「生图栏里有没有档位」对齐一次。见 `sync_imagegen_mcp` 的文档。
    //
    // ⚠️ **必须在 `prune_stale_tiers` 之后** —— 判据是「生图栏里还有档位吗」，
    // 而清理正是让最后一条生图档位消失的那一步。反过来的话，中转站下架全部生图分组后
    // 那个工具会留到下一次 provision 才撤掉，期间它每次调用都报「档位已经不在了」。
    //
    // 失败只 warn：档位已经存对了，不该因为一个 MCP 记录写不下去就把「获取密钥」
    // 整个报成失败（用户会以为连密钥都没拿到）。
    if let Err(e) = imagegen_mcp::sync_registration(state) {
        log::warn!("同步生图工具记录失败（生图可能暂时用不了）: {e}");
    }

    let retained_observed = keep.iter().any(|(app_type, provider_id)| {
        state
            .db
            .get_provider_by_id(provider_id, app_type)
            .ok()
            .flatten()
            .is_some()
    });
    if tiers.is_empty() && !retained_observed {
        let detail = batch
            .failures
            .iter()
            .map(|failure| format!("{}: {}", failure.group_name, failure.reason))
            .collect::<Vec<_>>()
            .join("；");
        return Err(AppError::Config(if detail.is_empty() {
            "没有可写入或可保留的托管档位".into()
        } else {
            format!("所有分组都没能备好托管档位（{detail}）")
        }));
    }

    Ok(ProvisionSummary {
        keys_created: batch.keys_created,
        tiers,
        failures: batch.failures,
        merged_providers,
    })
}

/// 把这些 app 的**当前项**的配置刷到 live 文件上。失败只 warn，不中断调用方。
///
/// ## 为什么必须有这一步（用户实测的症状）
///
/// CLI 读的是落地文件（`~/.codex/config.toml` 等），**不是我们的 DB**。所以凡是「改了
/// 当前项的 `settings_config` 却只 `save_provider`」的路径，结果都是：界面提示成功、
/// 库里也确实是新内容，而 **codex / claude 仍在用旧的**。
///
/// 而且用户没有自救手段 —— UI 认为这个档位已经是当前项（`isCurrent` 为 true），
/// 前端 `if (tier.isCurrent) return;` 会让「再点它一次」什么也不做。
///
/// 两个调用方：[`provision_relay`]（sk 被撤销后重建了一把）与
/// [`reset_tier_config_impl`]（把被改坏的配置恢复成默认）。
///
/// ## 为什么走 `sync_current_provider_for_app` 而不是 `switch`
///
/// 我们不是在**切换**当前项（它本来就是当前项），只是让落地配置追上 DB。那个 API 内部
/// 已处理代理接管（接管时更新备份而不是覆盖 live 文件）；而 `switch` 会跑一整套切换语义
/// ——接管态下走 `hot_switch_provider_inner`，还带「接管时不许切到官方供应商」那道拦，
/// 对一次「刷新密钥」是错的。
///
/// ## 失败只 warn
///
/// 记录已经存对了，用户手工切一次就能生效。不该因为落地文件写不下去（权限 / 文件被占）
/// 就把整次「获取密钥」报成失败 —— 那会让他以为连密钥都没拿到。
///
/// ⚠️ **收成一个函数是为了让命令层这一步可测**：两个调用方都吃 `&tauri::AppHandle`
/// （单测里造不出来），所以原来那两段内联代码**没有任何测试覆盖得到** ——
/// 第二路 review 实测：把它们注释掉，2578 条测试全绿。
pub(crate) fn refresh_live_for_current_tiers(state: &AppState, app_types: &[AppType]) {
    for app_type in app_types {
        if let Err(e) = ProviderService::sync_current_provider_for_app(state, app_type.clone()) {
            log::warn!(
                "刷新 {} 的当前配置失败（记录已保存，切换一次即生效）: {e}",
                app_type.as_str()
            );
        }
    }
}

/// 这条 provider 是不是「这个站 × 这个账号」名下的托管档位。
///
/// **收成一个函数是必要的，不是整理**：它同时被 [`prune_stale_tiers`]（要删哪些）与
/// [`apps_using_this_accounts_tiers`]（哪些不许删）消费 —— 两者必须对「归属」给出**同一个**
/// 答案。散成两份的后果是守卫与删除各认一套：守卫说「这条不是你的、不拦」，删除说
/// 「这条是你的、删了」⇒ 恰好绕过守卫删掉正在用的配置，而那正是守卫要防的事。
///
/// [`belongs_to_relay`]（严格版）才是余额 / 对账这些**归属**路径用的判据 ——
/// 两者 `(account_id, 档位标记)` 的 `(None, Some)` 那一格语义相反，见它那边的说明。
///
/// 三道判据缺一不可：
///
/// - `is_managed` —— 我们生成的（前缀 + 恰好 16 位小写 hex，即校验哈希形状）。用户手工加的 provider
///   一律不碰，错删它是不可挽回的。
/// - `website_url == site_origin` —— 只认这个站的。`provider_id` 是哈希，单向不可逆，
///   反推不出它属于谁。
/// - 账号维度 —— 同一个站可以挂多个账号。归属记在 `meta.loongportAccountId`，三种情况：
///   - **两边都有且不等** ⇒ 别人的，不是。
///   - **记录没有标记（`None`）** ⇒ 旧数据，只能靠站点判，**算是**。否则升级前生成的
///     孤儿档位永远清不掉（那正是 `prune_stale_tiers` 存在的理由），而它们必定 401。
///     代价是可能误伤同站另一账号的旧档位，但那些会在下次 provision 时重新生成并带上标记。
///   - **调用方不知道账号（`account_id` 为 `None`）** ⇒ 未登录的行（provision 不出档位）
///     或删站兜底路径，此时不按账号过滤。
pub(crate) fn belongs_to_account(
    provider: &Provider,
    site_origin: &str,
    account_id: Option<i64>,
) -> bool {
    if !is_managed(provider) {
        return false;
    }
    // 归属按注册域身份判：website_url 与 site_origin 都是 provision 时写入的
    // origin，同站不同子域拼写（面板域 vs API 域）也该认 —— 裸字符串相等会漏。
    if !same_site_identity(provider.website_url.as_deref(), Some(site_origin)) {
        return false;
    }
    match (
        account_id,
        provider.meta.as_ref().and_then(|m| m.loongport_account_id),
    ) {
        (Some(want), Some(owner)) => want == owner,
        _ => true,
    }
}

/// 这条 provider 是不是「这个 relay 行」名下的托管档位 —— **严格归属版**。
///
/// 与 [`belongs_to_account`] 只差一格：relay 行没登录（`account_id == None`）而档位
/// 记了**别人的**账号时，这里**不认**。两个方向语义相反，不能合并：
///
/// - [`belongs_to_account`] 服务清理 / 守卫 —— 宁可多认不误删（同站没记归属的旧档位
///   要能跟着清掉，`None` 一律「算是」）。
/// - 这里服务**把事实记到某一行头上**的路径（余额 sk 收集、扣费对账的成本归属）——
///   把别的账号的 sk / 成本算进未登录行，等于把 B 的消费记到 A 头上，比漏算更糟。
///   这也是 `apps_using_this_accounts_tiers` 当年要额外加 `account_id.is_some()` 的
///   同一类教训（见它那边的 ⚠️ 段）。
///
/// 判据即 `relay_balance_inputs` 原内联那份（`(None, Some(_)) => false`），收成函数
/// 供余额与 `relay_reconciliation`（`commands/reconcile.rs`）共用 —— 归属口径只有一份。
pub(crate) fn belongs_to_relay(
    provider: &Provider,
    site_origin: &str,
    account_id: Option<i64>,
) -> bool {
    if !is_managed(provider) {
        return false;
    }
    if !same_site_identity(provider.website_url.as_deref(), Some(site_origin)) {
        return false;
    }
    match (
        account_id,
        provider.meta.as_ref().and_then(|m| m.loongport_account_id),
    ) {
        (Some(want), Some(owner)) => want == owner,
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

/// 两个可选 origin 是否指向**同一站点**（注册域身份，None 与任何值都不相等）。
///
/// 收拢 `website_url` ↔ `site_origin` 这类归属判据：两边都是持久化的 origin 字符串，
/// 裸相等依赖「写入时同拼写」这个脆弱不变量 —— 同站的面板域与 API 域拼写不同
/// 就静默失配。身份归一见 [`crate::relay::identity`]。
pub(crate) fn same_site_identity(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            crate::relay::identity::site_domain(left) == crate::relay::identity::site_domain(right)
        }
        _ => false,
    }
}

/// 这个账号名下的档位里，有哪些**正是某个 app 的当前项**。返回 `(app_type, 档位名)`。
///
/// 给 [`remove_site_impl`] 当闸用：非空就说明删下去会毁掉一份**还能用**的配置
/// （见那边关于「为什么不能靠前端按钮态」的说明）。
///
/// 扫 `AppType::all()` 而不是某一个 app —— 这条闸的全部意义就在于**跨 app**：
/// 前端那道判据只看当前 tab，而档位可以是别的 app 的当前项。
///
/// 读不出某个 app 的列表时**跳过它**：那是「配置文件坏了 / 没权限」，而这条闸的作用是
/// 拦住已知的破坏。因为读不出来就把删除整个拦死，会让用户卡在一个他无法处置的错误上。
///
/// ## ⚠️ 这一行还没有 `account_id`（`None`）⇒ 一律不拦（第二路 review 抓出）
///
/// [`belongs_to_account`] 对 `account_id: None` 返回 `true`（"不按账号过滤"）。那个语义
/// 对**删除方向**是对的：删这一行时，同站那些没记归属的旧档位该跟着清掉。但对**守卫
/// 方向**反过来就错了 —— 它会把「同站另一个账号正在用的档位」算成「你名下的」。
///
/// 那种行真实可达，不是理论情况：`clear_credentials` 会把 `account_id` 置 `NULL`
/// （站点换了后端协议时走这条，见 [`load_validated_relay`]），而唯一索引把 `NULL`
/// 视为互不相等 ⇒ 它与那个已登录的行并存。此时用户删这个空行会看到
/// 「这个账号名下还有档位正在使用中：B 的档位（codex）」—— 点名一个**它并不拥有**的档位，
/// 而这一行压根没有任何档位。他唯一的出路是去 codex 把 B 切走，才能删掉一个空行。
///
/// （**登录态失效不再走那条路**：那边现在用 `creds::clear_session`，账号身份留着 ——
/// 见它的文档。所以 `None` 的来源只剩「从没登录过」与「协议变更」两种。）
///
/// 所以这里额外要求 `account_id.is_some()`：**认不出归属就不拦**。漏拦的代价是什么？
/// 没有 —— 没有 `account_id` 的行派生不出 provider id（`provider_id_for` 要它），
/// 所以它名下本来就不可能有档位，`prune_stale_tiers` 那一步也就没什么可删的。
pub(crate) fn apps_using_this_accounts_tiers(
    state: &AppState,
    site_origin: &str,
    account_id: Option<i64>,
) -> Vec<(AppType, String)> {
    let mut in_use = Vec::new();
    for app_type in AppType::all() {
        let Ok(list) = ProviderService::list(state, app_type.clone()) else {
            log::warn!(
                "检查「档位是否在用」时读不出 {} 的 provider 列表，跳过",
                app_type.as_str()
            );
            continue;
        };
        let Ok(current) = ProviderService::current(state, app_type.clone()) else {
            continue;
        };
        if current.is_empty() {
            continue;
        }
        if let Some(provider) = list.get(&current) {
            if belongs_to_account(provider, site_origin, account_id) && account_id.is_some() {
                in_use.push((app_type.clone(), provider.name.clone()));
            }
        }
    }
    in_use
}

/// 删掉「这个站在本地留着、但这次 provision 没再生成」的档位。返回删了几条。
///
/// ## 为什么必须有这一步（2026-08-03 加，用户实测发现）
///
/// `provision` 原来只 `save_provider`（新增或更新），**从不删**。于是任何一次
/// 「这条档位不该再存在了」都无法被纠正：
///
/// 1. **旧版本写错的记录永久残留**。曾经有个 bug 把 openai 分组写进了 claude 下
///    （`is_usable_for(&AppType::Codex)` 写死而外层已改成多平台），修掉代码之后
///    那些脏记录**点多少次「刷新」都不会消失** —— 用户看到「claude 页还有 codex
///    的分组」，只能怀疑是没修好。
/// 2. **中转站在网页端删掉一个分组，本地那条会一直留着**。用户点它 ⇒ 用一把
///    已失效的 sk 发请求 ⇒ 报一个看不懂的 401。
///
/// ## 判据必须精确，宁可漏删不可错删
///
/// 删除条件是**三个都成立**：
///
/// - `is_managed(id)` —— 是我们生成的（**前缀 + 恰好 16 位小写 hex**），用户手工加的 provider
///   一律不碰。这是最重要的一道：错删用户自己配的 provider 是不可挽回的。
/// - `website_url == 这次的 site_origin` —— **只清这个中转站的**。别的站的档位这次
///   压根没查（`provision` 只拉当前这一个站的分组），凭「这次没生成」删它们是错的。
/// - `id` 不在这次生成的集合里 —— 真的不该存在了。
///
/// ## 为什么扫全部 app_type 而不是只扫参数指定的那个
///
/// ⚠️ **依赖 `AppType::all()` 被同步维护** —— 它是**手工数组**（`app_config.rs:412`），
/// 不是从 enum 自动派生的。上游加一个 CLI 而漏改它，那个 app 下的串台脏记录就
/// **永远不被清理**（静默失效）。已加闸：`app_type_all_covers_every_variant`。
///
/// `provision` 现在一次探全部平台（分组自己的 `platform` 决定落到哪个 CLI），所以
/// 「不该存在的记录」可能在任何一个 app_type 下 —— 上面那个 claude/codex 串台的
/// bug 正是这种。只扫一个 app 就清不掉它。
///
/// ## 当前项也删 —— 一条不该存在的档位不配当「当前」
///
/// `ProviderService::delete` 会在「删的是当前项」时返回 `Err`（"无法删除当前正在
/// 使用的供应商"）。那条约束对**用户手工删除**是对的（防误删正在用的配置），但对
/// 这里不成立：走到这一步说明这条档位**服务端已经没有了**，它的 sk 是死的 ——
/// 留着当「当前项」只会让 CLI 拿一把失效密钥去发请求，报一个看不懂的 401。
/// 用户重新选一个可用的就好。
///
/// 所以当前项那条**直连 `state.db.delete_provider`** 绕过那层保护。
/// 悬空的 `is_current` 指针不用手工清：`settings::get_effective_current_provider`
/// 会验证 id 在库里是否存在，不存在就自动清掉本地 settings 并回落
/// （它的文档明说了这一条，正是为云同步导入后失效的场景写的）。
///
/// 非当前项走 `ProviderService::delete` —— 它顺带处理 additive-mode app 的
/// live config 清理，那套逻辑不该在这里重写一遍。
///
/// `AppType::all()` 是穷尽的（上游维护），加新 CLI 时这里自动覆盖。
///
/// 归属判据本身收在 [`belongs_to_account`]（删档位与删账号前的守卫共用一份）。
pub(crate) fn prune_stale_tiers(
    state: &AppState,
    site_origin: &str,
    account_id: Option<i64>,
    keep: &std::collections::HashSet<(String, String)>,
) -> Result<usize, AppError> {
    let mut removed = 0usize;
    for app_type in AppType::all() {
        let Ok(list) = ProviderService::list(state, app_type.clone()) else {
            // 某个 app 的配置读不出来（文件坏了 / 权限）时跳过它，别让整次 provision 失败 ——
            // 清理是收尾动作，主线（档位已经写好了）不该被它拖垮。
            log::warn!(
                "清理档位时读不出 {} 的 provider 列表，跳过",
                app_type.as_str()
            );
            continue;
        };

        // 读一次当前项，用来选删除路径（当前项要绕过 ProviderService 的保护）。
        let current = ProviderService::current(state, app_type.clone()).unwrap_or_default();

        for provider in list.values() {
            if !belongs_to_account(provider, site_origin, account_id) {
                continue;
            }
            // 判据是 **(app_type, id) 组合**，不是光看 id ——
            // 见 `persist_provision_batch` 里 `keep.insert` 那处的说明（同一个分组
            // 在两个 app 下是同一个 id，只看 id 会让串台的脏记录永远删不掉）。
            if keep.contains(&(app_type.as_str().to_string(), provider.id.clone())) {
                continue;
            }

            let is_current = provider.id == current;
            let outcome = if is_current {
                // 绕过 ProviderService::delete 的「不许删当前项」保护（见上方说明）。
                state.db.delete_provider(app_type.as_str(), &provider.id)
            } else {
                ProviderService::delete(state, app_type.clone(), &provider.id)
            };

            match outcome {
                Ok(()) => {
                    log::info!(
                        "删除不再存在的档位：{} ({} / {}){}",
                        provider.name,
                        app_type.as_str(),
                        provider.id,
                        if is_current {
                            " —— 它曾是当前项，请重新选一个档位"
                        } else {
                            ""
                        }
                    );
                    removed += 1;
                }
                // 删不掉如实记录但不中断 —— 其余的照样该清。
                Err(e) => log::warn!("删除档位 {} 失败: {e}", provider.id),
            }
        }
    }
    Ok(removed)
}
