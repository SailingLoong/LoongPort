use thiserror::Error;

/// Stable, secret-free errors at the credential protection boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum SecretError {
    #[error("Unsupported credential format")]
    UnsupportedFormat,
    #[error("Invalid credential envelope")]
    InvalidEnvelope,
    #[error("Invalid vault metadata")]
    InvalidMetadata,
    #[error("Credential identity does not match")]
    IdentityMismatch,
    #[error("Credential authentication failed")]
    AuthenticationFailed,
    #[error("Invalid data key length")]
    InvalidKeyLength,
    #[error("Data key does not unlock this vault")]
    KeyRejected,
    #[error("Recovery password does not unlock this vault")]
    PasswordRejected,
    #[error("A recovery password has not been configured")]
    PasswordUnavailable,
    #[error("Use at least 12 characters for the recovery password")]
    PasswordTooShort,
    #[error("Key derivation parameters exceed supported limits")]
    ResourceLimit,
    #[error("Key derivation failed")]
    KeyDerivationFailed,
    #[error("Secure random source is unavailable")]
    RandomUnavailable,
    #[error("Credential identity is required")]
    InvalidIdentity,
}

impl SecretError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "secret.unsupported_format",
            Self::InvalidEnvelope => "secret.invalid_envelope",
            Self::InvalidMetadata => "secret.invalid_metadata",
            Self::IdentityMismatch => "secret.identity_mismatch",
            Self::AuthenticationFailed => "secret.authentication_failed",
            Self::InvalidKeyLength => "secret.invalid_key_length",
            Self::KeyRejected => "secret.key_rejected",
            Self::PasswordRejected => "secret.password_rejected",
            Self::PasswordUnavailable => "secret.password_unavailable",
            Self::PasswordTooShort => "secret.password_too_short",
            Self::ResourceLimit => "secret.resource_limit",
            Self::KeyDerivationFailed => "secret.key_derivation_failed",
            Self::RandomUnavailable => "secret.random_unavailable",
            Self::InvalidIdentity => "secret.invalid_identity",
        }
    }
}

/// Public lifecycle errors expose stable codes, never paths or serialized data.
pub(crate) fn public_code(error: crate::error::AppError) -> String {
    match error {
        crate::error::AppError::Config(code)
            if code.starts_with("secret.") || code.starts_with("settings.") =>
        {
            code
        }
        crate::error::AppError::Json { .. } => "secret.invalid_file".into(),
        crate::error::AppError::Io { .. } | crate::error::AppError::IoContext { .. } => {
            "secret.storage_unavailable".into()
        }
        _ => "secret.operation_failed".into(),
    }
}
