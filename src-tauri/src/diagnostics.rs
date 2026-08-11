//! 全应用诊断日志的唯一安全出口。
//!
//! 负责日志正文脱敏、长度上限、统一事件结构和错误链展开。具体日志目标、轮转和
//! 动态级别仍由 `tauri-plugin-log` 管理。

use regex::Regex;
use serde_json::{Map, Value};
use std::{error::Error, fmt, sync::LazyLock};

pub(crate) const MAX_LOG_MESSAGE_LENGTH: usize = 12_000;
pub(crate) const MAX_RAW_LOG_INPUT_LENGTH: usize = 16_000;
const MAX_ERROR_CHAIN_DEPTH: usize = 8;

static QUERY_VALUE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([?&][A-Za-z0-9_.~-]+)=([^&#\s"'<>]*)"#).expect("valid query regex")
});
static URL_CREDENTIAL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(https?://)[^/@\s]+@").expect("valid URL credential regex"));
static SENSITIVE_HEADER_LINE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)(^|[\r\n])([ \t]*(?:(?:proxy-)?authorization|cookie|set-cookie|x-api-key|api-key)\s*[:=]\s*)[^\r\n]+",
    )
    .expect("valid sensitive header regex")
});
static AUTH_SCHEME_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(Bearer|Basic|Token|ApiKey|Digest|Negotiate|AWS4-HMAC-SHA256)\s+[^\s"',}\]]+"#,
    )
    .expect("valid auth scheme regex")
});
static JSON_BODY_FIELD_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(["'](?:request[_-]?body|response[_-]?body|body|payload)["']\s*:\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\[[^\]]*\]|\{[^{}]*\}|[^,}\r\n]+)"#,
    )
    .expect("valid JSON body field regex")
});
static BODY_FIELD_LINE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?im)(^|[\r\n \t])((?:request[_-]?body|response[_-]?body|body|payload)\b\s*[:=]\s*).*$"#,
    )
    .expect("valid body field line regex")
});
static HTTP_STATUS_BODY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(HTTP\s+\d{3}\s*:\s*)[^\r\n]*").expect("valid HTTP body regex")
});
static NAMED_SECRET_CONTAINER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:\\?["'])?\b(?:api[_-]?key|access[_-]?key|secret[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|refresh[_-]?token|id[_-]?token|session[_-]?token|session[_-]?id|authorization|credential|password|passwd|bearer|cookie|secret|token|auth|pwd|key)s?(?:\\?["'])?\s*[:=]\s*)(?:\[[^\]]*\]|\{[^{}]*\})"#,
    )
    .expect("valid secret container regex")
});
static QUOTED_NAMED_SECRET_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b(?:api[_-]?key|access[_-]?key|secret[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|refresh[_-]?token|id[_-]?token|session[_-]?token|session[_-]?id|authorization|credential|password|passwd|bearer|cookie|secret|token|auth|pwd|key)s?\s*["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*')"#,
    )
    .expect("valid quoted secret regex")
});
static NAMED_SECRET_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b(?:api[_-]?key|access[_-]?key|secret[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|refresh[_-]?token|id[_-]?token|session[_-]?token|session[_-]?id|authorization|credential|password|passwd|bearer|cookie|secret|token|auth|pwd|key)s?\s*["']?\s*[:=]\s*["']?)([^\s"',}]+)"#,
    )
    .expect("valid named secret regex")
});
static SECRET_VALUE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[^A-Za-z0-9])(?:sk-[A-Za-z0-9._~+/=-]{6,}|AIza[A-Za-z0-9_-]{8,}|github_pat_[A-Za-z0-9_]{6,}|gh[pousr]_[A-Za-z0-9_]{6,}|xox[baprs]-[A-Za-z0-9-]{6,}|ya29\.[A-Za-z0-9._-]{6,}|(?:AKIA|ASIA)[A-Z0-9]{12,}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)",
    )
    .expect("valid secret value regex")
});
static PRIVATE_KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?(?:-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|\z)",
    )
    .expect("valid private key regex")
});

