//! Single-attempt HTTP transport for the fixed official protocol.
use super::official::{
    OfficialError, OfficialMethod, OfficialRequest, OfficialResponse, OfficialTransport,
    TransportFuture, MAX_RESPONSE_BYTES, REQUEST_TIMEOUT_SECS,
};
use std::time::Duration;
use zeroize::Zeroizing;

pub struct ReqwestOfficialTransport {
    client: reqwest::Client,
}
impl ReqwestOfficialTransport {
    pub fn new() -> Result<Self, OfficialError> {
        Ok(Self {
            client: http_client_builder()
                .build()
                .map_err(|_| OfficialError::Transport)?,
        })
    }
}
impl OfficialTransport for ReqwestOfficialTransport {
    fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            validate_request(&request)?;
            let method = match request.method {
                OfficialMethod::Get => reqwest::Method::GET,
                OfficialMethod::Post => reqwest::Method::POST,
            };
            let mut builder = self
                .client
                .request(method, &request.url)
                .header(reqwest::header::CONTENT_TYPE, "application/json");
            if let Some(authorization) = request.authorization.as_ref() {
                let mut header = reqwest::header::HeaderValue::from_str(authorization.expose())
                    .map_err(|_| OfficialError::InvalidInput)?;
                header.set_sensitive(true);
                builder = builder.header(reqwest::header::AUTHORIZATION, header);
            }
            if let Some(body) = request.body.as_ref() {
                builder = builder.body(body.to_vec());
            }
            let request = builder.build().map_err(|_| OfficialError::InvalidInput)?;
            execute_bounded(&self.client, request).await
        })
    }
}
fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
}
fn validate_request(request: &OfficialRequest) -> Result<(), OfficialError> {
    let url = url::Url::parse(&request.url).map_err(|_| OfficialError::InvalidInput)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.fragment().is_some()
    {
        return Err(OfficialError::InvalidInput);
    }
    if request
        .body
        .as_ref()
        .is_some_and(|body| body.len() > MAX_RESPONSE_BYTES)
        || (request.method == OfficialMethod::Get && request.body.is_some())
    {
        return Err(OfficialError::InvalidInput);
    }
    let host = url.host_str().ok_or(OfficialError::InvalidInput)?;
    let path = url.path();
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let get = request.method == OfficialMethod::Get;
    let allowed = match host {
        "zcode.z.ai" => {
            (path == "/api/v1/oauth/cli/init" && !get)
                || (get
                    && parts.len() == 6
                    && parts[..5] == ["api", "v1", "oauth", "cli", "poll"]
                    && !parts[5].is_empty())
                || (get && path == "/api/v1/zcode-plan/billing/balance")
        }
        "bigmodel.cn" | "api.z.ai" => {
            (host == "api.z.ai" && path == "/api/auth/z/login" && !get)
                || (get
                    && matches!(
                        path,
                        "/api/biz/customer/getCustomerInfo"
                            | "/api/biz/subscription/list"
                            | "/api/monitor/usage/quota/limit"
                    ))
                || (parts.len() >= 8
                    && parts[..4] == ["api", "biz", "v1", "organization"]
                    && !parts[4].is_empty()
                    && parts[5] == "projects"
                    && !parts[6].is_empty()
                    && parts[7] == "api_keys"
                    && (parts.len() == 8
                        || (get
                            && parts.len() == 10
                            && parts[8] == "copy"
                            && !parts[9].is_empty())))
        }
        _ => false,
    };
    let query_allowed = if host == "zcode.z.ai" && path == "/api/v1/zcode-plan/billing/balance" {
        let pairs: Vec<_> = url.query_pairs().collect();
        pairs.len() == 1 && pairs[0].0 == "app_version" && !pairs[0].1.is_empty()
    } else {
        url.query().is_none()
    };
    if !allowed || !query_allowed {
        return Err(OfficialError::InvalidInput);
    }
    Ok(())
}
fn transport_error(error: reqwest::Error) -> OfficialError {
    if error.is_timeout() {
        OfficialError::Timeout
    } else {
        OfficialError::Transport
    }
}
async fn execute_bounded(
    client: &reqwest::Client,
    request: reqwest::Request,
) -> Result<OfficialResponse, OfficialError> {
    let mut response = client.execute(request).await.map_err(transport_error)?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(OfficialError::ResponseTooLarge);
    }
    let status = response.status().as_u16();
    let mut body = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(OfficialError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(OfficialResponse { status, body })
}

#[cfg(test)]
#[path = "official_http_tests.rs"]
mod tests;
