//! Fixed official ZCode protocol. Credentials never double as account identity.
//! Contract: zai-org/ZCode 29628c9acdb81b703bbd4080c207a0e7ce5e276e,
//! CLI cli-oauth.ts; accountProviderApiKeyResolver.ts; zaiStartPlanBilling.ts;
//! codingPlanEntitlement.ts and bigmodelUsageQuotaProvider.ts.
use super::core::OAuthFamily;
use serde_json::Value;
use std::{fmt, future::Future, pin::Pin};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const REQUEST_TIMEOUT_SECS: u64 = 15;
pub const KEY_NAME: &str = "zcode-api-key";
pub type RequestCheck<'a> = dyn Fn() -> Result<(), OfficialError> + Send + Sync + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficialError {
    Cancelled,
    InvalidInput,
    InvalidResponse,
    ResponseTooLarge,
    Unauthorized,
    Http(u16),
    BusinessRejected,
    Transport,
    Timeout,
    ConsentMismatch,
}
impl fmt::Display for OfficialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for OfficialError {}

#[derive(Clone)]
pub struct SecretValue(Zeroizing<String>);
impl SecretValue {
    pub fn new(value: &str) -> Result<Self, OfficialError> {
        let value = value.trim();
        if value.is_empty() || value.len() > 16 * 1024 || value.chars().any(char::is_control) {
            return Err(OfficialError::InvalidInput);
        }
        Ok(Self(Zeroizing::new(value.into())))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
macro_rules! credential {
    ($name:ident) => {
        #[derive(Clone, Debug)]
        pub struct $name(SecretValue);
        impl $name {
            pub fn new(value: &str) -> Result<Self, OfficialError> {
                SecretValue::new(value).map(Self)
            }
            pub fn expose(&self) -> &str {
                self.0.expose()
            }
        }
    };
}
credential!(PollToken);
credential!(ProviderAccessToken);
credential!(StartJwt);
credential!(CodingKey);

#[derive(Clone, Debug)]
pub struct BusinessToken {
    family: OAuthFamily,
    secret: SecretValue,
}
impl BusinessToken {
    /// Reads an already normalized stored business token; does not authenticate it.
    pub fn from_stored(family: OAuthFamily, value: &str) -> Result<Self, OfficialError> {
        Ok(Self {
            family,
            secret: SecretValue::new(value)?,
        })
    }
    pub fn family(&self) -> OAuthFamily {
        self.family
    }
    pub fn expose(&self) -> &str {
        self.secret.expose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthInit {
    pub authorize_url: String,
    pub flow_id: String,
    pub expires_at: u64,
    pub poll_interval_sec: u64,
}
#[derive(Clone)]
pub struct OfficialUser {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}
impl fmt::Debug for OfficialUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OfficialUser([REDACTED])")
    }
}
#[derive(Debug)]
pub struct PollReady {
    pub family: OAuthFamily,
    pub user: OfficialUser,
    pub start_jwt: StartJwt,
    pub provider_access_token: ProviderAccessToken,
    pub refresh_token: Option<SecretValue>,
}
#[derive(Debug)]
pub enum OAuthPoll {
    Pending,
    Failed,
    Ready(PollReady),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalProject {
    pub organization_id: String,
    pub project_id: String,
    pub organization_name: Option<String>,
    pub project_name: Option<String>,
}
pub struct CustomerInfo {
    pub identity_id: Option<String>,
    pub display_name: Option<String>,
    pub personal_project: Option<PersonalProject>,
}
#[derive(Debug)]
pub struct KeySummary {
    pub api_key: SecretValue,
    pub name: String,
}

/// Created by the flow owner only after current-account approval and durable intent.
/// Consumed by a single attempt; constructing another permit is never a retry policy.
pub struct KeyCreationPermit {
    account_id: String,
    family: OAuthFamily,
    project: PersonalProject,
}
impl KeyCreationPermit {
    pub fn new(
        account_id: &str,
        family: OAuthFamily,
        project: PersonalProject,
    ) -> Result<Self, OfficialError> {
        if account_id.trim().is_empty()
            || project.organization_id.trim().is_empty()
            || project.project_id.trim().is_empty()
        {
            return Err(OfficialError::InvalidInput);
        }
        Ok(Self {
            account_id: account_id.into(),
            family,
            project,
        })
    }
}
#[derive(Debug)]
pub enum CreateKeyOutcome {
    Created(KeySummary),
    NotSent(OfficialError),
    MayHaveBeenSent(OfficialError),
}

/// Backend-only parsed response. Its unknown strings must not reach UI or logs.
pub struct RemoteData(Value);
impl RemoteData {
    pub fn value(&self) -> &Value {
        &self.0
    }
}
impl fmt::Debug for RemoteData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteData([REDACTED])")
    }
}
impl Drop for RemoteData {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            match value {
                Value::String(s) => s.zeroize(),
                Value::Array(v) => v.iter_mut().for_each(wipe),
                Value::Object(v) => v.values_mut().for_each(wipe),
                _ => {}
            }
        }
        wipe(&mut self.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficialMethod {
    Get,
    Post,
}
pub struct OfficialRequest {
    pub method: OfficialMethod,
    pub url: String,
    pub authorization: Option<SecretValue>,
    pub body: Option<Zeroizing<Vec<u8>>>,
}
impl fmt::Debug for OfficialRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OfficialRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
pub struct OfficialResponse {
    pub status: u16,
    pub body: Zeroizing<Vec<u8>>,
}
impl fmt::Debug for OfficialResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OfficialResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<OfficialResponse, OfficialError>> + Send + 'a>>;
pub trait OfficialTransport: Send + Sync {
    fn send(&self, request: OfficialRequest) -> TransportFuture<'_>;
}

pub struct OfficialClient<T> {
    pub(super) transport: T,
}
impl<T: OfficialTransport> OfficialClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
    pub async fn init(
        &self,
        family: OAuthFamily,
        poll_token: &PollToken,
        check: &RequestCheck<'_>,
    ) -> Result<OAuthInit, OfficialError> {
        let request = OfficialRequest {
            method: OfficialMethod::Post,
            url: "https://zcode.z.ai/api/v1/oauth/cli/init".into(),
            authorization: Some(bearer(poll_token.expose())?),
            body: Some(Zeroizing::new(
                format!("{{\"provider\":\"{}\"}}", family_name(family)).into_bytes(),
            )),
        };
        let data = self.request(request, true, check).await?;
        let value = data.value();
        let authorize_url = field(value, "authorize_url")?.to_owned();
        let url = url::Url::parse(&authorize_url).map_err(|_| OfficialError::InvalidResponse)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(OfficialError::InvalidResponse);
        }
        let expires_at = value
            .get("expires_at")
            .and_then(Value::as_u64)
            .filter(|x| *x > 0)
            .ok_or(OfficialError::InvalidResponse)?;
        let poll_interval_sec = value
            .get("poll_interval_sec")
            .and_then(Value::as_u64)
            .filter(|x| *x >= 1 && *x <= 300)
            .ok_or(OfficialError::InvalidResponse)?;
        Ok(OAuthInit {
            authorize_url,
            flow_id: field(value, "flow_id")?.into(),
            expires_at,
            poll_interval_sec,
        })
    }
    async fn request(
        &self,
        request: OfficialRequest,
        strict_code: bool,
        check: &RequestCheck<'_>,
    ) -> Result<RemoteData, OfficialError> {
        check()?;
        let result = self.transport.send(request).await;
        check()?;
        parse_response(result?, strict_code)
    }
    pub async fn poll(
        &self,
        family: OAuthFamily,
        flow_id: &str,
        poll_token: &PollToken,
        check: &RequestCheck<'_>,
    ) -> Result<OAuthPoll, OfficialError> {
        let data = self
            .request(
                get(
                    format!(
                        "https://zcode.z.ai/api/v1/oauth/cli/poll/{}",
                        segment(flow_id)?
                    ),
                    bearer(poll_token.expose())?,
                ),
                true,
                check,
            )
            .await?;
        let value = data.value();
        match field(value, "status")? {
            "pending" => Ok(OAuthPoll::Pending),
            "failed" => Ok(OAuthPoll::Failed),
            "ready" => {
                let user = value.get("user").ok_or(OfficialError::InvalidResponse)?;
                let provider = value
                    .get(family_name(family))
                    .ok_or(OfficialError::InvalidResponse)?;
                Ok(OAuthPoll::Ready(PollReady {
                    family,
                    user: OfficialUser {
                        id: field(user, "user_id")?.into(),
                        name: optional_field(user, "name"),
                        email: optional_field(user, "email"),
                    },
                    start_jwt: StartJwt::new(field(value, "token")?)?,
                    provider_access_token: ProviderAccessToken::new(
                        alias(provider, "access_token", "accessToken")?
                            .ok_or(OfficialError::InvalidResponse)?,
                    )?,
                    refresh_token: alias(provider, "refresh_token", "refreshToken")?
                        .map(SecretValue::new)
                        .transpose()?,
                }))
            }
            _ => Err(OfficialError::InvalidResponse),
        }
    }
    pub async fn normalize_business_token(
        &self,
        family: OAuthFamily,
        access: &ProviderAccessToken,
        check: &RequestCheck<'_>,
    ) -> Result<BusinessToken, OfficialError> {
        check()?;
        if family == OAuthFamily::BigModel {
            return BusinessToken::from_stored(family, access.expose());
        }
        #[derive(serde::Serialize)]
        struct Login<'a> {
            token: &'a str,
        }
        let body = serde_json::to_vec(&Login {
            token: access.expose(),
        })
        .map_err(|_| OfficialError::InvalidInput)?;
        let data = self
            .request(
                OfficialRequest {
                    method: OfficialMethod::Post,
                    url: "https://api.z.ai/api/auth/z/login".into(),
                    authorization: None,
                    body: Some(Zeroizing::new(body)),
                },
                false,
                check,
            )
            .await?;
        BusinessToken::from_stored(
            family,
            alias(data.value(), "access_token", "accessToken")?
                .ok_or(OfficialError::InvalidResponse)?,
        )
    }
    pub async fn read_customer(
        &self,
        token: &BusinessToken,
        check: &RequestCheck<'_>,
    ) -> Result<CustomerInfo, OfficialError> {
        let data = self
            .request(
                get(
                    format!("{}/api/biz/customer/getCustomerInfo", host(token.family)),
                    business_auth(token)?,
                ),
                false,
                check,
            )
            .await?;
        if !data.value().is_object() {
            return Err(OfficialError::InvalidResponse);
        }
        Ok(CustomerInfo {
            identity_id: optional_field(data.value(), "customerNumber"),
            display_name: optional_field(data.value(), "customerName")
                .or_else(|| optional_field(data.value(), "nickName")),
            personal_project: pick_personal_project(data.value())?,
        })
    }
    pub async fn list_keys(
        &self,
        token: &BusinessToken,
        project: &PersonalProject,
        check: &RequestCheck<'_>,
    ) -> Result<Vec<KeySummary>, OfficialError> {
        let data = self
            .request(
                get(keys_url(token.family, project)?, business_auth(token)?),
                false,
                check,
            )
            .await?;
        let list = data
            .value()
            .as_array()
            .ok_or(OfficialError::InvalidResponse)?;
        list.iter().map(parse_key).collect()
    }
    pub async fn copy_key(
        &self,
        token: &BusinessToken,
        project: &PersonalProject,
        key: &KeySummary,
        check: &RequestCheck<'_>,
    ) -> Result<CodingKey, OfficialError> {
        let url = format!(
            "{}/copy/{}",
            keys_url(token.family, project)?,
            segment(key.api_key.expose())?
        );
        let data = self
            .request(get(url, business_auth(token)?), false, check)
            .await?;
        if !data.value().is_object() {
            return Err(OfficialError::InvalidResponse);
        }
        if let Ok(secret) = field(data.value(), "secretKey") {
            return CodingKey::new(&Zeroizing::new(format!(
                "{}.{}",
                key.api_key.expose(),
                secret
            )));
        }
        if token.family == OAuthFamily::BigModel {
            CodingKey::new(key.api_key.expose())
        } else {
            Err(OfficialError::InvalidResponse)
        }
    }
    pub async fn discover_key(
        &self,
        token: &BusinessToken,
        project: &PersonalProject,
        check: &RequestCheck<'_>,
    ) -> Result<Option<CodingKey>, OfficialError> {
        let keys = self.list_keys(token, project, check).await?;
        match keys.iter().find(|key| key.name == KEY_NAME) {
            Some(key) => self.copy_key(token, project, key, check).await.map(Some),
            None => Ok(None),
        }
    }
    pub async fn create_key_once(
        &self,
        token: &BusinessToken,
        account_id: &str,
        permit: KeyCreationPermit,
        check: &RequestCheck<'_>,
    ) -> CreateKeyOutcome {
        if permit.account_id != account_id || permit.family != token.family {
            return CreateKeyOutcome::NotSent(OfficialError::ConsentMismatch);
        }
        let request = (|| {
            Ok(OfficialRequest {
                method: OfficialMethod::Post,
                url: keys_url(token.family, &permit.project)?,
                authorization: Some(business_auth(token)?),
                body: Some(Zeroizing::new(b"{\"name\":\"zcode-api-key\"}".to_vec())),
            })
        })();
        let request = match request {
            Ok(request) => request,
            Err(error) => return CreateKeyOutcome::NotSent(error),
        };
        if let Err(error) = check() {
            return CreateKeyOutcome::NotSent(error);
        }
        // A transport failure, cancelled waiter, or malformed response cannot prove
        // the POST failed remotely. The owner must query the original project.
        let result = self.transport.send(request).await;
        if let Err(error) = check() {
            return CreateKeyOutcome::MayHaveBeenSent(error);
        }
        match result
            .and_then(|response| parse_response(response, false))
            .and_then(|data| parse_key(data.value()))
        {
            Ok(key) => CreateKeyOutcome::Created(key),
            Err(error) => CreateKeyOutcome::MayHaveBeenSent(error),
        }
    }
    pub async fn read_start_balance(
        &self,
        jwt: &StartJwt,
        app_version: &str,
        check: &RequestCheck<'_>,
    ) -> Result<RemoteData, OfficialError> {
        if app_version.is_empty()
            || app_version.len() > 64
            || !app_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
        {
            return Err(OfficialError::InvalidInput);
        }
        let url = format!(
            "https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version={}",
            segment(app_version)?
        );
        self.request(get(url, bearer(jwt.expose())?), false, check)
            .await
    }
    pub async fn read_coding_subscription(
        &self,
        family: OAuthFamily,
        key: &CodingKey,
        check: &RequestCheck<'_>,
    ) -> Result<RemoteData, OfficialError> {
        self.request(
            get(
                format!("{}/api/biz/subscription/list", host(family)),
                key.0.clone(),
            ),
            false,
            check,
        )
        .await
    }
    pub async fn read_coding_quota(
        &self,
        family: OAuthFamily,
        key: &CodingKey,
        check: &RequestCheck<'_>,
    ) -> Result<RemoteData, OfficialError> {
        self.request(
            get(
                format!("{}/api/monitor/usage/quota/limit", host(family)),
                key.0.clone(),
            ),
            false,
            check,
        )
        .await
    }
}