fn truncate_at_char_boundary(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

/// 所有 Rust 与 WebView 日志在写入 stdout/文件前都会经过这里。
pub(crate) fn redact_log_text(input: &str) -> String {
    let bounded = truncate_at_char_boundary(input, MAX_RAW_LOG_INPUT_LENGTH);
    let mut output = URL_CREDENTIAL_PATTERN
        .replace_all(bounded, "$1[REDACTED]@")
        .into_owned();
    output = NAMED_SECRET_CONTAINER_PATTERN
        .replace_all(&output, "$1\"[REDACTED]\"")
        .into_owned();
    output = QUERY_VALUE_PATTERN
        .replace_all(&output, "$1=[REDACTED]")
        .into_owned();
    output = SENSITIVE_HEADER_LINE_PATTERN
        .replace_all(&output, "$1$2[REDACTED]")
        .into_owned();
    output = AUTH_SCHEME_PATTERN
        .replace_all(&output, "$1 [REDACTED]")
        .into_owned();
    output = JSON_BODY_FIELD_PATTERN
        .replace_all(&output, "$1\"[REDACTED BODY]\"")
        .into_owned();
    output = BODY_FIELD_LINE_PATTERN
        .replace_all(&output, "$1$2[REDACTED BODY]")
        .into_owned();
    output = HTTP_STATUS_BODY_PATTERN
        .replace_all(&output, "$1[REDACTED RESPONSE BODY]")
        .into_owned();
    output = QUOTED_NAMED_SECRET_PATTERN
        .replace_all(&output, "$1\"[REDACTED]\"")
        .into_owned();
    output = NAMED_SECRET_PATTERN
        .replace_all(&output, "$1[REDACTED]")
        .into_owned();
    output = SECRET_VALUE_PATTERN
        .replace_all(&output, "$1[REDACTED]")
        .into_owned();
    output = PRIVATE_KEY_PATTERN
        .replace_all(&output, "[REDACTED PRIVATE KEY]")
        .into_owned();

    if input.len() > MAX_RAW_LOG_INPUT_LENGTH || output.len() > MAX_LOG_MESSAGE_LENGTH {
        let truncated = truncate_at_char_boundary(&output, MAX_LOG_MESSAGE_LENGTH);
        format!("{truncated}\n[truncated]")
    } else {
        output
    }
}

/// 统一的单行 JSON 诊断事件；字段排序稳定，便于 grep、脚本和 Issue 排查。
pub(crate) struct DiagnosticEvent {
    fields: Map<String, Value>,
}

impl DiagnosticEvent {
    pub(crate) fn new(event: impl Into<String>, outcome: impl Into<String>) -> Self {
        let mut fields = Map::new();
        fields.insert("event".into(), Value::String(event.into()));
        fields.insert("outcome".into(), Value::String(outcome.into()));
        Self { fields }
    }

    pub(crate) fn field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub(crate) fn field_display(
        mut self,
        key: impl Into<String>,
        value: impl fmt::Display,
    ) -> Self {
        self.fields
            .insert(key.into(), Value::String(value.to_string()));
        self
    }
}

impl fmt::Display for DiagnosticEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Value::Object(self.fields.clone()).fmt(formatter)
    }
}

pub(crate) trait ResultLogExt {
    /// 记录必须继续执行的 best-effort 失败。
    fn warn_on_err(self, event: DiagnosticEvent);

    /// 记录数据一致性或恢复流程中的严重 best-effort 失败。
    fn error_on_err(self, event: DiagnosticEvent);
}

impl<T, E: fmt::Display> ResultLogExt for Result<T, E> {
    fn warn_on_err(self, event: DiagnosticEvent) {
        if let Err(error) = self {
            log::warn!("{}", event.field_display("error", error));
        }
    }

    fn error_on_err(self, event: DiagnosticEvent) {
        if let Err(error) = self {
            log::error!("{}", event.field_display("error", error));
        }
    }
}

