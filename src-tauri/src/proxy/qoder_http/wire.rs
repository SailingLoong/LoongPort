//! Private per-request transport framing. The existing host checks binding and
//! requires an explicit clean end plus a successfully reaped worker exit.
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use serde_json::Value;

const MAX_CHUNK: usize = 48 * 1024;

pub(super) enum Frame {
    Head(StatusCode, HeaderMap),
    Data(Bytes),
}

pub(super) struct Decoder {
    head: bool,
    ended: bool,
    total: usize,
}

impl Decoder {
    pub(super) fn new() -> Self {
        Self {
            head: false,
            ended: false,
            total: 0,
        }
    }

    pub(super) fn event(&mut self, value: &Value) -> Result<Frame, ()> {
        if self.ended {
            return Err(());
        }
        let object = value.as_object().ok_or(())?;
        let frame = match value["type"].as_str() {
            Some("response_head") if !self.head => {
                if object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "accountId" | "sessionId" | "turnId" | "type" | "status" | "headers"
                    )
                }) {
                    return Err(());
                }
                let status = value["status"]
                    .as_u64()
                    .and_then(|value| u16::try_from(value).ok())
                    .and_then(|value| StatusCode::from_u16(value).ok())
                    .ok_or(())?;
                // No interim replies, redirects, or bodyless handoff semantics.
                if status.as_u16() > 599
                    || status.as_u16() < 200
                    || status.is_redirection()
                    || status == StatusCode::NO_CONTENT
                {
                    return Err(());
                }
                let mut headers = HeaderMap::new();
                for (name, value) in value["headers"].as_object().ok_or(())? {
                    if !matches!(name.as_str(), "content-type" | "cache-control") {
                        return Err(());
                    }
                    let text = value
                        .as_str()
                        .filter(|value| value.len() <= 256)
                        .ok_or(())?;
                    if (name == "content-type"
                        && !matches!(text, "application/json" | "text/event-stream"))
                        || (name == "cache-control" && !matches!(text, "no-cache" | "no-store"))
                    {
                        return Err(());
                    }
                    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?;
                    let value = HeaderValue::from_str(text).map_err(|_| ())?;
                    headers.insert(name, value);
                }
                if !headers.contains_key("content-type") {
                    return Err(());
                }
                self.head = true;
                Frame::Head(status, headers)
            }
            Some("response_chunk") if self.head => {
                if object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "accountId" | "sessionId" | "turnId" | "type" | "data"
                    )
                }) {
                    return Err(());
                }
                let encoded = value["data"]
                    .as_str()
                    .filter(|value| value.len() <= MAX_CHUNK / 3 * 4)
                    .ok_or(())?;
                let bytes = STANDARD.decode(encoded).map_err(|_| ())?;
                if bytes.is_empty() || bytes.len() > MAX_CHUNK {
                    return Err(());
                }
                let total = self.total.checked_add(bytes.len()).ok_or(())?;
                if total > super::super::hyper_client::MAX_RESPONSE_BODY_BYTES {
                    return Err(());
                }
                self.total = total;
                Frame::Data(Bytes::from(bytes))
            }
            _ => return Err(()),
        };
        Ok(frame)
    }

    /// Called only after QoderHost verifies clean end AND real worker exit 0.
    pub(super) fn finish(&mut self) -> Result<(), ()> {
        if self.ended || !self.head || self.total == 0 {
            return Err(());
        }
        self.ended = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn head() -> Value {
        json!({"type":"response_head","status":200,"headers":{"content-type":"text/event-stream"}})
    }
    fn chunk() -> Value {
        json!({"type":"response_chunk","data":STANDARD.encode(b"data: exact\n\n")})
    }
    #[test]
    fn preserves_bytes_and_single_head_end() {
        let mut decoder = Decoder::new();
        assert!(matches!(
            decoder.event(&head()),
            Ok(Frame::Head(StatusCode::OK, _))
        ));
        assert!(decoder.event(&head()).is_err());
        match decoder.event(&chunk()).unwrap() {
            Frame::Data(bytes) => assert_eq!(&bytes[..], b"data: exact\n\n"),
            _ => panic!("data expected"),
        }
        assert!(decoder.finish().is_ok());
        assert!(decoder.finish().is_err());
        assert!(decoder.event(&chunk()).is_err());
    }
    #[test]
    fn rejects_out_of_order_empty_and_unknown_frames() {
        let mut decoder = Decoder::new();
        assert!(decoder.event(&chunk()).is_err());
        assert!(decoder.finish().is_err());
        assert!(decoder.event(&head()).is_ok());
        assert!(decoder.finish().is_err());
        assert!(decoder.event(&json!({"type":"tool_call"})).is_err());
        assert!(decoder
            .event(&json!({"type":"response_chunk","data":""}))
            .is_err());
        assert!(decoder
            .event(&json!({"type":"response_chunk","data":"?"}))
            .is_err());
    }
    #[test]
    fn rejects_redirect_headers_and_oversized_chunks() {
        for field in ["authorization", "set-cookie", "location", "content-length"] {
            let mut value = head();
            value["headers"][field] = json!("private");
            assert!(Decoder::new().event(&value).is_err());
        }
        let mut value = head();
        value["status"] = json!(302);
        assert!(Decoder::new().event(&value).is_err());
        let mut decoder = Decoder::new();
        decoder.event(&head()).unwrap();
        assert!(decoder
            .event(&json!({"type":"response_chunk","data":STANDARD.encode(vec![0; MAX_CHUNK+1])}))
            .is_err());
        decoder.total = super::super::super::hyper_client::MAX_RESPONSE_BODY_BYTES;
        assert!(decoder.event(&chunk()).is_err());
    }
}
