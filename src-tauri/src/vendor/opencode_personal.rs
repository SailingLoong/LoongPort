//! User-entered Console personal keys. No browser cookies, remote key creation or OAuth copies.
//! Public contract: https://opencode.ai/v2/docs/console/harnesses and /console/api/inference.
use super::PlanInfo;
use crate::{app_config::AppType, error::AppError, relay::provider_config::ProvisionStyle};

pub const PREFIX: &str = "personal-key:";
pub const ZEN: &str = "opencode-personal";
pub const GO: &str = "opencode-personal-go";
pub const PLANS: [PlanInfo; 2] = [
    PlanInfo {
        id_segment: ZEN,
        display_name: "OpenCode Console · Personal key",
    },
    PlanInfo {
        id_segment: GO,
        display_name: "OpenCode Go · Personal key",
    },
];
pub const ALL_PLANS: [PlanInfo; 5] = [
    super::opencode::Plan::PLAN_INFOS[0],
    super::opencode::Plan::PLAN_INFOS[1],
    super::opencode::Plan::PLAN_INFOS[2],
    PLANS[0],
    PLANS[1],
];
pub fn is_plan(segment: &str) -> bool {
    segment == ZEN || segment == GO
}
pub fn is_account(account: &str) -> bool {
    account.starts_with(PREFIX)
}

/// Opaque local credential identity, NOT an inferred remote user/workspace identity.
/// Same key imports are idempotent; different keys are never merged by display name.
pub fn identity(key: &str) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    let suffix = key.strip_prefix("oc_sk_").ok_or_else(|| {
        AppError::Config(
            "请输入完整的 OpenCode Personal API key（oc_sk_…），不是掩码或网页登录态".into(),
        )
    })?;
    let valid = suffix.split_once('_').is_some_and(|(prefix, secret)| {
        !prefix.is_empty()
            && secret.len() >= 16
            && suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    });
    if !valid {
        return Err(AppError::Config(
            "请输入完整密钥；官网列表中的前缀或掩码不能用于调用".into(),
        ));
    }
    Ok(format!(
        "{PREFIX}{}",
        hex::encode(Sha256::digest(key.as_bytes()))
    ))
}

pub fn config_for(segment: &str, app: &AppType) -> Option<(String, String)> {
    let go = segment == GO;
    if !is_plan(segment) {
        return None;
    }
    let base = if go {
        "https://opencode.ai/inference/go"
    } else {
        "https://opencode.ai/inference"
    };
    let (family, model) = match app {
        AppType::Claude | AppType::ClaudeDesktop => (
            "anthropic",
            if go {
                "deepseek-v4-pro"
            } else {
                "claude-sonnet-5"
            },
        ),
        AppType::Codex => ("openai/v1", if go { "glm-5.2" } else { "gpt-5.3-codex" }),
        AppType::Hermes | AppType::OpenClaw | AppType::OpenCode => ("openai/v1", "glm-5.2"),
        _ => return None,
    };
    Some((format!("{base}/{family}"), model.into()))
}
pub fn style(segment: &str) -> ProvisionStyle {
    ProvisionStyle {
        codex_wire_chat: segment == GO,
        ..Default::default()
    }
}
pub fn roles(segment: &str) -> crate::relay::model_selection::ClaudeRoleModels {
    let model = if segment == GO {
        "deepseek-v4-pro"
    } else {
        "claude-sonnet-5"
    }
    .to_string();
    crate::relay::model_selection::ClaudeRoleModels {
        opus: model.clone(),
        fable: model.clone(),
        sonnet: model.clone(),
        haiku: model.clone(),
        subagent: model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_identity_rejects_masks_and_never_claims_remote_workspace() {
        for key in [
            "",
            "sk-old",
            "oc_sk_prefix",
            "oc_sk_prefix_***",
            "oc_sk_prefix_123456789012345…",
        ] {
            assert!(identity(key).is_err());
        }
        let key = "oc_sk_fixture_SyntheticSecretOnly1234";
        let id = identity(key).unwrap();
        assert!(id.starts_with(PREFIX));
        assert!(!id.contains(key));
        assert_eq!(id, identity(key).unwrap());
        assert_ne!(
            id,
            identity("oc_sk_fixture_OtherSyntheticSecret1234").unwrap()
        );
    }
    #[test]
    fn personal_endpoints_are_fixed_and_legacy_templates_are_unchanged() {
        assert_eq!(
            config_for(ZEN, &AppType::Codex).unwrap().0,
            "https://opencode.ai/inference/openai/v1"
        );
        assert_eq!(
            config_for(GO, &AppType::Claude).unwrap().0,
            "https://opencode.ai/inference/go/anthropic"
        );
        assert!(config_for(GO, &AppType::Gemini).is_none());
        assert!(!style(GO).claude_auth_via_api_key);
        assert!(style(GO).codex_wire_chat);
        assert_eq!(
            super::super::opencode::API_ORIGIN,
            "https://opencode.ai/zen/v1"
        );
    }
}

/// Atomic local persistence. It neither verifies nor spends the credential.
pub fn save(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::VaultContext,
    key: &str,
    label: &str,
) -> Result<i64, AppError> {
    use rusqlite::{params, OptionalExtension};
    let key = key.trim();
    let identity = identity(key)?;
    let label = label.trim();
    if label.contains(key) {
        return Err(AppError::Config("显示名不能包含密钥".into()));
    }
    if label.chars().count() > 100 {
        return Err(AppError::Config("显示名不能超过 100 字".into()));
    }
    let label = if label.is_empty() {
        "OpenCode · Personal key"
    } else {
        label
    };
    // Never silently move an existing web-login credential into a different billing path.
    for row in super::creds::list(conn, vault)? {
        if row.vendor_id == "opencode"
            && row.api_key == key
            && row.account_id.as_deref() != Some(identity.as_str())
        {
            return Err(AppError::Config(
                "这把密钥已保存在另一条 OpenCode 账号记录中，请先处理原记录；不会自动迁移".into(),
            ));
        }
    }
    let tx = conn.unchecked_transaction()?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM loongport_vendor WHERE vendor_id='opencode' AND account_id=?1",
            params![identity],
            |r| r.get(0),
        )
        .optional()?;
    let id = match existing {
        Some(id) => id,
        None => {
            tx.execute(
                "INSERT INTO loongport_vendor(vendor_id,account_id) VALUES('opencode',?1)",
                params![identity],
            )?;
            tx.last_insert_rowid()
        }
    };
    let encrypted = crate::secrets::inventory::seal_db(
        vault,
        "loongport_vendor",
        "api_key",
        &[&id.to_string()],
        key,
    )?;
    tx.execute(
        "UPDATE loongport_vendor SET api_key=?1, account_label=?2 WHERE id=?3",
        params![encrypted, label, id],
    )?;
    tx.commit()?;
    Ok(id)
}

