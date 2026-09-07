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
                // `sub2api::base_url_for`, so no persisted sub2api base belongs here.
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
    browser_fallback: Option<sub2api::BrowserApiFallback>,
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
            let mut client = sub2api::Client::new(
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
                let models = match sub2api::list_models(&op.site_origin, &group.api_key).await {
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

        let base_url = sub2api::base_url_for(app_type, &op.site_origin, &candidate.api_base_url);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

    fn provider_with_id(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: "t".into(),
            settings_config: serde_json::json!({}),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn managed_detection_matches_generated_ids_only() {
        // 正面：provision 生成的 id 必须被认出来。
        let real = provision::provider_id_for("https://bestapi.store", Some(1), 42);
        assert!(is_managed(&provider_with_id(&real)));

        // 反面：用户自己加的 provider 不能被当成托管的（否则会被 provision 覆盖）。
        for id in ["custom-1", "codex-official", "", "LoongPort-1"] {
            assert!(!is_managed(&provider_with_id(id)), "id: {id}");
        }
    }

    #[test]
    fn provision_merge_removes_only_same_app_unmanaged_duplicate() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let site = "https://relay.example";
        let key = "sk-same";
        let settings = provision::settings_config_for(
            &app_type,
            key,
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");

        let duplicate = Provider {
            id: "cc-switch-duplicate".into(),
            name: "Imported duplicate".into(),
            settings_config: settings.clone(),
            website_url: Some(site.into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        let different_key = Provider {
            id: "cc-switch-different-key".into(),
            name: "Keep different key".into(),
            settings_config: provision::settings_config_for(
                &app_type,
                "sk-other",
                "Other",
                "https://relay.example/v1",
                "model-a",
            )
            .expect("codex 配置"),
            ..duplicate.clone()
        };
        let managed_duplicate = Provider {
            id: provision::provider_id_for(site, Some(1), 42),
            name: "Managed duplicate".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入重复项");
        db.save_provider(app_type.as_str(), &different_key)
            .expect("写入不同 key");
        db.save_provider(app_type.as_str(), &managed_duplicate)
            .expect("写入托管项");

        let merged =
            provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed_duplicate)
                .expect("收编不该失败");

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "Imported duplicate");
        assert!(db
            .get_provider_by_id("cc-switch-duplicate", app_type.as_str())
            .expect("查询")
            .is_none());
        assert!(db
            .get_provider_by_id("cc-switch-different-key", app_type.as_str())
            .expect("查询")
            .is_some());
        assert!(db
            .get_provider_by_id(&managed_duplicate.id, app_type.as_str())
            .expect("查询")
            .is_some());
    }

    #[test]
    fn provision_merge_reports_when_duplicate_was_current() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-current",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let duplicate = Provider {
            id: "cc-switch-current".into(),
            name: "Current imported duplicate".into(),
            settings_config: settings,
            website_url: Some("https://relay.example".into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入当前项");
        db.set_current_provider(app_type.as_str(), &duplicate.id)
            .expect("设为当前");

        let managed = Provider {
            id: provision::provider_id_for("https://relay.example", Some(1), 99),
            name: "Managed replacement".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &managed)
            .expect("写入托管替代项");

        let merged = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
            .expect("收编不该失败");

        assert_eq!(merged.len(), 1);
        assert!(merged[0].was_current);
        assert_eq!(
            db.get_current_provider(app_type.as_str())
                .expect("读取收编后的当前项")
                .as_deref(),
            Some(managed.id.as_str())
        );
    }

    #[test]
    fn provision_merge_rolls_back_duplicate_deletion_when_current_transfer_fails() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-current",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let duplicate = Provider {
            id: "cc-switch-current".into(),
            name: "Current imported duplicate".into(),
            settings_config: settings,
            website_url: Some("https://relay.example".into()),
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        db.save_provider(app_type.as_str(), &duplicate)
            .expect("写入当前项");
        db.set_current_provider(app_type.as_str(), &duplicate.id)
            .expect("设为当前");

        let managed = Provider {
            id: provision::provider_id_for("https://relay.example", Some(1), 99),
            name: "Managed replacement".into(),
            meta: Some(managed_meta(&app_type, Some(1), None)),
            ..duplicate.clone()
        };
        db.save_provider(app_type.as_str(), &managed)
            .expect("写入托管替代项");
        {
            let conn = db.conn.lock().expect("lock db");
            conn.execute_batch(&format!(
                "CREATE TRIGGER fail_managed_current
                     BEFORE UPDATE OF is_current ON providers
                     WHEN NEW.id = '{}' AND NEW.is_current = 1
                     BEGIN
                       SELECT RAISE(FAIL, 'injected current transfer failure');
                     END;",
                managed.id
            ))
            .expect("install current-transfer failure");
        }

        let error = provider_fingerprint::remove_unmanaged_duplicates(&db, &app_type, &managed)
            .expect_err("current transfer failure must roll back adoption")
            .to_string();

        assert!(error.contains("injected current transfer failure"));
        assert!(db
            .get_provider_by_id(&duplicate.id, app_type.as_str())
            .expect("read duplicate")
            .is_some());
        assert_eq!(
            db.get_current_provider(app_type.as_str())
                .expect("read current after rollback")
                .as_deref(),
            Some(duplicate.id.as_str())
        );
    }

    #[test]
    fn provision_merge_never_uses_an_unmanaged_provider_as_the_owner() {
        let db = crate::database::Database::memory().expect("内存库");
        let app_type = AppType::Codex;
        let settings = provision::settings_config_for(
            &app_type,
            "sk-shared",
            "Imported",
            "https://relay.example/v1",
            "model-a",
        )
        .expect("codex 配置");
        let imported = Provider {
            id: "cc-switch-imported".into(),
            name: "Imported".into(),
            settings_config: settings.clone(),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        let non_managed_candidate = Provider {
            id: "manual-provider".into(),
            name: "Manual".into(),
            settings_config: settings,
            ..imported.clone()
        };
        db.save_provider(app_type.as_str(), &imported)
            .expect("写入导入项");
        db.save_provider(app_type.as_str(), &non_managed_candidate)
            .expect("写入手工项");

        assert!(provider_fingerprint::remove_unmanaged_duplicates(
            &db,
            &app_type,
            &non_managed_candidate,
        )
        .expect("不该失败")
        .is_empty());
        assert!(db
            .get_provider_by_id(&imported.id, app_type.as_str())
            .expect("查询")
            .is_some());
    }

    #[test]
    fn provision_summary_reports_adopted_providers_to_the_frontend() {
        let summary = ProvisionSummary {
            tiers: Vec::new(),
            failures: Vec::new(),
            keys_created: 0,
            merged_providers: vec![MergedProviderInfo {
                name: "Imported duplicate".into(),
                app_id: AppType::Codex.as_str().to_string(),
            }],
        };

        let json = serde_json::to_value(summary).expect("应能序列化");
        assert_eq!(json["mergedProviders"][0]["name"], "Imported duplicate");
        assert_eq!(json["mergedProviders"][0]["appId"], "codex");
    }

    #[tokio::test]
    async fn newapi_account_mismatch_stops_before_group_or_token_inventory() {
        use axum::{
            routing::{delete, get, post},
            Json, Router,
        };
        use serde_json::json;

        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let account_requests = Arc::clone(&requests);
        let group_requests = Arc::clone(&requests);
        let token_requests = Arc::clone(&requests);
        let create_requests = Arc::clone(&requests);
        let reveal_requests = Arc::clone(&requests);
        let delete_requests = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/api/user/self",
                get(move || {
                    let requests = Arc::clone(&account_requests);
                    async move {
                        requests.lock().unwrap().push("account".into());
                        Json(json!({
                            "success": true,
                            "data": {
                                "id": 99,
                                "username": "other-account",
                                "display_name": "Other Account",
                                "email": "other@example.test",
                                "group": "default",
                                "quota": 0,
                                "used_quota": 0
                            }
                        }))
                    }
                }),
            )
            .route(
                "/api/user/self/groups",
                get(move || {
                    let requests = Arc::clone(&group_requests);
                    async move {
                        requests.lock().unwrap().push("groups".into());
                        Json(json!({ "success": true, "data": {} }))
                    }
                }),
            )
            .route(
                "/api/token/",
                get(move || {
                    let requests = Arc::clone(&token_requests);
                    async move {
                        requests.lock().unwrap().push("tokens".into());
                        Json(json!({
                            "success": true,
                            "data": {
                                "page": 1,
                                "page_size": 100,
                                "total": 0,
                                "items": []
                            }
                        }))
                    }
                })
                .post(move || {
                    let requests = Arc::clone(&create_requests);
                    async move {
                        requests.lock().unwrap().push("create".into());
                        Json(json!({ "success": true }))
                    }
                }),
            )
            .route(
                "/api/token/{id}/key",
                post(move || {
                    let requests = Arc::clone(&reveal_requests);
                    async move {
                        requests.lock().unwrap().push("reveal".into());
                        Json(json!({ "success": true, "data": { "key": "unexpected" } }))
                    }
                }),
            )
            .route(
                "/api/token/{id}",
                delete(move || {
                    let requests = Arc::clone(&delete_requests);
                    async move {
                        requests.lock().unwrap().push("delete".into());
                        Json(json!({ "success": true }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind account-mismatch server");
        let origin = format!("http://{}", listener.local_addr().expect("server address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        let op = creds::Relay {
            site_origin: origin,
            ..test_newapi_relay(7)
        };

        let error = match provision_backend(&op, None).await {
            Ok(_) => panic!("persisted account mismatch must stop provisioning"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("账号不一致"), "{error}");
        assert_eq!(
                requests.lock().unwrap().as_slice(),
                ["account"],
                "account preflight must be the only remote request; no group/token inventory or mutation may run"
            );
        server.abort();
    }

    fn test_newapi_group(
        identity: &str,
        api_key: &str,
    ) -> crate::relay::newapi_provision::ReconciledGroup {
        crate::relay::newapi_provision::ReconciledGroup {
            identity: crate::relay::newapi::GroupIdentity(identity.into()),
            name: identity.into(),
            rate_multiplier: Some(1.25),
            description: format!("{identity} description"),
            api_key: api_key.into(),
            token_was_created: false,
        }
    }

    fn newapi_models() -> Vec<String> {
        provision::normalize_model_names(vec![
            "gemini-2.5-pro".into(),
            "claude-haiku-4-5".into(),
            "gpt-5.4".into(),
            "claude-sonnet-4-5".into(),
            "gpt-5.4".into(),
        ])
    }

    #[test]
    fn newapi_model_catalog_requires_at_least_one_normalized_model() {
        assert!(normalize_newapi_model_catalog(None).is_none());
        assert!(normalize_newapi_model_catalog(Some(vec!["  ".into(), "\n".into()])).is_none());
        assert_eq!(
            normalize_newapi_model_catalog(Some(vec![
                " gpt-5.4 ".into(),
                "gemini-2.5-pro".into(),
                "gpt-5.4".into(),
            ])),
            Some(vec!["gemini-2.5-pro".into(), "gpt-5.4".into()])
        );
    }

    fn newapi_batch(
        op: &creds::Relay,
        groups: &[crate::relay::newapi_provision::ReconciledGroup],
    ) -> ManagedProvisionBatch {
        let account_id = op.account_id.expect("test relay has account id");
        // 与 provision_backend 同一条纪律：keep 槽位跟着分类走（混合目录 = 三个聊天栏）。
        let mut observed_keep = std::collections::HashSet::new();
        let candidates = groups
            .iter()
            .flat_map(|group| {
                let models = newapi_models();
                newapi_keep_insert(
                    &mut observed_keep,
                    &op.site_origin,
                    account_id,
                    &group.identity,
                    Some(&models),
                );
                newapi_candidates_for_group(
                    &op.site_origin,
                    account_id,
                    group,
                    &models,
                    // 测试钉内置表：选型断言不随本机真实远端缓存漂移。
                    &provision::ModelSelectionTables::builtin(),
                )
            })
            .collect();
        ManagedProvisionBatch {
            account_id: Some(account_id),
            site_declaration: None,
            candidates,
            observed_keep,
            failures: Vec::new(),
            keys_created: 0,
        }
    }

    /// **纯生图分组的 new-api 扇出只出生图候选**（2026-09-05 某 new-api 站点纯生图
    /// 分组的实测形状：`gpt-image-2 + nano-banana-2`）。
    ///
    /// 旧行为把每个分组无条件扇出到 claude/codex/gemini：生图模型被写成聊天模型
    /// （`ANTHROPIC_MODEL=nano-banana-2`、`GEMINI_MODEL=gpt-image-2`），切过去调用必
    /// 404 —— 生图栏则永远零档位（「此账号在当前平台没有可用分组」）。
    #[test]
    fn newapi_pure_image_group_lands_only_in_the_image_column_and_migrates_legacy_tiers() {
        let op = test_newapi_relay(7);
        let group = test_newapi_group("图", "sk-image");
        let image_models =
            provision::normalize_model_names(vec!["nano-banana-2".into(), "gpt-image-2".into()]);
        let account_id = 7;

        let candidates = newapi_candidates_for_group(
            &op.site_origin,
            account_id,
            &group,
            &image_models,
            &provision::ModelSelectionTables::builtin(),
        );
        assert_eq!(candidates.len(), 1, "纯生图分组不该再扇出到聊天栏");
        assert_eq!(candidates[0].app_type, AppType::CodexImage);
        // 默认模型 = gpt-image 家族优先（跨家族并存时表里靠前的家族胜出）。
        assert_eq!(candidates[0].model, "gpt-image-2");

        // 先按旧行为落三栏（等价于升级前 provision 过的存量），再按新分类
        // provision 一次：三个聊天栏的旧投影必须被清掉、生图栏出现新档位。
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        persist_provision_batch(&state, &op, newapi_batch(&op, std::slice::from_ref(&group)))
            .expect("seed legacy three-column projections");
        let provider_id =
            provision::newapi_provider_id_for(&op.site_origin, account_id, &group.identity.0);
        for app_type in newapi_app_types() {
            assert!(db
                .get_provider_by_id(&provider_id, app_type.as_str())
                .expect("read legacy projection")
                .is_some());
        }

        let mut keep = std::collections::HashSet::new();
        newapi_keep_insert(
            &mut keep,
            &op.site_origin,
            account_id,
            &group.identity,
            Some(&image_models),
        );
        let migrated = ManagedProvisionBatch {
            account_id: Some(account_id),
            site_declaration: None,
            candidates,
            observed_keep: keep,
            failures: Vec::new(),
            keys_created: 0,
        };
        let summary =
            persist_provision_batch(&state, &op, migrated).expect("migrate to image column");
        assert_eq!(summary.tiers.len(), 1);
        assert_eq!(summary.tiers[0].app_id, AppType::CodexImage.as_str());
        for app_type in newapi_app_types() {
            assert!(
                db.get_provider_by_id(&provider_id, app_type.as_str())
                    .expect("read pruned projection")
                    .is_none(),
                "{} 的旧投影没被清掉",
                app_type.as_str()
            );
        }
        // 生图栏新档位：codex 形状（生图 MCP 的读取契约）+ 家族优先选出的模型。
        let image_provider = db
            .get_provider_by_id(&provider_id, AppType::CodexImage.as_str())
            .expect("read image projection")
            .expect("image projection exists");
        assert_eq!(
            provision::extract_model(&image_provider.settings_config).as_deref(),
            Some("gpt-image-2")
        );
        assert_eq!(
            provision::extract_api_key(&image_provider.settings_config, &AppType::CodexImage)
                .as_deref(),
            Some("sk-image")
        );
    }

    #[test]
    fn newapi_group_expands_to_three_app_configs_with_one_provider_id() {
        let op = test_newapi_relay(7);
        let group = test_newapi_group(" vip/\u{4e2d}\u{6587} \u{1f680} ", "sk-shared");
        let batch = newapi_batch(&op, std::slice::from_ref(&group));

        assert_eq!(batch.candidates.len(), 3);
        assert_eq!(batch.observed_keep.len(), 3);
        let provider_ids = batch
            .candidates
            .iter()
            .map(|candidate| candidate.provider_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(provider_ids.len(), 1);
        assert_eq!(
            batch
                .candidates
                .iter()
                .map(|candidate| candidate.app_type.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex", "gemini"]
        );

        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        let summary = persist_provision_batch(&state, &op, batch).expect("persist projections");

        assert_eq!(summary.tiers.len(), 3);
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            let provider = db
                .get_provider_by_id(summary.tiers[0].provider_id.as_str(), app_type.as_str())
                .expect("read provider")
                .expect("projection exists");
            assert_eq!(
                provision::extract_api_key(&provider.settings_config, &app_type).as_deref(),
                Some("sk-shared")
            );
            assert_eq!(
                provider.website_url.as_deref(),
                Some(op.site_origin.as_str())
            );
            assert_eq!(
                provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.loongport_account_id),
                Some(7)
            );
        }
    }

    #[test]
    fn newapi_refresh_preserves_edited_config_but_recomputes_unedited_defaults() {
        let op = test_newapi_relay(7);
        let first_group = test_newapi_group("vip", "sk-first");
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());
        let first = persist_provision_batch(&state, &op, newapi_batch(&op, &[first_group]))
            .expect("initial provision");
        let provider_id = first.tiers[0].provider_id.clone();

        let mut edited = db
            .get_provider_by_id(&provider_id, AppType::Codex.as_str())
            .expect("read edited provider")
            .expect("edited provider exists");
        edited.settings_config = provision::settings_config_for(
            &AppType::Codex,
            "sk-first",
            "Custom Name",
            "https://custom.example/v1",
            "gpt-custom",
        )
        .expect("custom codex config");
        let mut expected_edited = edited.settings_config.clone();
        assert!(provision::patch_api_key(
            &mut expected_edited,
            &AppType::Codex,
            "sk-second"
        ));
        db.save_provider(AppType::Codex.as_str(), &edited)
            .expect("save edited provider");
        db.set_user_edited(AppType::Codex.as_str(), &provider_id, true)
            .expect("mark edited");

        let mut unedited = db
            .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
            .expect("read unedited provider")
            .expect("unedited provider exists");
        unedited.settings_config["env"]["GEMINI_MODEL"] =
            serde_json::Value::String("gemini-stale".into());
        db.save_provider(AppType::Gemini.as_str(), &unedited)
            .expect("save stale unedited provider");

        let second_group = test_newapi_group("vip", "sk-second");
        let second_batch = newapi_batch(&op, &[second_group]);
        persist_provision_batch(&state, &op, second_batch).expect("refresh provision");

        let edited_after = db
            .get_provider_by_id(&provider_id, AppType::Codex.as_str())
            .expect("read refreshed edited provider")
            .expect("refreshed edited provider exists");
        assert_eq!(edited_after.settings_config, expected_edited);
        let unedited_after = db
            .get_provider_by_id(&provider_id, AppType::Gemini.as_str())
            .expect("read refreshed default provider")
            .expect("refreshed default provider exists");
        assert_eq!(
            provision::extract_api_key(&unedited_after.settings_config, &AppType::Gemini)
                .as_deref(),
            Some("sk-second")
        );
        assert_eq!(
            unedited_after
                .settings_config
                .pointer("/env/GEMINI_MODEL")
                .and_then(serde_json::Value::as_str),
            Some("gemini-2.5-pro")
        );
    }

    #[test]
    fn newapi_unclassified_keep_retains_failed_group_and_prunes_only_the_current_account() {
        let account_seven = test_newapi_relay(7);
        let account_eight = test_newapi_relay(8);
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        let state = AppState::new(db.clone());

        persist_provision_batch(
            &state,
            &account_seven,
            newapi_batch(
                &account_seven,
                &[
                    test_newapi_group("observed", "sk-seven-observed"),
                    test_newapi_group("removed", "sk-seven-removed"),
                ],
            ),
        )
        .expect("seed account seven");
        persist_provision_batch(
            &state,
            &account_eight,
            newapi_batch(
                &account_eight,
                &[
                    test_newapi_group("observed", "sk-eight-observed"),
                    test_newapi_group("removed", "sk-eight-removed"),
                ],
            ),
        )
        .expect("seed account eight");

        let observed = crate::relay::newapi::GroupIdentity("observed".into());
        let retained_id =
            provision::newapi_provider_id_for(&account_seven.site_origin, 7, &observed.0);
        let removed_id =
            provision::newapi_provider_id_for(&account_seven.site_origin, 7, "removed");
        // 对账没走完（拿到 observed 清单但没拿到 sk）：分类未知，保全四个槽位。
        let mut failure_keep = std::collections::HashSet::new();
        newapi_keep_insert(
            &mut failure_keep,
            &account_seven.site_origin,
            7,
            &observed,
            None,
        );
        let failure_batch = ManagedProvisionBatch {
            account_id: Some(7),
            site_declaration: None,
            candidates: Vec::new(),
            observed_keep: failure_keep,
            failures: vec![FailureInfo {
                group_name: "observed".into(),
                reason: "reveal: temporary failure".into(),
            }],
            keys_created: 0,
        };
        let summary = persist_provision_batch(&state, &account_seven, failure_batch)
            .expect("retained existing providers keep the refresh partial-successful");

        assert!(summary.tiers.is_empty());
        assert_eq!(summary.failures.len(), 1);
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            assert!(db
                .get_provider_by_id(&retained_id, app_type.as_str())
                .expect("read retained provider")
                .is_some());
            assert!(db
                .get_provider_by_id(&removed_id, app_type.as_str())
                .expect("read removed provider")
                .is_none());

            let other_account_id =
                provision::newapi_provider_id_for(&account_eight.site_origin, 8, "removed");
            assert!(db
                .get_provider_by_id(&other_account_id, app_type.as_str())
                .expect("read other account provider")
                .is_some());
        }
    }

    #[test]
    fn newapi_provider_write_failure_keeps_successful_apps_and_reports_the_failure() {
        let op = test_newapi_relay(7);
        let db = Arc::new(crate::database::Database::memory().expect("memory db"));
        {
            let conn = db.conn.lock().expect("lock memory db");
            conn.execute_batch(
                "CREATE TRIGGER fail_newapi_claude_write
                     BEFORE INSERT ON providers
                     WHEN NEW.app_type = 'claude'
                     BEGIN
                       SELECT RAISE(FAIL, 'injected claude write failure');
                     END;",
            )
            .expect("install selective write failure");
        }
        let state = AppState::new(db.clone());

        let summary = persist_provision_batch(
            &state,
            &op,
            newapi_batch(&op, &[test_newapi_group("partial", "sk-partial")]),
        )
        .expect("two successful app projections keep the batch successful");

        assert_eq!(
            summary
                .tiers
                .iter()
                .map(|tier| tier.app_id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "gemini"]
        );
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].group_name, "partial");
        assert!(summary.failures[0].reason.contains("claude"));
        assert!(summary.failures[0]
            .reason
            .contains("injected claude write failure"));
    }

    #[test]
    fn managed_meta_pins_api_format_for_codex_and_leaves_others_empty() {
        // codex：不写 apiFormat 会落到 ProxyChat profile —— 那是唯一会 spawn codex
        // 子进程的分支。
        assert_eq!(
            managed_meta(&AppType::Codex, Some(1), None)
                .api_format
                .as_deref(),
            Some("openai_responses")
        );

        // 其它 CLI：`api_format` **只被 codex_config.rs 消费**，给它们填值不会有人读，
        // 反而让人以为那里有语义。
        for app_type in [AppType::Claude, AppType::Gemini] {
            assert_eq!(
                managed_meta(&app_type, Some(1), None).api_format,
                None,
                "{app_type:?} 不该有 api_format —— 只有 codex 会读它"
            );
        }
    }

    /// ⭐ **A 账号 provision 不能删掉同站 B 账号的档位。**
    ///
    /// 这是本轮实测追出来的一类：归属原本只判 `website_url`（站点），而 `keep` 只装
    /// **这一次** provision（= 一个账号）生成的 id ⇒ A 刷新一次就把 B 的全部档位
    /// 当成「不再存在」删光。同一个缺陷在 `remove_site_impl`（删一个账号）下更彻底：
    /// 它传空 `keep`，等于清掉该站所有账号的档位。
    #[test]
    fn pruning_one_account_leaves_another_accounts_tiers_on_the_same_site() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 7 的两条：一条这次仍在（keep 里），一条已失效。
        let a_kept = provision::provider_id_for(site, Some(7), 1);
        let a_stale = provision::provider_id_for(site, Some(7), 2);
        // 账号 9 的一条：**这次压根没查它**（不同账号、不同分组集合）。
        let b_tier = provision::provider_id_for(site, Some(9), 1);

        for p in [
            seeded_owned(&a_kept, "A·留", Some(site), 7),
            seeded_owned(&a_stale, "A·废", Some(site), 7),
            seeded_owned(&b_tier, "B·别动", Some(site), 9),
        ] {
            db.save_provider("codex", &p).expect("seed");
        }

        let state = AppState::new(db.clone());
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), a_kept.clone())]
                .into_iter()
                .collect();

        // 以账号 7 的身份清理。
        let removed = prune_stale_tiers(&state, site, Some(7), &keep).expect("prune");
        assert_eq!(removed, 1, "只该删账号 7 那条失效的");

        let ids = db.get_provider_ids("codex").expect("list");
        assert!(ids.contains(&a_kept), "账号 7 这次生成的要留着");
        assert!(!ids.contains(&a_stale), "账号 7 失效的那条该删");
        assert!(
            ids.contains(&b_tier),
            "⭐ 账号 9 的档位**必须留着** —— 它不在这次的 keep 里只是因为压根没查它"
        );
    }

    /// 这道闸守 `prune_stale_tiers` 的三个判据。
    ///
    /// 它是**唯一会删用户数据的 relay 代码路径**，判据放宽一点就会误删用户手工配置的
    /// provider（不可挽回）；收紧一点则清不掉脏记录（就是用户撞见的「claude 下还有
    /// codex 分组，点刷新也不消失」）。所以正反两面都要钉住。
    #[test]
    fn prune_only_touches_this_sites_managed_tiers() {
        let site = "https://bestapi.store";
        let other_site = "https://other.dev";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 这次 provision 生成的（该留）。
        let kept_id = provision::provider_id_for(site, Some(1), 1);
        // 同一个站的托管项，但这次没生成（该删 —— 分组已被中转站删掉 / 旧版本写错的）。
        let stale_id = provision::provider_id_for(site, Some(1), 2);
        // **别的站**的托管项：这次压根没查它的分组，凭「这次没生成」删它是错的。
        let other_site_id = provision::provider_id_for(other_site, Some(1), 3);

        for (app, p) in [
            ("codex", seeded(&kept_id, "留下", Some(site))),
            ("codex", seeded(&stale_id, "该删", Some(site))),
            ("codex", seeded(&other_site_id, "别的站", Some(other_site))),
            // 用户手工加的：id 不是我们生成的形状 ⇒ 一律不碰，哪怕 website_url 是同一个站。
            ("codex", seeded("my-own-provider", "用户自己的", Some(site))),
            // 托管项但没有 website_url（历史数据）⇒ 归属不明，不删（宁可漏删不可错删）。
            (
                "codex",
                seeded(
                    &provision::provider_id_for(site, Some(1), 9),
                    "无归属",
                    None,
                ),
            ),
            // **另一个 app_type 下的脏记录** —— 正是用户撞见的那种（openai 分组被
            // 旧代码写进了 claude 下）。必须也被清掉，所以不能只扫参数指定的那个 app。
            ("claude", seeded(&stale_id, "串台到 claude", Some(site))),
        ] {
            db.save_provider(app, &p).expect("seed");
        }

        let state = AppState::new(db.clone());
        // 这次只在 codex 下生成了 kept_id。
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), kept_id.clone())]
                .into_iter()
                .collect();

        let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");
        assert_eq!(removed, 2, "该删的是 codex 与 claude 下那两条 stale");

        let codex_ids = db.get_provider_ids("codex").expect("list codex");
        assert!(codex_ids.contains(&kept_id), "这次生成的必须留着");
        assert!(!codex_ids.contains(&stale_id), "同站的过期档位必须删掉");
        assert!(
            codex_ids.contains(&other_site_id),
            "别的站的档位不能删 —— 这次没查它的分组"
        );
        assert!(
            codex_ids.contains("my-own-provider"),
            "用户手工配的 provider 绝不能删"
        );
        assert!(
            codex_ids.contains(&provision::provider_id_for(site, Some(1), 9)),
            "没有 website_url 的托管项归属不明，不该删"
        );

        let claude_ids = db.get_provider_ids("claude").expect("list claude");
        assert!(
            !claude_ids.contains(&stale_id),
            "串到别的 app_type 下的脏记录也要清 —— 只扫一个 app 就漏了它"
        );
    }

    /// ⭐ 用户实测那个 bug 的**精确复现**：同一个 id 在一个 app 下合法、在另一个下是脏的。
    ///
    /// ## 为什么上面那条测试放过了它
    ///
    /// 那条构造的串台记录在**两个 app 下都该删**（`keep` 里压根没有它）。
    /// 而真实情形是：`pro池` 这个分组的 platform 是 openai ⇒ 它在 **codex 下合法**，
    /// 但旧版本的 bug 把它也写进了 **claude** ⇒ claude 下那条是脏的。
    ///
    /// 而 `provider_id = sha256(site_origin + group_id)`，**不含 app_type** ⇒
    /// 两条记录的 id **完全相同**（实测 `loongport-8c669ca0b007e7ea`）。
    /// 于是「keep 只放 id」时：那个 id 因为 codex 下合法而进了 keep，
    /// claude 下那条脏记录就被当成「该保留」⇒ **点多少次刷新都不消失**。
    ///
    /// 这正是用户反复报的那个现象。判据必须是 **(app_type, id) 组合**。
    #[test]
    fn a_group_valid_in_one_app_does_not_protect_its_twin_in_another_app() {
        let site = "https://790053500.com";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 同一个分组（group_id = 1）⇒ 两个 app 下**同一个 id**。
        let shared_id = provision::provider_id_for(site, Some(1), 1);
        db.save_provider("codex", &seeded(&shared_id, "pro池", Some(site)))
            .expect("seed codex");
        db.save_provider("claude", &seeded(&shared_id, "pro池", Some(site)))
            .expect("seed claude");

        let state = AppState::new(db.clone());
        // 这次 provision 只把它落到 codex（因为它的 platform 是 openai）。
        let keep: std::collections::HashSet<(String, String)> =
            [("codex".to_string(), shared_id.clone())]
                .into_iter()
                .collect();

        let removed = prune_stale_tiers(&state, site, Some(1), &keep).expect("prune");

        assert_eq!(removed, 1, "claude 下那条脏记录必须被删掉");
        assert!(
            db.get_provider_ids("codex")
                .expect("codex")
                .contains(&shared_id),
            "codex 下那条是这次生成的，必须留着"
        );
        assert!(
            !db.get_provider_ids("claude")
                .expect("claude")
                .contains(&shared_id),
            "claude 下那条必须被删 —— 它与 codex 下那条 id 相同，\
                 但『在 codex 下合法』不该保护它"
        );
    }

    /// 当前项也删。
    ///
    /// `ProviderService::delete` 拒绝删当前项（防用户误删正在用的配置），但走到 prune
    /// 这一步说明**服务端已经没有这个分组了**，它的 sk 是死的 —— 留着当「当前项」只会
    /// 让 CLI 拿失效密钥去发请求。用户重新选一个可用的即可。
    #[test]
    fn prune_deletes_the_current_tier_too() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let stale_id = provision::provider_id_for(site, Some(1), 7);

        db.save_provider("codex", &seeded(&stale_id, "过期的当前项", Some(site)))
            .expect("seed");
        db.set_current_provider("codex", &stale_id)
            .expect("set current");

        let state = AppState::new(db.clone());
        let removed = prune_stale_tiers(&state, site, Some(1), &std::collections::HashSet::new())
            .expect("prune");

        assert_eq!(removed, 1, "当前项也该被删掉");
        assert!(
            db.get_provider_by_id(&stale_id, "codex")
                .expect("query")
                .is_none(),
            "过期的当前项必须真的从库里消失"
        );
    }

    /// ⭐ **命令层必须真的调 `refresh_live_for_current_tiers`** —— 两处都不能漏。
    ///
    /// ## 为什么这条测试读源码而不是调函数
    ///
    /// provision 入口仍吃 `&tauri::AppHandle`；reset 的数据库与协调器路径已经下沉到
    /// `reset_tier_config_in_state` 并由真实行为测试覆盖，但“当前项刷新 live 文件”会触碰
    /// 用户配置，单元测试不能安全执行。第二路 review 实测证明了这条接线盲区的代价：
    /// 把那两处调用注释掉，2578 条测试**全绿**——
    /// 那条集成测试（`loongport_codex_live.rs`）自己调服务层，所以它测的是服务层，
    /// 不是「命令层有没有调服务层」。
    ///
    /// 源码断言是这里唯一能把那一步钉住的手段（与仓里 `vendorSwitchGuardContract`
    /// 那条同一个理由与形态）。它守的不是实现细节，而是**这条链路还接着吗** ——
    /// 断了的症状是静默的：界面提示刷新成功，而 CLI 一直用旧密钥。
    #[test]
    fn refresh_live_for_current_tiers_is_wired_into_both_commands() {
        // 两条路各在各的领域模块里（provision 管线 / 行维护），各扫各的文件。
        let src = include_str!("provision.rs");

        // 取 `refresh_relay_provision` 到 `prune_stale_tiers` 调用之间那段（provision 那条路）。
        let provision = {
            let start = src
                .find("async fn refresh_relay_provision")
                .expect("refresh_relay_provision 还在吗");
            let end = src[start..]
                .find("let removed = prune_stale_tiers")
                .expect("provision 末尾那段清理还在吗");
            &src[start..start + end]
        };
        assert!(
            provision.contains("refresh_live_for_current_tiers(state, &refresh_live)"),
            "⭐ provision 链路不再刷新当前档位的 live config —— \
                 sk 被撤销重建后，CLI 会一直用旧密钥，而用户点不动那个档位（UI 认为它已是当前项）"
        );

        // 取真正执行重置的 state helper 那段（在 rows.rs）。
        let rows_src = include_str!("rows.rs");
        let reset = {
            let start = rows_src
                .find("fn reset_tier_config_in_state")
                .expect("reset_tier_config_in_state 还在吗");
            let end = rows_src[start..]
                .find("\n/// 保存中转站行的手工顺序")
                .expect("reset 之后那个命令还在吗");
            &rows_src[start..start + end]
        };
        assert!(
            reset.contains("refresh_live_for_current_tiers("),
            "⭐ `reset_tier_config_impl` 不再刷新 live config —— \
                 那会让「恢复默认配置」这个按钮对当前项**整体无效**（改坏的配置就在 live 文件里）"
        );
    }

    /// 闸的归属判据必须与 `prune_stale_tiers` 是**同一份** —— 否则守卫与删除各认一套：
    /// 守卫说「这条不是你的、不拦」，删除说「这条是你的、删了」⇒ 恰好绕过守卫。
    ///
    /// 这条钉的是「别人的当前项不该拦住我」这一半（宽松方向的误判）。
    #[test]
    fn the_guard_ignores_another_accounts_current_tier_on_the_same_site() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位是 codex 的当前项。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        db.save_provider("codex", &seeded_owned(&b_tier, "B 的档位", Some(site), 9))
            .expect("seed");
        db.set_current_provider("codex", &b_tier)
            .expect("set current");

        let state = AppState::new(db.clone());

        // 以账号 7 的身份问「我名下有在用的吗」—— 答案必须是「没有」。
        assert!(
            apps_using_this_accounts_tiers(&state, site, Some(7)).is_empty(),
            "同站另一个账号的当前项不该拦住我删自己的账号"
        );
        // 而账号 9 自己问，必须撞上。
        assert_eq!(
            apps_using_this_accounts_tiers(&state, site, Some(9)).len(),
            1,
            "账号 9 名下那条正是当前项，必须被认出来"
        );
    }

    /// ⭐ **还没登录的 relay 行，不能把同站别人账号的档位记到自己头上。**
    ///
    /// Task 3 review 抓出的：把 `relay_balance_inputs` 的内联判据收敛到
    /// `belongs_to_account` 时，`(relay.account_id, 档位账号)` 的 `(None, Some)` 那格
    /// 从「不认」翻成了「认」—— 未登录行会收走别人档位的 sk、对账会把别人的成本
    /// 算进这一行。现在余额 / 对账走严格版 [`belongs_to_relay`]（还原原内联语义
    /// `(None, Some(_)) => false`），清理 / 守卫路径仍走宽松版 [`belongs_to_account`]。
    ///
    /// 会红的改法：`relay_balance_inputs` 改回 `belongs_to_account`。
    #[test]
    fn an_unlogged_relay_row_is_not_attributed_another_accounts_tier() {
        let site = "https://bestapi.store";
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));

        // 账号 9 的档位，带真实 sk（否则「没收走」的断言没有判别力）。
        let b_tier = provision::provider_id_for(site, Some(9), 1);
        let mut b_provider = seeded_owned(&b_tier, "B 的档位", Some(site), 9);
        b_provider.settings_config = serde_json::json!({ "auth": { "OPENAI_API_KEY": "sk-b" } });
        db.save_provider("codex", &b_provider).expect("seed B");

        // 同站一条没记账号的旧档位（升级前生成），也没有 sk —— 只用于钉住
        // `(None, None) => true` 这格没被顺手改掉。
        let legacy = provision::provider_id_for(site, None, 5);
        db.save_provider("codex", &seeded(&legacy, "旧数据", Some(site)))
            .expect("seed legacy");

        let state = AppState::new(db.clone());
        let mut unlogged = purchase_capability_relay(creds::BackendKind::Sub2Api);
        unlogged.site_origin = site.to_string();
        unlogged.account_id = None;

        let (_, keys) = relay_balance_inputs(&state, &unlogged);
        assert!(
            keys.is_empty(),
            "⭐ 未登录的行认不出归属 ⇒ 同站别人账号的档位（哪怕有 sk）不该被收走：{keys:?}"
        );

        // 对照组：账号 9 自己的行必须能拿到那把 sk —— 证明上面不是「本来就收不到」。
        let mut owner_row = unlogged.clone();
        owner_row.account_id = Some(9);
        let (_, keys) = relay_balance_inputs(&state, &owner_row);
        assert_eq!(
            keys,
            vec!["sk-b".to_string()],
            "档位自己的账号必须收得到 sk"
        );

        // 两个判据函数在关键那格的分歧是**有意的**，钉住防止将来被「顺手统一」：
        // 删除方向（belongs_to_account）对 None 宽松（旧数据要能清），
        // 归属方向（belongs_to_relay）对 None 严格（别人的不能认领）。
        let b_in_db = db
            .get_provider_by_id(&b_tier, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_account(&b_in_db, site, None),
            "删除方向对 `None` 仍宽松 —— 别改"
        );
        assert!(
            !belongs_to_relay(&b_in_db, site, None),
            "归属方向对 `(None, Some)` 必须严格 —— 别人的档位不能记到未登录行头上"
        );
        let legacy_in_db = db
            .get_provider_by_id(&legacy, "codex")
            .expect("query")
            .expect("在");
        assert!(
            belongs_to_relay(&legacy_in_db, site, None),
            "同站没记归属的旧档位仍是「可能是我的」（(None, None) => true）"
        );
    }

    /// ⭐ **倍率必须活过 provision → 库 → `listRelays` 这一整条**。
    ///
    /// 它是这次改动的核心：倍率从「每次渲染现拉」改成「provision 写一次、之后只读本地」。
    /// 链路上任何一环断掉，症状都是**界面永远显示「倍率未知」**，而没有报错 ——
    /// 只有这条端到端的断言守得住。
    ///
    /// 会红的改法：`persist_provision_batch` 里不写 `set_tier_rate_multiplier`，
    /// 或 `list_tiers_impl` 把 `rate_multiplier` 改回写死 `None`。
    #[test]
    fn a_provisioned_rate_survives_into_list_relays() {
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
                "tok",
                None,
                Some(i64::MAX),
                creds::SessionEnvironment::default(),
            )
        })
        .expect("credentials");
        let op = with_conn(&state, |conn| creds::get(conn, row_id))
            .expect("load")
            .expect("exists");

        let provider_id = provision::provider_id_for(site, Some(7), 1);
        let batch = ManagedProvisionBatch {
            account_id: Some(7),
            site_declaration: None,
            candidates: vec![ManagedProvisionCandidate {
                provider_id: provider_id.clone(),
                app_type: AppType::Codex,
                group_id: "1".into(),
                group_name: "Pro池".into(),
                rate_multiplier: Some(0.15),
                api_key: "sk-test".into(),
                model: "gpt-5.6-sol".into(),
                models: None,
                roles: None,
                allow_image_generation: Some(false),
                api_base_url: site.into(),
            }],
            observed_keep: Default::default(),
            failures: Vec::new(),
            keys_created: 0,
        };
        persist_provision_batch(&state, &op, batch).expect("persist");

        let rows = list_relays_impl(&state, AppType::Codex).expect("list relays");
        let tier = rows
            .iter()
            .find(|r| r.id == row_id)
            .expect("行在")
            .tiers
            .first()
            .expect("档位在");
        assert_eq!(
            tier.rate_multiplier,
            Some(0.15),
            "⭐ 倍率必须从本地库读回来 —— 它不再靠任何网络请求补齐"
        );
    }

    fn pricing_timestamp_state(initial: Option<i64>) -> (AppState, i64) {
        let db = Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);
        let relay_id = with_conn(&state, |conn| {
            creds::save_site_with_backend(
                conn,
                "https://pricing.example",
                "Pricing",
                "https://pricing.example/v1",
                discovery::BackendKind::Sub2Api,
            )
        })
        .unwrap();
        if let Some(initial) = initial {
            with_conn(&state, |conn| {
                creds::mark_pricing_synced(conn, relay_id, initial)
            })
            .unwrap();
        }
        (state, relay_id)
    }

    #[test]
    fn successful_full_refresh_marks_pricing_fresh() {
        let (state, relay_id) = pricing_timestamp_state(None);

        mark_pricing_after_success(&state, relay_id, 456, Ok(())).unwrap();

        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
            .unwrap()
            .unwrap();
        assert_eq!(relay.pricing_synced_at, Some(456));
    }

    #[test]
    fn failed_full_refresh_keeps_the_previous_pricing_time() {
        let (state, relay_id) = pricing_timestamp_state(Some(123));

        let result: Result<(), AppError> = mark_pricing_after_success(
            &state,
            relay_id,
            456,
            Err(AppError::Message("expected failure".into())),
        );

        assert!(result.is_err());
        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))
            .unwrap()
            .unwrap();
        assert_eq!(relay.pricing_synced_at, Some(123));
    }
}
