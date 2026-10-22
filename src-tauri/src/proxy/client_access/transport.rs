//! Separate local caller authentication from native upstream authorization.
use axum::http::{header::AUTHORIZATION, HeaderMap};

pub(crate) const CLIENT_KEY_HEADER: &str = "x-loongport-client-key";

pub(super) fn take_key(headers: &mut HeaderMap) -> Result<String, ()> {
    if headers.contains_key(CLIENT_KEY_HEADER) {
        if headers.get_all(CLIENT_KEY_HEADER).iter().count() != 1 {
            return Err(());
        }
        return headers
            .remove(CLIENT_KEY_HEADER)
            .ok_or(())?
            .to_str()
            .map(str::to_owned)
            .map_err(|_| ());
    }
    let carriers = [AUTHORIZATION.as_str(), "x-api-key", "x-goog-api-key"];
    let present: Vec<_> = carriers
        .into_iter()
        .filter(|name| headers.contains_key(*name))
        .collect();
    if present.len() != 1 {
        return Err(());
    }
    let name = present[0];
    if headers.get_all(name).iter().count() != 1 {
        return Err(());
    }
    let value = headers.remove(name).ok_or(())?;
    let value = value.to_str().map_err(|_| ())?;
    if name == AUTHORIZATION.as_str() {
        let (scheme, token) = value.split_once(' ').ok_or(())?;
        if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
            return Err(());
        }
        Ok(token.to_owned())
    } else if value.is_empty() {
        Err(())
    } else {
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_authorization_is_preserved_and_local_key_is_removed() {
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_KEY_HEADER, "lp_client_fixture.key".parse().unwrap());
        headers.insert(AUTHORIZATION, "Bearer native-token".parse().unwrap());
        headers.insert("chatgpt-account-id", "native-account".parse().unwrap());
        assert_eq!(take_key(&mut headers).unwrap(), "lp_client_fixture.key");
        assert!(!headers.contains_key(CLIENT_KEY_HEADER));
        assert_eq!(headers[AUTHORIZATION], "Bearer native-token");
        assert_eq!(headers["chatgpt-account-id"], "native-account");
    }
    #[test]
    fn bearer_credentials_are_consumed_and_ambiguous_auth_fails() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            "Bearer lp_client_fixture.key".parse().unwrap(),
        );
        assert_eq!(take_key(&mut headers).unwrap(), "lp_client_fixture.key");
        assert!(!headers.contains_key(AUTHORIZATION));
        headers.insert("x-api-key", "first".parse().unwrap());
        headers.insert("x-goog-api-key", "second".parse().unwrap());
        assert!(take_key(&mut headers).is_err());
        assert!(take_key(&mut HeaderMap::new()).is_err());
    }
}
