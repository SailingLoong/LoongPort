use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Cn,
    Global,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Invalid,
    Context,
    Expired,
}
// Wire facts: MiniMax-AI/minimax-code 9a935605956c4500e9cb415f29258e2887e97817,
// packages/oauth-core (MIT; licenses/minimax-code-MIT.txt). Independent parser.
// Only the access capability is retained privately. Local JWT claims must
// not be projected as a verified profile realUserID.
pub struct NativeCredential {
    pub subject: Option<String>,
    pub access_token: zeroize::Zeroizing<String>,
    pub generation: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub account_id: Option<String>,
    pub expires_at_ms: i64,
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str, Failure> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty() && s.len() <= 16384 && s.trim() == *s && !s.chars().any(char::is_control)
        })
        .ok_or(Failure::Invalid)
}
pub fn parse_native(
    raw: &[u8],
    auth_home: &str,
    region: Region,
    now: i64,
) -> Result<NativeCredential, Failure> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    if raw.len() > 1024 * 1024 || now < 0 || auth_home.is_empty() {
        return Err(Failure::Invalid);
    }
    let v: Value = serde_json::from_slice(raw).map_err(|_| Failure::Invalid)?;
    if v.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(Failure::Invalid);
    }
    let account = URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!("{auth_home}\0mcode-public").as_bytes(),
    ));
    let r = match region {
        Region::Cn => "cn",
        Region::Global => "en",
    };
    let key = format!("com.minimax.mcode.oauth.prod.{r}\0{account}");
    let records = v
        .get("records")
        .and_then(Value::as_object)
        .ok_or(Failure::Invalid)?;
    let c = records.get(&key).ok_or(Failure::Context)?;
    if c.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(Failure::Invalid);
    }
    if text(c, "clientId")? != "mcode-public"
        || text(c, "audience")? != "agent-backend"
        || text(c, "tokenType")? != "Bearer"
    {
        return Err(Failure::Context);
    }
    let scopes = c
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or(Failure::Invalid)?;
    if !scopes.iter().all(Value::is_string) {
        return Err(Failure::Invalid);
    }
    if !scopes.iter().any(|s| s.as_str() == Some("agent.default")) {
        return Err(Failure::Context);
    }
    let access_token = text(c, "accessToken")?;
    text(c, "refreshToken")?;
    let expires = c
        .get("expiresAtMs")
        .and_then(Value::as_i64)
        .filter(|v| *v <= 9007199254740991)
        .ok_or(Failure::Invalid)?;
    if expires <= now {
        return Err(Failure::Expired);
    }
    let generation = c
        .get("generation")
        .and_then(Value::as_u64)
        .filter(|v| *v <= 9007199254740991)
        .ok_or(Failure::Invalid)?;
    // StoredCredential.subject is optional in the official store contract.
    // It is display-only and is never substituted for the verified realUserID.
    let subject = if c.get("subject").is_some() {
        Some(text(c, "subject")?.to_owned())
    } else {
        None
    };
    if subject.as_ref().is_some_and(|s| s.len() > 256) {
        return Err(Failure::Invalid);
    }
    let account_id = if c.get("accountId").is_some() {
        Some(text(c, "accountId")?.to_owned())
    } else {
        None
    };
    if c.get("loginEpoch").is_some() {
        text(c, "loginEpoch")?;
    }
    Ok(NativeCredential {
        subject,
        access_token: zeroize::Zeroizing::new(access_token.to_owned()),
        generation,
        account_id,
        expires_at_ms: expires,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    fn fixture(region: Region) -> Value {
        let r = if region == Region::Cn { "cn" } else { "en" };
        let account =
            URL_SAFE_NO_PAD.encode(Sha256::digest(b"/fixture/.minimax/auth\0mcode-public"));
        let key = format!("com.minimax.mcode.oauth.prod.{r}\0{account}");
        serde_json::json!({"schemaVersion":1,"records":{key:{"schemaVersion":1,"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","tokenType":"Bearer","clientId":"mcode-public","scopes":["agent.default"],"audience":"agent-backend","expiresAtMs":200000,"generation":2,"subject":"fixture-user","accountId":"fixture-user","loginEpoch":"fixture-epoch"}}})
    }
    fn run(v: &Value, r: Region) -> Result<NativeCredential, Failure> {
        parse_native(
            &serde_json::to_vec(v).unwrap(),
            "/fixture/.minimax/auth",
            r,
            100000,
        )
    }
    fn record(v: &mut Value) -> &mut Value {
        v["records"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
    }
    #[test]
    fn parses_cn_record() {
        assert_eq!(
            run(&fixture(Region::Cn), Region::Cn)
                .unwrap()
                .subject
                .as_deref(),
            Some("fixture-user")
        );
    }
    #[test]
    fn parses_global_record() {
        assert!(run(&fixture(Region::Global), Region::Global).is_ok());
    }
    #[test]
    fn rejects_cross_region() {
        assert!(matches!(
            run(&fixture(Region::Cn), Region::Global),
            Err(Failure::Context)
        ));
    }
    #[test]
    fn rejects_expired() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["expiresAtMs"] = 99999.into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Expired)));
    }
    #[test]
    fn rejects_empty_refresh() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["refreshToken"] = "".into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Invalid)));
    }
    #[test]
    fn accepts_missing_optional_subject_without_inventing_identity() {
        let mut v = fixture(Region::Cn);
        record(&mut v).as_object_mut().unwrap().remove("subject");
        assert!(run(&v, Region::Cn).is_ok());
    }
    #[test]
    fn preserves_distinct_account_and_subject() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["accountId"] = "billing-account".into();
        assert_eq!(
            run(&v, Region::Cn).unwrap().account_id.as_deref(),
            Some("billing-account")
        );
    }
    #[test]
    fn rejects_wrong_client() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["clientId"] = "other-client".into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Context)));
    }
    #[test]
    fn rejects_wrong_audience() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["audience"] = "other".into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Context)));
    }
    #[test]
    fn rejects_wrong_scope() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["scopes"] = serde_json::json!(["other"]);
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Context)));
    }
    #[test]
    fn rejects_wrong_home() {
        let v = fixture(Region::Cn);
        assert!(matches!(
            parse_native(
                &serde_json::to_vec(&v).unwrap(),
                "/fixture/other/auth",
                Region::Cn,
                100000
            ),
            Err(Failure::Context)
        ));
    }
    #[test]
    fn rejects_old_schema() {
        let mut v = fixture(Region::Cn);
        v["schemaVersion"] = 2.into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Invalid)));
    }
    #[test]
    fn rejects_equal_expiry() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["expiresAtMs"] = 100000.into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Expired)));
    }
    #[test]
    fn rejects_negative_generation() {
        let mut v = fixture(Region::Cn);
        record(&mut v)["generation"] = (-1).into();
        assert!(matches!(run(&v, Region::Cn), Err(Failure::Invalid)));
    }
    #[test]
    fn rejects_legacy_token_only() {
        assert!(matches!(
            run(
                &serde_json::json!({"tokens":{"accessToken":"synthetic-access"}}),
                Region::Cn
            ),
            Err(Failure::Invalid)
        ));
    }
    #[test]
    fn malformed_error_is_redacted() {
        let e = parse_native(
            b"synthetic-secret-not-json",
            "/fixture/auth",
            Region::Cn,
            100,
        )
        .err()
        .unwrap();
        assert_eq!(format!("{e:?}"), "Invalid");
    }
}
