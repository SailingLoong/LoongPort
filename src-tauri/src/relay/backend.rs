//! 中转站协议适配器的共享契约。
//!
//! 协议模块拥有 endpoint、wire DTO 和 detector；discovery 只遍历这里的窄描述符，
//! 不携带任何协议专属响应类型。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Sub2Api,
    NewApi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSite {
    pub backend_kind: BackendKind,
    pub site_name: String,
    pub api_base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeCandidate {
    pub id: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Copy)]
pub struct ProbeAdapter {
    pub candidate: ProbeCandidate,
    pub detect: fn(&str) -> Option<DetectedSite>,
}
