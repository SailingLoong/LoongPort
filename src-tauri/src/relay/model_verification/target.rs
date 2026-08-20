use std::str::FromStr;

use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    relay::{api, creds, provision},
};

use super::types::{ModelFitness, TargetKey, TargetScope, VerificationModelOption};

/// A verification target whose managed provider, relay account, and API key have all been
/// resolved. It intentionally does not implement `Debug` so credentials cannot be formatted
/// into logs or errors by accident.
pub struct ResolvedTarget {
    target: TargetKey,
    scope: ResolvedScope,
}

impl ResolvedTarget {
    pub(super) fn resolve(db: &Database, target: TargetKey) -> Result<Self, AppError> {
        let scope = ResolvedScope::resolve(
            db,
            TargetScope::new(target.provider_id.clone(), target.app_type.clone()),
        )?;
        Ok(Self { target, scope })
    }

    pub(super) fn api_root(&self) -> &str {
        &self.scope.api_root
    }

    pub(super) fn protocol_base(&self) -> &str {
        &self.scope.protocol_base
    }

    pub(super) fn api_key(&self) -> &str {
        &self.scope.api_key
    }

    #[allow(dead_code)]
    pub(super) fn target(&self) -> &TargetKey {
        &self.target
    }
}

const MODEL_LIST_UNAVAILABLE: &str = "无法读取这个分组的模型列表，请重试。";

/// Lists the exact models advertised for one managed provider scope, each annotated
/// with its protocol pre-screening fitness (see [`ModelFitness`]).
///
/// The shared relay client includes an upstream response preview in some errors for its existing
/// callers. Model verification deliberately collapses every failure to one finite message so a
/// server response or API key can never cross this boundary.
pub(crate) async fn list_models(
    db: &Database,
    provider_id: &str,
    app_type: &str,
) -> Result<Vec<VerificationModelOption>, AppError> {
    let scope = ResolvedScope::resolve(db, TargetScope::new(provider_id, app_type))
        .map_err(|_| unavailable_models_error())?;
    // 模型列表与站点能力图**并行**取：能力图是公开 well-known，不打 sk、
    // 不阻塞在另一个请求后面；它失败也只意味着「无预筛」，模型列表照常。
    let (models, capabilities) = tokio::join!(
        api::list_models(&scope.api_root, &scope.api_key),
        crate::relay::transit::model_protocol_capabilities(&scope.site_origin),
    );
    let mut models = models
        .map_err(|_| unavailable_models_error())?
        .filter(|models| !models.is_empty())
        .ok_or_else(unavailable_models_error)?;
    models.sort_unstable();
    models.dedup();

    let app_type = AppType::from_str(app_type).map_err(|_| unavailable_models_error())?;
    let group_name = scope.group.as_ref().map(|group| group.name.as_str());
    Ok(models
        .into_iter()
        .map(|name| VerificationModelOption {
            fitness: fitness_for(&app_type, capabilities.as_ref(), group_name, &name),
            name,
        })
        .collect())
}

/// 预筛判据的纯函数核——测试直接喂能力图 fixture，不打网络。
///
/// 验证协议对应的声明值认**两代命名**（2026-08-21 活探针实证）：
/// codex 走 Responses ← `openai_responses`（新版）或 `responses`（旧版站）；
/// claude 走 Messages ← `anthropic_messages`。旧版 anthropic 站没有
/// messages 键（那代站点不发布 anthropic 分组），不猜 `messages` 这种
/// 没见过的值。
fn fitness_for(
    app_type: &AppType,
    capabilities: Option<&crate::relay::transit::ModelProtocolCapabilities>,
    group: Option<&str>,
    model: &str,
) -> ModelFitness {
    let (capabilities, group) = match (capabilities, group) {
        (Some(capabilities), Some(group)) => (capabilities, group),
        // 站点没公开数据 / 档位没记分组身份（旧数据）——无从判定。
        _ => return ModelFitness::Unknown,
    };
    let Some(protocols) = capabilities.protocols_for(group, model) else {
        // 分组或模型不在快照清单里 = 没有正向覆盖，不是「不支持」。
        return ModelFitness::Unknown;
    };
    let supported = match app_type {
        AppType::Codex => protocols.contains("openai_responses") || protocols.contains("responses"),
        AppType::Claude => protocols.contains("anthropic_messages"),
        _ => return ModelFitness::Unknown,
    };
    if supported {
        ModelFitness::Supported
    } else {
        ModelFitness::UnsupportedProtocol
    }
}

#[cfg(test)]
mod fitness_tests {
    use super::*;
    use crate::relay::transit::ModelProtocolCapabilities;
    use std::collections::BTreeMap;

