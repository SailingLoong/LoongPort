//! Synthetic metadata only; no account package or password is read.
#[path = "../../src-tauri/src/zcode_accounts/bundle_limits.rs"]
mod bundle_limits;

#[cfg(test)]
mod tests {
    use super::bundle_limits::*;

    fn supported() -> EnvelopeParameters<'static> {
        EnvelopeParameters {
            format: "zsw-accounts-bundle",
            version: 1,
            kdf: "pbkdf2-hmac-sha256",
            iterations: 100_000,
            salt_bytes: 16,
            cipher: "aes-256-gcm",
            nonce_bytes: 12,
            tag_bytes: 16,
            ciphertext_bytes: 128,
        }
    }

    #[test]
    fn supported_fixed_parameters_are_accepted() {
        assert_eq!(validate_outer(1024, &supported()), Ok(()));
        assert_eq!(validate_inner("zcode-accounts-bundle", 2, 50, 1024), Ok(()));
    }

    #[test]
    fn unknown_formats_and_versions_are_rejected() {
        let mut p = supported();
        p.format = "another-format";
        assert_eq!(
            validate_outer(1024, &p),
            Err(BundleError::UnsupportedFormat)
        );
        p = supported();
        p.version = 2;
        assert_eq!(
            validate_outer(1024, &p),
            Err(BundleError::UnsupportedVersion)
        );
        assert_eq!(
            validate_inner("zcode-accounts-bundle", 1, 1, 1024),
            Err(BundleError::UnsupportedVersion)
        );
        assert_eq!(
            validate_inner("another-format", 2, 1, 1024),
            Err(BundleError::UnsupportedFormat)
        );
    }

    #[test]
    fn attacker_selected_kdf_work_is_never_accepted() {
        for iterations in [0, 1, 99_999, 100_001, u32::MAX] {
            let mut p = supported();
            p.iterations = iterations;
            assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidKdf));
        }
        let mut p = supported();
        p.kdf = "pbkdf2-hmac-sha512";
        assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidKdf));
        for salt_bytes in [0, 15, 17, usize::MAX] {
            p = supported();
            p.salt_bytes = salt_bytes;
            assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidKdf));
        }
    }

    #[test]
    fn cipher_and_authenticated_envelope_lengths_are_fixed() {
        let mut p = supported();
        p.cipher = "aes-256-cbc";
        assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidCipher));
        for n in [0, 11, 13, usize::MAX] {
            p = supported();
            p.nonce_bytes = n;
            assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidCipher));
        }
        for n in [0, 15, 17, usize::MAX] {
            p = supported();
            p.tag_bytes = n;
            assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidCipher));
        }
        p = supported();
        p.ciphertext_bytes = 0;
        assert_eq!(validate_outer(1024, &p), Err(BundleError::InvalidCipher));
    }

    #[test]
    fn bounded_package_is_checked_before_crypto_work() {
        for size in [0, MAX_BUNDLE_BYTES + 1, usize::MAX] {
            assert_eq!(
                validate_outer(size, &supported()),
                Err(BundleError::ResourceLimit)
            );
        }
        let mut p = supported();
        p.ciphertext_bytes = MAX_BUNDLE_BYTES + 1;
        assert_eq!(validate_outer(1024, &p), Err(BundleError::ResourceLimit));
    }

    #[test]
    fn inner_account_count_and_plaintext_are_bounded() {
        for accounts in [0, 51, usize::MAX] {
            assert_eq!(
                validate_inner("zcode-accounts-bundle", 2, accounts, 1024),
                Err(BundleError::ResourceLimit)
            );
        }
        for bytes in [0, MAX_BUNDLE_BYTES + 1, usize::MAX] {
            assert_eq!(
                validate_inner("zcode-accounts-bundle", 2, 1, bytes),
                Err(BundleError::ResourceLimit)
            );
        }
    }
}
