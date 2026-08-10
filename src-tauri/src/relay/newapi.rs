//! NewAPI 的窄 DTO 与严格响应 parser。
//!
//! 这里只承载协议形状，不负责 HTTP 请求、登录态或业务编排。所有 parser 都先验证
//! NewAPI 的 `success/data` envelope；服务端失败消息不回传，避免把敏感值带进错误文本。

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    _message: String,
    data: Option<T>,
}

fn parse_envelope<T: DeserializeOwned>(
    body: &str,
    operation: &str,
    requires_data: bool,
) -> Result<Option<T>, AppError> {
    let envelope: Envelope<T> = serde_json::from_str(body)
        .map_err(|error| AppError::Config(format!("newapi {operation} 响应格式无效: {error}")))?;
    if !envelope.success {
        return Err(AppError::Config(format!("newapi {operation} 请求失败")));
    }
    if requires_data && envelope.data.is_none() {
        return Err(AppError::Config(format!(
            "newapi {operation} 响应缺少 data"
        )));
    }
    Ok(envelope.data)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub system_name: String,
    pub theme: String,
    pub register_enabled: bool,
    pub password_login_enabled: bool,
}

pub fn parse_status(body: &str) -> Result<Status, AppError> {
    let status = parse_envelope::<Status>(body, "status", true)?
        .expect("requires_data guarantees status data");
    if status.version.trim().is_empty() {
        return Err(AppError::Config("newapi status 缺少非空 version".into()));
    }
    if status.system_name.trim().is_empty() {
        return Err(AppError::Config(
            "newapi status 缺少非空 system_name".into(),
        ));
    }
    if status.theme != "default" {
        return Err(AppError::Config("newapi status theme 不是 default".into()));
    }
    Ok(status)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfAccount {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub group: String,
    pub quota: i64,
    pub used_quota: i64,
}

pub fn parse_self(body: &str) -> Result<SelfAccount, AppError> {
    Ok(parse_envelope::<SelfAccount>(body, "self", true)?
        .expect("requires_data guarantees self data"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupIdentity(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub identity: GroupIdentity,
    pub name: String,
    pub rate_multiplier: Option<f64>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct GroupWire {
    ratio: RatioWire,
    desc: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RatioWire {
    Number(f64),
    Text(String),
}

impl RatioWire {
    fn into_rate_multiplier(self) -> Result<Option<f64>, AppError> {
        match self {
            Self::Number(value) if value.is_finite() => Ok(Some(value)),
            Self::Number(_) => Err(AppError::Config("newapi groups ratio 不是有限数字".into())),
            Self::Text(value) if value == "自动" => Ok(None),
            Self::Text(_) => Err(AppError::Config(
                "newapi groups ratio 不是数字或自动".into(),
            )),
        }
    }
}

pub fn parse_groups(body: &str) -> Result<Vec<Group>, AppError> {
    let groups = parse_envelope::<BTreeMap<String, GroupWire>>(body, "groups", true)?
        .expect("requires_data guarantees groups data");
    groups
        .into_iter()
        .map(|(name, wire)| {
            Ok(Group {
                identity: GroupIdentity(name.clone()),
                name,
                rate_multiplier: wire.ratio.into_rate_multiplier()?,
                description: wire.desc,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub status: i64,
    #[serde(default)]
    pub remain_quota: i64,
    #[serde(default)]
    pub used_quota: i64,
    #[serde(default)]
    pub unlimited_quota: bool,
    #[serde(default)]
    pub expired_time: i64,
    #[serde(default)]
    pub created_time: i64,
    #[serde(default)]
    pub accessed_time: i64,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub auto_groups: Option<Vec<String>>,
    #[serde(default)]
    pub cross_group_retry: bool,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: String,
    #[serde(default)]
    pub allow_ips: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenPage {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub items: Vec<Token>,
}

pub fn parse_token_list(body: &str) -> Result<TokenPage, AppError> {
    Ok(parse_envelope::<TokenPage>(body, "token list", true)?
        .expect("requires_data guarantees token list data"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCreate;

pub fn parse_token_create(body: &str) -> Result<TokenCreate, AppError> {
    parse_envelope::<serde_json::Value>(body, "token create", false)?;
    Ok(TokenCreate)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenReveal {
    pub key: String,
}

pub fn parse_token_reveal(body: &str) -> Result<TokenReveal, AppError> {
    let reveal = parse_envelope::<TokenReveal>(body, "token reveal", true)?
        .expect("requires_data guarantees token reveal data");
    if reveal.key.is_empty() {
        return Err(AppError::Config("newapi token reveal 响应缺少 key".into()));
    }
    Ok(reveal)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenDelete;

pub fn parse_token_delete(body: &str) -> Result<TokenDelete, AppError> {
    parse_envelope::<serde_json::Value>(body, "token delete", false)?;
    Ok(TokenDelete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parser_requires_success_envelope_and_stable_fields() {
        let status = parse_status(
            r#"{
                "success": true,
                "message": "",
                "data": {
                    "version": "1.2.3",
                    "system_name": "New API",
                    "theme": "default",
                    "register_enabled": true,
                    "password_login_enabled": false
                }
            }"#,
        )
        .unwrap();
        assert_eq!(status.version, "1.2.3");
        assert_eq!(status.system_name, "New API");
        assert!(!status.password_login_enabled);

        for body in [
            r#"{"success":false,"message":"nope","data":{}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":"New API","theme":"default","register_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":"New API","theme":"classic","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"","system_name":"New API","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":" ","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":1,"system_name":"New API","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
        ] {
            assert!(
                parse_status(body).is_err(),
                "accepted invalid status: {body}"
            );
        }
    }

    #[test]
    fn self_parser_reads_identity_and_quota_fields() {
        let account = parse_self(
            r#"{"success":true,"message":"","data":{
                "id":7,"username":"alice","display_name":"Alice","email":"a@example.com",
                "group":"vip","quota":12345,"used_quota":678
            }}"#,
        )
        .unwrap();
        assert_eq!(account.id, 7);
        assert_eq!(account.username, "alice");
        assert_eq!(account.display_name, "Alice");
        assert_eq!(account.email, "a@example.com");
        assert_eq!(account.group, "vip");
        assert_eq!(account.quota, 12345);
        assert_eq!(account.used_quota, 678);
    }

    #[test]
    fn groups_parser_supports_numeric_and_auto_ratios_without_losing_identity() {
        let groups = parse_groups(
            r#"{"success":true,"message":"","data":{
                "vip / 特殊": {"ratio": 0.75, "desc":"paid"},
                "自动": {"ratio":"自动", "desc":"automatic"}
            }}"#,
        )
        .unwrap();
        let special = groups
            .iter()
            .find(|group| group.name == "vip / 特殊")
            .unwrap();
        assert_eq!(special.identity.0, "vip / 特殊");
        assert_eq!(special.rate_multiplier, Some(0.75));
        let automatic = groups.iter().find(|group| group.name == "自动").unwrap();
        assert_eq!(automatic.identity.0, "自动");
        assert_eq!(automatic.rate_multiplier, None);
    }

    #[test]
    fn token_parsers_cover_list_create_reveal_and_delete() {
        let page = parse_token_list(
            r#"{"success":true,"message":"","data":{
                "page":1,"page_size":10,"total":1,
                "items":[{"id":9,"name":"relay","key":"sk-****","status":1}]
            }}"#,
        )
        .unwrap();
        assert_eq!(page.items[0].id, 9);
        assert_eq!(page.items[0].key, "sk-****");

        assert!(parse_token_create(r#"{"success":true,"message":""}"#).is_ok());
        assert!(parse_token_delete(r#"{"success":true,"message":""}"#).is_ok());
        assert_eq!(
            parse_token_reveal(r#"{"success":true,"message":"","data":{"key":"sk-full-secret"}}"#,)
                .unwrap()
                .key,
            "sk-full-secret"
        );
    }

    #[test]
    fn token_parsers_reject_failed_envelopes_without_leaking_reveal_key() {
        let error = parse_token_reveal(
            r#"{"success":false,"message":"bad sk-full-secret","data":{"key":"sk-full-secret"}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("sk-full-secret"));
        assert!(parse_token_list(r#"{"success":false,"message":"nope"}"#).is_err());
        assert!(parse_token_create(r#"{"success":false,"message":"nope"}"#).is_err());
        assert!(parse_token_delete(r#"{"success":false,"message":"nope"}"#).is_err());
    }
}
