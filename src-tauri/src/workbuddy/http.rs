//! Official HTTPS transport. No native session access, redirects or write retries.
//! Package request bodies adapted from workbuddy-switch credits.rs (MIT).
use super::{model::*, protocol::*};
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, time::Duration};
pub(crate) struct OfficialHttp;
const PAID_CODES: &[&str] = &[
    "TCACA_code_002_AkiJS3ZHF5",
    "TCACA_code_023_4xbGhMrE6q",
    "TCACA_code_026_BaESVICNoi",
    "TCACA_code_027_0FCGVA6vSa",
    "TCACA_code_009_0XmEQc2xOf",
    "TCACA_code_038_OhvqZtiPKr",
    "TCACA_code_003_FAnt7lcmRT",
    "TCACA_code_036_lupO5WgNdG",
];
const FREE_CODES: &[&str] = &[
    "TCACA_code_008_cfWoLwvjU4",
    "TCACA_code_007_nzdH5h4Nl0",
    "TCACA_code_028_NtpWi0jzXs",
    "TCACA_code_029_6wCGEWquYy",
    "TCACA_code_030_BjSt89qTvr",
    "TCACA_code_001_PqouKr6QWV",
    "TCACA_code_006_DbXS0lrypC",
    "TCACA_code_035_ArVxJcGDsm",
    "TCACA_code_037_WxOD3MpI2o",
    "TCACA_code_039_KRcQj7wUat",
    "TCACA_code_040_mi9rCYg46x",
];
pub(crate) fn request_url(
    endpoint: Endpoint,
    account: Option<&Account>,
    state: Option<&str>,
) -> Result<reqwest::Url, Failure> {
    let host = match endpoint {
        Endpoint::AuthState | Endpoint::AuthToken => "www.codebuddy.cn",
        _ => account.ok_or(Failure::NeedsVerification)?.domain.host(),
    };
    let mut url = reqwest::Url::parse(&format!("https://{host}{}", endpoint.path()))
        .map_err(|_| Failure::UnsupportedContext)?;
    match endpoint {
        Endpoint::AuthState => {
            url.query_pairs_mut().append_pair("platform", "workbuddy");
        }
        Endpoint::AuthToken | Endpoint::Profile => {
            url.query_pairs_mut().append_pair(
                "state",
                state
                    .filter(|s| !s.is_empty() && s.len() <= 4096)
                    .ok_or(Failure::Unconfirmed)?,
            );
        }
        _ => {}
    }
    Ok(url)
}
pub(crate) fn body(endpoint: Endpoint, now: i64) -> Value {
    use chrono::TimeZone;
    match endpoint {
        Endpoint::Paid => {
            json!({"PageNumber":1,"PageSize":200,"Status":[0,3],"PackageCodes":PAID_CODES,"NeedRenewInfo":true,"IsDisplayTotalInfo":true})
        }
        Endpoint::Free => {
            let zone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
            let date = zone
                .timestamp_millis_opt(now)
                .single()
                .unwrap()
                .format("%Y-%m-%d");
            json!({"PageNumber":1,"PageSize":200,"Status":[0,3],"PackageCodes":FREE_CODES,"SlicePeriodStartTime":format!("{date} 00:00:00"),"SlicePeriodEndTime":format!("{date} 23:59:59"),"IsDisplayTotalInfo":true})
        }
        _ => json!({}),
    }
}
impl Transport for OfficialHttp {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        account: Option<&'a Account>,
        state: Option<&'a str>,
        now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let url = request_url(endpoint, account, state)?;
            let client = reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| Failure::Unconfirmed)?;
            let get = matches!(endpoint, Endpoint::AuthToken | Endpoint::Profile);
            let mut request = if get {
                client.get(url)
            } else {
                client.post(url).json(&body(endpoint, now))
            };
            request = request.header("Accept", "application/json");
            if let Some(account) = account {
                request = request
                    .bearer_auth(&account.token)
                    .header("X-Domain", account.domain.host());
                if endpoint != Endpoint::Profile {
                    request = request.header("X-User-Id", &account.uid);
                } else {
                    request = request
                        .header("X-No-User-Id", "true")
                        .header("X-No-Enterprise-Id", "true");
                }
                if matches!(
                    endpoint,
                    Endpoint::Summary | Endpoint::Paid | Endpoint::Free
                ) {
                    let origin = format!("https://{}", account.domain.host());
                    request = request
                        .header("X-Client-Platform", "web")
                        .header("Origin", &origin)
                        .header("Referer", format!("{origin}/profile/plans-usage"));
                }
            }
            let mut response = request.send().await.map_err(|_| Failure::Unconfirmed)?;
            if matches!(response.status().as_u16(), 401 | 403) {
                return Err(Failure::NeedsVerification);
            }
            if !response.status().is_success() {
                return Err(Failure::Unconfirmed);
            }
            const MAX: usize = 4 * 1024 * 1024;
            if response.content_length().is_some_and(|n| n > MAX as u64) {
                return Err(Failure::Unconfirmed);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Unconfirmed)? {
                if bytes.len() + chunk.len() > MAX {
                    return Err(Failure::Unconfirmed);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| Failure::Unconfirmed)
        })
    }
}