/// 展开标准 Error source 链，避免日志只剩最外层“操作失败”。
pub(crate) fn format_error_chain(error: &(dyn Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(current) = source {
        if parts.len() >= MAX_ERROR_CHAIN_DEPTH {
            parts.push("[error chain truncated]".into());
            break;
        }
        parts.push(current.to_string());
        source = current.source();
    }
    parts.join(" <- ")
}

/// 供 tauri-plugin-log formatter 使用，保证所有目标共享同一脱敏与行格式。
pub(crate) fn format_log_line(
    timestamp: &str,
    level: log::Level,
    target: &str,
    message: &str,
) -> String {
    format!(
        "[{timestamp}][{level}][{}] {}",
        redact_log_text(target),
        redact_log_text(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials_queries_headers_and_response_bodies() {
        let input = concat!(
            "url=https://user:pass@example.com/v1?token=secret-token&model=x\n",
            "Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz\n",
            "cookie=session=opaque-cookie\n",
            r#"payload={"access_token":"top-secret","nested":{"ok":true}}"#,
            "\n",
            "HTTP 403: <html>private verification body</html>",
        );

        let redacted = redact_log_text(input);

        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("secret-token"));
        assert!(!redacted.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(!redacted.contains("opaque-cookie"));
        assert!(!redacted.contains("top-secret"));
        assert!(!redacted.contains("private verification body"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("HTTP 403: [REDACTED RESPONSE BODY]"));
    }

    #[test]
    fn redacts_sensitive_arrays_and_objects_in_legacy_json() {
        let input = r#"legacy={\"tokens\":[\"short-secret\"],\"auth\":{\"session\":\"opaque\"},\"keep\":\"visible\"}"#;

        let redacted = redact_log_text(input);

        assert!(!redacted.contains("short-secret"));
        assert!(!redacted.contains("opaque"));
        assert!(redacted.contains("visible"));
    }

    #[test]
    fn keeps_non_sensitive_key_suffixes_visible() {
        let inputs = [
            r#"{"monkey":"banana","hotkey":"command-k"}"#,
            r#"{\"monkey\":\"banana\",\"hotkey\":\"command-k\"}"#,
            "monkey=banana hotkey=command-k",
        ];

        for input in inputs {
            assert_eq!(redact_log_text(input), input);
        }
    }

    #[test]
    fn redacts_truncated_private_keys_without_an_end_marker() {
        let input = format!(
            "-----BEGIN PRIVATE KEY-----\nprivate-material\n{}",
            "A".repeat(MAX_RAW_LOG_INPUT_LENGTH + 100)
        );

        let redacted = redact_log_text(&input);

        assert!(!redacted.contains("private-material"));
        assert!(!redacted.contains("BEGIN PRIVATE KEY"));
        assert!(redacted.contains("[REDACTED PRIVATE KEY]"));
    }

    #[test]
    fn bounds_log_messages_before_persisting() {
        let output = redact_log_text(&"x".repeat(MAX_RAW_LOG_INPUT_LENGTH + 100));
        assert!(output.len() <= MAX_LOG_MESSAGE_LENGTH + "\n[truncated]".len());
        assert!(output.ends_with("[truncated]"));
    }

    #[test]
    fn diagnostic_events_are_single_line_structured_json() {
        let event = DiagnosticEvent::new("relay.browser_probe", "unmatched")
            .field("site", "https://relay.example")
            .field("status", 403_u64)
            .field("json", false)
            .to_string();

        assert_eq!(
            event,
            r#"{"event":"relay.browser_probe","outcome":"unmatched","site":"https://relay.example","status":403,"json":false}"#
        );
        assert!(!event.contains('\n'));
    }

    #[test]
    fn final_redaction_preserves_structured_event_json() {
        let event = DiagnosticEvent::new("sync.status", "persist_failed")
            .field("response_body", "private response")
            .field("status", 403_u64)
            .to_string();

        let redacted = redact_log_text(&event);
        let parsed: Value =
            serde_json::from_str(&redacted).expect("redacted event stays valid JSON");

        assert_eq!(parsed["response_body"], "[REDACTED BODY]");
        assert_eq!(parsed["status"], 403);
        assert!(!redacted.contains("private response"));
    }

    #[test]
    fn error_chain_keeps_sources_for_root_cause_diagnosis() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "disk denied");
        let error = crate::AppError::IoContext {
            context: "write settings".into(),
            source: io,
        };

        assert_eq!(
            format_error_chain(&error),
            "write settings: disk denied <- disk denied"
        );
    }
}
