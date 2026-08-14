use std::str::FromStr;

use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    relay::{api, creds, provision},
};

use super::types::{TargetKey, TargetScope};

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

/// Lists the exact models advertised for one managed provider scope.
///
/// The shared relay client includes an upstream response preview in some errors for its existing
/// callers. Model verification deliberately collapses every failure to one finite message so a
/// server response or API key can never cross this boundary.
pub(crate) async fn list_models(
    db: &Database,
    provider_id: &str,
    app_type: &str,
) -> Result<Vec<String>, AppError> {
    let scope = ResolvedScope::resolve(db, TargetScope::new(provider_id, app_type))
        .map_err(|_| unavailable_models_error())?;
    let mut models = api::list_models(&scope.api_root, &scope.api_key)
        .await
        .map_err(|_| unavailable_models_error())?
        .filter(|models| !models.is_empty())
        .ok_or_else(unavailable_models_error)?;
    models.sort_unstable();
    models.dedup();
    Ok(models)
}

fn unavailable_models_error() -> AppError {
    AppError::Config(MODEL_LIST_UNAVAILABLE.into())
}

struct ResolvedScope {
    api_root: String,
    protocol_base: String,
    api_key: String,
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
