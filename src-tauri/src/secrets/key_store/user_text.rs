//! Versioned raw-byte cache encoding for user text. Metadata chooses this codec;
//! the appearance or byte length of input never selects a legacy key format.
use crate::secrets::SecretError;
use zeroize::Zeroizing;
const PREFIX: &[u8] = b"LoongPort.UserTextKey.v1\0";
pub(crate) fn encode(text: &str) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    if text.is_empty() {
        return Err(SecretError::UserTextRequired);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(PREFIX.len() + text.len()));
    bytes.extend_from_slice(PREFIX);
    bytes.extend_from_slice(text.as_bytes());
    Ok(bytes)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Zeroizing<String>, SecretError> {
    let bytes = bytes
        .strip_prefix(PREFIX)
        .ok_or(SecretError::UnsupportedFormat)?;
    let text = std::str::from_utf8(bytes).map_err(|_| SecretError::UnsupportedFormat)?;
    if text.is_empty() {
        return Err(SecretError::UserTextRequired);
    }
    Ok(Zeroizing::new(text.to_owned()))
}
/// The durable vault descriptor alone chooses raw legacy material versus versioned text.
pub(crate) fn authenticate(
    metadata: crate::secrets::VaultMetadata,
    bytes: Zeroizing<Vec<u8>>,
) -> Result<crate::secrets::VaultContext, SecretError> {
    metadata.validate()?;
    if metadata.user_text_key.is_some() {
        let text = decode(&bytes)?;
        crate::secrets::VaultContext::from_user_text(metadata, &text)
    } else {
        crate::secrets::VaultContext::from_key(metadata, bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_preserves_every_nonempty_utf8_byte_without_format_or_strength_rules() {
        for text in [
            "中文🔑",
            " abc ",
            "aBcD",
            "123",
            "x",
            " ",
            "\t\n",
            "AB".repeat(32).as_str(),
            "z".repeat(32).as_str(),
            "nul\0inside",
        ] {
            let bytes = encode(text).unwrap();
            assert_eq!(&bytes[PREFIX.len()..], text.as_bytes());
            assert_eq!(&*decode(&bytes).unwrap(), text);
        }
        assert!(encode("").is_err());
    }
    #[test]
    fn unversioned_raw_bytes_and_unknown_versions_are_not_current_text_cache() {
        for bytes in [
            vec![0; 32],
            vec![b'a'; 32],
            vec![b'a'; 64],
            b"LoongPort.UserTextKey.v2\0x".to_vec(),
            PREFIX.to_vec(),
            [PREFIX, &[255][..]].concat(),
        ] {
            assert!(decode(&bytes).is_err());
        }
    }
}