    fn capabilities() -> ModelProtocolCapabilities {
        // 两代协议值并存（实证快照的投影）：新版 openai_responses /
        // anthropic_messages，旧版 responses。
        crate::relay::transit::tests::capabilities_from_snapshot_json(
            r#"{
              "groups": [
                {"name": "g-new", "models": [
                  {"raw_model": "gpt-x", "standard_model": "gpt-x-std",
                   "supported_protocols": ["openai_responses"]},
                  {"raw_model": "claude-y", "standard_model": "claude-y-std",
                   "supported_protocols": ["anthropic_messages"]}
                ]},
                {"name": "g-legacy", "models": [
                  {"raw_model": "old-gpt", "supported_protocols": ["responses"]}
                ]}
              ]
            }"#,
        )
    }

    fn fitness(app: &str, group: Option<&str>, model: &str) -> ModelFitness {
        let caps = capabilities();
        fitness_for(&AppType::from_str(app).unwrap(), Some(&caps), group, model)
    }

    #[test]
    fn supported_matches_both_protocol_generations() {
        // 新版值。
        assert_eq!(
            fitness("codex", Some("g-new"), "gpt-x"),
            ModelFitness::Supported
        );
        // 旧版值（老站只有 responses 键）。
        assert_eq!(
            fitness("codex", Some("g-legacy"), "old-gpt"),
            ModelFitness::Supported
        );
        // claude 的 Messages。
        assert_eq!(
            fitness("claude", Some("g-new"), "claude-y"),
            ModelFitness::Supported
        );
        // standard_model 名字也能命中（/v1/models 两种形态都可能是它）。
        assert_eq!(
            fitness("codex", Some("g-new"), "gpt-x-std"),
            ModelFitness::Supported
        );
    }

    #[test]
    fn positive_coverage_excludes_cross_protocol_models() {
        // 快照正向覆盖了（分组在、模型在），但声明的是另一族协议 —— 排除。
        assert_eq!(
            fitness("claude", Some("g-new"), "gpt-x"),
            ModelFitness::UnsupportedProtocol
        );
        // claude 撞旧版 openai 分组：旧站没有 messages 形态的键，也不猜
        // 没见过的值 —— 正向覆盖且不含 anthropic_messages ⇒ 排除。
        assert_eq!(
            fitness("claude", Some("g-legacy"), "old-gpt"),
            ModelFitness::UnsupportedProtocol
        );
    }

    #[test]
    fn missing_coverage_is_unknown_never_excluded() {
        // 分组不在快照（站点只发布部分分组是常态）。
        assert_eq!(
            fitness("codex", Some("g-unpublished"), "gpt-x"),
            ModelFitness::Unknown
        );
        // 模型不在该分组清单。
        assert_eq!(
            fitness("codex", Some("g-new"), "unlisted-model"),
            ModelFitness::Unknown
        );
        // 档位没记分组身份（旧数据）。
        assert_eq!(fitness("codex", None, "gpt-x"), ModelFitness::Unknown);
        // 站点没有公开数据。
        let none = fitness_for(
            &AppType::from_str("codex").unwrap(),
            None,
            Some("g-new"),
            "gpt-x",
        );
        assert_eq!(none, ModelFitness::Unknown);
    }
}

fn unavailable_models_error() -> AppError {
    AppError::Config(MODEL_LIST_UNAVAILABLE.into())
}

struct ResolvedScope {
    api_root: String,
    protocol_base: String,
    api_key: String,
    /// 站点 origin——协议预筛按它取站点的 ai-transit 能力图。
    site_origin: String,
    /// 档位记的分组身份（provision 持久化；旧数据 `None` ⇒ 预筛全 Unknown）。
    group: Option<crate::provider::LoongportGroupIdentity>,
}

impl ResolvedScope {
    fn resolve(db: &Database, scope: TargetScope) -> Result<Self, AppError> {
        let app_type = supported_app_type(&scope.app_type)?;
        let provider = db
            .get_provider_by_id(&scope.provider_id, app_type.as_str())?
            .ok_or_else(|| AppError::Config("这个档位不存在。".into()))?;

        if !crate::relay::is_managed(&provider.id) {
            return Err(AppError::Config(
                "只有 LoongPort 托管的档位才能验证模型。".into(),
            ));
        }

        let site_origin = provider.website_url.as_deref().ok_or_else(|| {
            AppError::Config(
                "这个档位没有记录它属于哪个中转站，请用「获取密钥」重新生成它。".into(),
            )
        })?;
        let account_id = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.loongport_account_id);
        let relay = resolve_relay(db, site_origin, account_id)?;
        let api_key =
            provision::extract_api_key(&provider.settings_config, &app_type).ok_or_else(|| {
                AppError::Config(
                    "这个档位的配置里读不出密钥了，请用「获取密钥」重新生成它。".into(),
                )
            })?;

        Ok(Self {
            api_root: api::site_api_root(&relay.site_origin, &relay.api_base_url),
            protocol_base: api::base_url_for(&app_type, &relay.site_origin, &relay.api_base_url),
            api_key,
            site_origin: relay.site_origin.clone(),
            group: provider
                .meta
                .as_ref()
                .and_then(|meta| meta.loongport_group.clone()),
        })
    }
}

pub(crate) fn supports_app_type(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Codex | AppType::Claude)
}

fn supported_app_type(app_type: &str) -> Result<AppType, AppError> {
    let app_type = AppType::from_str(app_type)?;
    if supports_app_type(&app_type) {
        Ok(app_type)
    } else {
        Err(AppError::Config("模型验证仅支持 codex 和 claude。".into()))
    }
}

fn resolve_relay(
    db: &Database,
    site_origin: &str,
    account_id: Option<i64>,
) -> Result<creds::Relay, AppError> {
    let candidates: Vec<_> = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Database(format!("读取中转站失败: {error}")))?;
        creds::list(&conn)?
            .into_iter()
            .filter(|candidate| candidate.site_origin == site_origin)
            .collect()
    };

    match account_id {
        Some(want) => candidates
            .into_iter()
            .find(|candidate| candidate.account_id == Some(want))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "这个档位属于 {site_origin} 上的某个账号，但那个账号已经不在列表里了。重新登录它、或者直接删掉这个档位。"
                ))
            }),
        None if candidates.len() == 1 => Ok(candidates.into_iter().next().expect("checked length")),
        None if candidates.is_empty() => Err(AppError::Config(format!(
            "这个档位属于 {site_origin}，但那个中转站已经不在列表里了。重新添加它、或者直接删掉这个档位。"
        ))),
        None => Err(AppError::Config(format!(
            "这个档位没有记录属于 {site_origin} 上的哪个账号，而那个站现在挂着多个账号。请用「获取密钥」重新生成它 —— 那会带上账号归属。"
        ))),
    }
}