/// Only the new explicit personal-key source uses these headers. Preserve original body/SSE.
pub fn request_headers(
    plan: &str,
    url: &str,
    headers: &mut http::HeaderMap,
    client_session: Option<&str>,
) -> Result<(), crate::proxy::ProxyError> {
    let parsed = url::Url::parse(url).map_err(|_| {
        crate::proxy::ProxyError::Internal("OpenCode official endpoint is invalid".into())
    })?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("opencode.ai")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !parsed.path().starts_with("/inference/")
    {
        return Err(crate::proxy::ProxyError::Internal(
            "OpenCode personal keys require the official inference endpoint".into(),
        ));
    }
    let base = if plan == GO {
        "/inference/go/"
    } else if plan == ZEN {
        "/inference/"
    } else {
        return Err(crate::proxy::ProxyError::ConfigError(
            "OpenCode personal key plan is invalid".into(),
        ));
    };
    let endpoint = parsed.path().strip_prefix(base).unwrap_or("");
    if !matches!(
        endpoint,
        "openai/v1/responses" | "openai/v1/chat/completions" | "anthropic/v1/messages"
    ) {
        return Err(crate::proxy::ProxyError::ConfigError(
            "OpenCode endpoint does not match the explicitly selected plan and protocol".into(),
        ));
    }
    headers.insert(
        http::header::USER_AGENT,
        http::HeaderValue::from_static(concat!("LoongPort/", env!("CARGO_PKG_VERSION"))),
    );
    if let Some(session) = client_session.filter(|s| !s.trim().is_empty()) {
        let value = http::HeaderValue::from_str(session).map_err(|_| {
            crate::proxy::ProxyError::Internal("OpenCode conversation ID is invalid".into())
        })?;
        headers.insert("x-opencode-session", value);
    } else if parsed.path().starts_with("/inference/go/")
        && headers
            .get("x-opencode-session")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| v.trim().is_empty())
    {
        return Err(crate::proxy::ProxyError::Internal(
            "OpenCode Go requires a stable conversation ID from the calling application".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    fn go_requires_stable_client_identity_and_never_changes_body_or_auth() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-opencode-session", http::HeaderValue::from_static("   "));
        assert!(request_headers(
            GO,
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
            &mut headers,
            None
        )
        .is_err());
        headers.remove("x-opencode-session");
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer SYNTHETIC_ONLY"),
        );
        assert!(request_headers(
            GO,
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
            &mut headers,
            None
        )
        .is_err());
        request_headers(
            GO,
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
            &mut headers,
            Some("fixture-conversation"),
        )
        .unwrap();
        assert_eq!(headers["x-opencode-session"], "fixture-conversation");
        assert_eq!(
            headers[http::header::AUTHORIZATION],
            "Bearer SYNTHETIC_ONLY"
        );
        assert!(headers[http::header::USER_AGENT]
            .to_str()
            .unwrap()
            .starts_with("LoongPort/"));
        assert!(request_headers(
            GO,
            "https://opencode.ai/inference/openai/v1/chat/completions",
            &mut headers,
            Some("fixture-conversation")
        )
        .is_err());
        assert!(request_headers(
            ZEN,
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
            &mut headers,
            Some("fixture-conversation")
        )
        .is_err());
        for url in [
            "https://opencode.ai.evil.invalid/inference/openai/v1/responses",
            "http://opencode.ai/inference/openai/v1/responses",
            "https://opencode.ai/zen/v1/responses",
        ] {
            assert!(request_headers(ZEN, url, &mut headers, Some("fixture-conversation")).is_err());
        }
    }
}
