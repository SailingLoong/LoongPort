//! International campaign HTTP adapter. Fixed production origin, normal TLS,
//! no redirects/retry/fallback; no secrets or device output are logged/persisted.
use super::{
    device::{DeviceIdentity, DeviceSource, InstalledApp},
    *,
};
use reqwest::{header, Method, Url};

use zeroize::Zeroizing;
const ORIGIN: &str = "https://openapi.qoder.sh";
const CAMPAIGNS: &str = "/sash/api/v1/me/campaigns?clientType=10";

/// A per-action source declaration, never an authentication proof. An old generic
/// Provider row is unknown until the user explicitly selects this source.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatDeclaration {
    pub provider_id: String,
    pub credential_generation: String,
    pub source: CredentialSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialSource {
    InternationalQoderPat,
}

pub(super) struct Session {
    bearer: Zeroizing<String>,
    device: DeviceIdentity,
}
pub(super) struct HttpAdapter {
    client: reqwest::Client,
    origin: Url,
    declaration: Option<PatDeclaration>,
    device: Box<dyn DeviceSource>,
}
impl HttpAdapter {
    pub(super) fn production(declaration: Option<PatDeclaration>) -> Result<Self, AppError> {
        Self::new(
            Url::parse(ORIGIN).map_err(|_| unavailable())?,
            declaration,
            Box::new(InstalledApp),
        )
    }
    fn new(
        origin: Url,
        declaration: Option<PatDeclaration>,
        device: Box<dyn DeviceSource>,
    ) -> Result<Self, AppError> {
        let client_builder = crate::services::provider::qoder_auth_http::client_builder();
        // Synthetic local-server tests must never inherit a host proxy. This
        // branch is absent from production and changes no real HTTPS policy.
        #[cfg(test)]
        let client_builder = if origin.scheme() == "http" && origin.host_str() == Some("127.0.0.1")
        {
            client_builder.no_proxy()
        } else {
            client_builder
        };
        let client = client_builder.build().map_err(|_| unavailable())?;
        Ok(Self {
            client,
            origin,
            declaration,
            device,
        })
    }
    fn request(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        device: Option<&DeviceIdentity>,
        body: Option<Value>,
    ) -> Result<HttpResponse, AppError> {
        let url = self.origin.join(path).map_err(|_| unavailable())?;
        let mut request = self
            .client
            .request(method.clone(), url)
            .header(header::ACCEPT, "application/json");
        if let Some(bearer) = bearer {
            request = request
                .bearer_auth(bearer)
                .header("cosy-clienttype", "10")
                .header(header::USER_AGENT, "Qoder");
        }
        if let Some(device) = device {
            request = request
                .header("cosy-machinetoken", device.token.as_str())
                .header("cosy-machinecode", device.code.as_str())
                .header("cosy-machinetype", device.kind.as_str());
        }
        if method == Method::POST {
            request = request.header(header::ORIGIN, self.origin.as_str().trim_end_matches('/'));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let started = Instant::now();
        let response = crate::rt::block_on(async {
            let response = request.send().await.map_err(|_| unavailable())?;
            Ok::<_, AppError>(HttpResponse::read(response, started).await)
        })?;
        if !self.matches(&response.response) {
            return Err(unavailable());
        }
        Ok(response)
    }
    fn matches(&self, response: &reqwest::Response) -> bool {
        let url = response.url();
        url.origin() == self.origin.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && !response.status().is_redirection()
    }
    fn campaign_request(
        &self,
        context: &VerifiedContext,
        method: Method,
        path: &str,
    ) -> Result<Response, AppError> {
        let session = context.session.as_ref().ok_or_else(unavailable)?;
        if context.origin != self.origin {
            return Err(unavailable());
        }
        let body = (method == Method::POST).then(|| serde_json::json!({}));
        let http = self.request(
            method,
            path,
            Some(&session.bearer),
            Some(&session.device),
            body,
        )?;
        Ok(Response {
            identity: context.identity.clone(),
            http,
        })
    }
    #[cfg(test)]
    pub(crate) fn synthetic_server(
        origin: Url,
        declaration: Option<PatDeclaration>,
        device: Box<dyn DeviceSource>,
    ) -> Self {
        assert_eq!(origin.scheme(), "http");
        assert_eq!(origin.host_str(), Some("127.0.0.1"));
        Self::new(origin, declaration, device).unwrap()
    }
}
impl CampaignAdapter for HttpAdapter {
    fn verify(&self, provider: &Provider, generation: &str) -> Result<VerifiedContext, Dependency> {
        let Some(declaration) = &self.declaration else {
            return Err(Dependency::AuthFamilyUnverified);
        };
        if declaration.provider_id != provider.id
            || declaration.credential_generation != generation
            || declaration.source != CredentialSource::InternationalQoderPat
        {
            return Err(Dependency::AuthFamilyUnverified);
        }
        self.device.availability()?;
        let candidate = Zeroizing::new(
            credential_candidate(provider)
                .map_err(|_| Dependency::ExternalCredentialSourceUntracked)?
                .filter(|v| !v.trim().is_empty())
                .ok_or(Dependency::IdentityUnavailable)?,
        );
        let exchange = self
            .request(
                Method::POST,
                crate::services::provider::qoder_auth_http::PAT_EXCHANGE_PATH,
                None,
                None,
                Some(
                    crate::services::provider::qoder_auth_http::pat_exchange_body(
                        candidate.as_str(),
                    ),
                ),
            )
            .map_err(|_| Dependency::AuthenticationFailed)?;
        if exchange.response.status() != reqwest::StatusCode::OK {
            return Err(Dependency::AuthenticationFailed);
        }
        let body = exchange.body.ok_or(Dependency::AuthenticationFailed)?;
        if !contract::non_error_envelope(&body) {
            return Err(Dependency::AuthenticationFailed);
        }
        // Only the known top-level token field; no legacy token/family fallback.
        let token = body
            .get("token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 16384)
            .ok_or(Dependency::AuthenticationFailed)?;
        let bearer = Zeroizing::new(token.to_owned());
        let bootstrap = self
            .request(Method::GET, CAMPAIGNS, Some(&bearer), None, None)
            .map_err(|_| Dependency::IdentityUnavailable)?;
        if bootstrap.response.status() != reqwest::StatusCode::OK {
            return Err(Dependency::IdentityUnavailable);
        }
        let uid = bootstrap
            .body
            .as_ref()
            .and_then(contract::listing_subject)
            .ok_or(Dependency::IdentityUnavailable)?
            .to_owned();
        let device = self.device.resolve(&uid)?;
        Ok(VerifiedContext {
            identity: Identity {
                provider_id: provider.id.clone(),
                uid,
                region: Region::International,
            },
            generation: generation.to_owned(),
            official_device: true,
            origin: self.origin.clone(),
            session: Some(Session { bearer, device }),
        })
    }
    fn list(&self, context: &VerifiedContext) -> Result<Listing, AppError> {
        let response = self.campaign_request(context, Method::GET, CAMPAIGNS)?;
        Ok(Listing {
            identity: response.identity,
            region: Region::International,
            http: response.http,
        })
    }
    fn post(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, AppError> {
        self.campaign_request(
            context,
            Method::POST,
            &format!(
                "/sash/api/v1/me/campaigns/{}/claim",
                binding.target().campaign_id
            ),
        )
    }
    fn get_reward(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, AppError> {
        self.campaign_request(
            context,
            Method::GET,
            &format!(
                "/sash/api/v1/me/campaigns/{}/reward",
                binding.target().campaign_id
            ),
        )
    }
}
#[cfg(test)]
pub(crate) fn synthetic_adapter(
    origin: Url,
    declaration: Option<PatDeclaration>,
    available: bool,
) -> impl CampaignAdapter {
    struct SyntheticDevice(bool);
    impl DeviceSource for SyntheticDevice {
        fn resolve(&self, subject: &str) -> Result<DeviceIdentity, Dependency> {
            assert_eq!(subject, "synthetic-official-subject");
            if !self.0 {
                return Err(Dependency::OfficialDeviceUnavailable);
            }
            Ok(DeviceIdentity {
                token: Zeroizing::new("synthetic-device-token".into()),
                code: Zeroizing::new("synthetic-device-code".into()),
                kind: Zeroizing::new("synthetic-device-type".into()),
            })
        }
    }
    HttpAdapter::synthetic_server(origin, declaration, Box::new(SyntheticDevice(available)))
}