fn family_name(family: OAuthFamily) -> &'static str {
    match family {
        OAuthFamily::Zai => "zai",
        OAuthFamily::BigModel => "bigmodel",
    }
}
fn host(family: OAuthFamily) -> &'static str {
    match family {
        OAuthFamily::Zai => "https://api.z.ai",
        OAuthFamily::BigModel => "https://bigmodel.cn",
    }
}
fn bearer(value: &str) -> Result<SecretValue, OfficialError> {
    SecretValue::new(&Zeroizing::new(format!("Bearer {value}")))
}
fn business_auth(token: &BusinessToken) -> Result<SecretValue, OfficialError> {
    match token.family {
        OAuthFamily::Zai => bearer(token.expose()),
        OAuthFamily::BigModel => Ok(token.secret.clone()),
    }
}
fn get(url: String, authorization: SecretValue) -> OfficialRequest {
    OfficialRequest {
        method: OfficialMethod::Get,
        url,
        authorization: Some(authorization),
        body: None,
    }
}
fn segment(value: &str) -> Result<String, OfficialError> {
    if value.trim().is_empty() || value.len() > 16 * 1024 || matches!(value, "." | "..") {
        return Err(OfficialError::InvalidInput);
    }
    let mut encoded = String::new();
    use std::fmt::Write;
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    Ok(encoded)
}
fn keys_url(family: OAuthFamily, project: &PersonalProject) -> Result<String, OfficialError> {
    Ok(format!(
        "{}/api/biz/v1/organization/{}/projects/{}/api_keys",
        host(family),
        segment(&project.organization_id)?,
        segment(&project.project_id)?
    ))
}
fn optional_field(value: &Value, key: &str) -> Option<String> {
    field(value, key).ok().map(str::to_owned)
}
fn alias<'a>(
    value: &'a Value,
    first: &str,
    second: &str,
) -> Result<Option<&'a str>, OfficialError> {
    let left = field(value, first).ok();
    let right = field(value, second).ok();
    if let (Some(left), Some(right)) = (left, right) {
        if left != right {
            return Err(OfficialError::InvalidResponse);
        }
    }
    Ok(left.or(right))
}
fn parse_key(value: &Value) -> Result<KeySummary, OfficialError> {
    Ok(KeySummary {
        api_key: SecretValue::new(field(value, "apiKey")?)?,
        name: optional_field(value, "name").unwrap_or_default(),
    })
}
fn pick_personal_project(value: &Value) -> Result<Option<PersonalProject>, OfficialError> {
    let Some(organizations) = value.get("organizations") else {
        return Ok(None);
    };
    let organizations = organizations
        .as_array()
        .ok_or(OfficialError::InvalidResponse)?;
    let mut candidates = Vec::new();
    for org in organizations {
        let Ok(org_id) = field(org, "organizationId") else {
            continue;
        };
        let Some(projects) = org.get("projects").and_then(Value::as_array) else {
            continue;
        };
        let personal: Vec<_> = projects
            .iter()
            .filter(|project| {
                let team = match project.get("projectType") {
                    Some(Value::Number(n)) => n.as_i64() == Some(2),
                    Some(Value::String(s)) => s.trim() == "2",
                    _ => false,
                };
                !team && field(project, "projectId").is_ok()
            })
            .collect();
        let selected = personal
            .iter()
            .find(|project| field(project, "projectName").is_ok_and(|n| n.contains("默认项目")))
            .or(personal.first());
        if let Some(selected) = selected {
            candidates.push(PersonalProject {
                organization_id: org_id.into(),
                project_id: field(selected, "projectId")?.into(),
                organization_name: optional_field(org, "organizationName"),
                project_name: optional_field(selected, "projectName"),
            });
        }
    }
    let preferred = candidates
        .iter()
        .position(|project| {
            project
                .organization_name
                .as_ref()
                .is_some_and(|name| name.contains("默认机构"))
        })
        .unwrap_or(0);
    Ok(if candidates.is_empty() {
        None
    } else {
        Some(candidates.swap_remove(preferred))
    })
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, OfficialError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(OfficialError::InvalidResponse)
}
fn parse_response(
    response: OfficialResponse,
    strict_code: bool,
) -> Result<RemoteData, OfficialError> {
    if response.body.len() > MAX_RESPONSE_BYTES {
        return Err(OfficialError::ResponseTooLarge);
    }
    if matches!(response.status, 401 | 403) {
        return Err(OfficialError::Unauthorized);
    }
    if !(200..300).contains(&response.status) {
        return Err(OfficialError::Http(response.status));
    }
    let mut envelope = RemoteData(
        serde_json::from_slice(&response.body).map_err(|_| OfficialError::InvalidResponse)?,
    );
    let value = envelope.value();
    if !value.is_object() {
        return Err(OfficialError::InvalidResponse);
    }
    let code = value.get("code");
    let success = if strict_code {
        code == Some(&Value::from(0))
    } else {
        match code {
            None | Some(Value::Null) => true,
            Some(Value::Number(n)) => matches!(n.as_i64(), Some(0 | 200)),
            Some(Value::String(s)) => matches!(s.as_str(), "0" | "200"),
            _ => false,
        }
    };
    if !success || value.get("success") == Some(&Value::Bool(false)) {
        return Err(OfficialError::BusinessRejected);
    }
    let data = envelope
        .0
        .get_mut("data")
        .ok_or(OfficialError::InvalidResponse)?
        .take();
    Ok(RemoteData(data))
}

#[cfg(test)]
#[path = "official_tests.rs"]
mod tests;
