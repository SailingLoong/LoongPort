//! Vault-only scope. This never admits native credential reads or writes.
use super::admission::BlockedReason;
use super::native::NativeCipher;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct LibraryContext {
    home: String,
    username: String,
    data_root: PathBuf,
    context_id: String,
}

impl LibraryContext {
    pub(super) fn from_os_identity(
        home: &str,
        username: &str,
        data_root: &Path,
    ) -> Result<Self, BlockedReason> {
        use base64::Engine;
        use sha2::Digest;
        if !Path::new(home).is_absolute()
            || username.is_empty()
            || home.contains('\0')
            || username.contains('\0')
            || !data_root.is_absolute()
            || data_root.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(BlockedReason::RootUnverified);
        }
        let identity =
            serde_json::to_vec(&("zcode-credential-v1", "darwin", home, username, data_root))
                .map_err(|_| BlockedReason::RootUnverified)?;
        Ok(Self {
            home: home.into(),
            username: username.into(),
            data_root: data_root.into(),
            context_id: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(identity)),
        })
    }
    pub(super) fn context_id(&self) -> &str {
        &self.context_id
    }
    pub(super) fn data_root(&self) -> &Path {
        &self.data_root
    }
    pub(super) fn cipher(&self) -> Result<NativeCipher, BlockedReason> {
        let secret = zeroize::Zeroizing::new(format!(
            "zcode-credential-fallback:darwin:{}:{}",
            self.home, self.username
        ));
        NativeCipher::new(&self.context_id, &secret).map_err(|_| BlockedReason::KeyContextUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_scope_matches_published_native_namespace_without_install_or_session() {
        use base64::Engine;
        use sha2::Digest;
        let home = crate::zcode_accounts::synthetic_test_path("home");
        let root = home.join(".zcode/v2");
        let home = home.to_str().unwrap();
        let ctx = LibraryContext::from_os_identity(home, "synthetic-user", &root).unwrap();
        let expected = serde_json::to_vec(&(
            "zcode-credential-v1",
            "darwin",
            home,
            "synthetic-user",
            &root,
        ))
        .unwrap();
        assert_eq!(
            ctx.context_id(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(expected))
        );
        let existing = NativeCipher::new(
            ctx.context_id(),
            &format!("zcode-credential-fallback:darwin:{home}:synthetic-user"),
        )
        .unwrap();
        let ciphertext = existing.encrypt("synthetic session").unwrap();
        assert_eq!(
            ctx.cipher().unwrap().decrypt(&ciphertext).unwrap().as_str(),
            "synthetic session"
        );
        assert_eq!(ctx.data_root(), root);
    }
    #[test]
    fn library_scope_separates_os_user_and_data_root_and_rejects_relative_input() {
        let home = crate::zcode_accounts::synthetic_test_path("home");
        let root = home.join(".zcode/v2");
        let home = home.to_str().unwrap();
        let a = LibraryContext::from_os_identity(home, "alice", &root).unwrap();
        let b = LibraryContext::from_os_identity(home, "bob", &root).unwrap();
        let c = LibraryContext::from_os_identity(home, "alice", &root.join("other")).unwrap();
        assert_ne!(a.context_id(), b.context_id());
        assert_ne!(a.context_id(), c.context_id());
        for bad in [PathBuf::from("relative"), root.join("../other")] {
            assert!(LibraryContext::from_os_identity(home, "alice", &bad).is_err());
        }
        assert!(LibraryContext::from_os_identity(home, "", &root).is_err());
    }
}
