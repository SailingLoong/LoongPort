//! Small shared protocol primitives. Campaign identity/device lifecycle stays
//! entirely in qoder_daily. This module owns no credentials, state, or endpoint choice.
use std::time::Duration;
pub(crate) const PAT_EXCHANGE_PATH: &str = "/api/v1/jobToken/exchange";
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
}
pub(crate) fn pat_exchange_body(pat: &str) -> serde_json::Value {
    serde_json::json!({"personal_token":pat})
}
