//! Original account credentials. Opaque native retention never grants API or source authenticity.
use super::{Account, Domain, Failure};
use crate::workbuddy::native::{self, NativeContext};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

// Not convertible to native::NativeSnapshot: storage validity is not capture authority.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredNativeSnapshot {
    context: NativeContext,
    identity: String,
    bytes: Zeroizing<Vec<u8>>,
    // LoongPort row lifetime only; never source authentication or rollback proof.
    incarnation: Option<uuid::Uuid>,
}
impl StoredNativeSnapshot {
    fn context(&self) -> &NativeContext {
        &self.context
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Credential {
    Api(String),
    NativeOnly(StoredNativeSnapshot),
    ApiWithNative {
        token: String,
        snapshot: StoredNativeSnapshot,
    },
}
impl Credential {
    /// Non-secret local row version, never source authenticity or admission.
    pub(in crate::workbuddy) fn native_incarnation(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Api(_) => None,
            Self::NativeOnly(snapshot) | Self::ApiWithNative { snapshot, .. } => {
                snapshot.incarnation
            }
        }
    }
}
impl From<String> for Credential {
    fn from(value: String) -> Self {
        Self::Api(value)
    }
}
impl From<&str> for Credential {
    fn from(value: &str) -> Self {
        Self::Api(value.into())
    }
}
impl Drop for Credential {
    fn drop(&mut self) {
        match self {
            Self::Api(token) | Self::ApiWithNative { token, .. } => token.zeroize(),
            Self::NativeOnly(_) => {}
        }
    }
}
impl Account {
    pub(in crate::workbuddy) fn from_observation(
        bytes: &[u8],
        context: &NativeContext,
        label: &str,
    ) -> Result<Self, Failure> {
        Self::observed_with_lifetime(bytes, context, label, uuid::Uuid::new_v4())
    }
    pub(in crate::workbuddy) fn recapture_observation(
        &self,
        bytes: &[u8],
        context: &NativeContext,
        label: &str,
    ) -> Result<Self, Failure> {
        let Credential::NativeOnly(snapshot) = &self.credential else {
            return Err(Failure::UnsupportedContext);
        };
        if snapshot.context() != context {
            return Err(Failure::UnsupportedContext);
        }
        let incarnation = snapshot.incarnation.ok_or(Failure::UnsupportedContext)?;
        let next = Self::observed_with_lifetime(bytes, context, label, incarnation)?;
        if next.uid != self.uid {
            return Err(Failure::UnsupportedContext);
        }
        Ok(next)
    }
    fn observed_with_lifetime(
        bytes: &[u8],
        context: &NativeContext,
        label: &str,
        incarnation: uuid::Uuid,
    ) -> Result<Self, Failure> {
        let identity = native::validate_stored_identity(bytes, context)
            .map_err(|_| Failure::UnsupportedContext)?;
        Ok(Self {
            uid: identity.clone(),
            domain: Domain::parse(&context.realm)?,
            label: label.into(),
            credential: Credential::NativeOnly(StoredNativeSnapshot {
                context: context.clone(),
                identity,
                bytes: Zeroizing::new(bytes.to_vec()),
                incarnation: Some(incarnation),
            }),
        })
    }
    pub(in crate::workbuddy) fn has_native_lifetime(&self) -> bool {
        self.native_snapshot()
            .is_some_and(|snapshot| snapshot.incarnation.is_some())
    }
    pub(in crate::workbuddy) fn same_native_scope(&self, context: &NativeContext) -> bool {
        matches!(&self.credential, Credential::NativeOnly(snapshot) if snapshot.context()==context)
    }
    pub(in crate::workbuddy) fn native_for_controller(
        &self,
        permit: &crate::workbuddy::native_controller::ReadPermit,
    ) -> Result<native::NativeSnapshot, Failure> {
        let snapshot = self.native_snapshot().ok_or(Failure::UnsupportedContext)?;
        if snapshot.incarnation.is_none() || !permit.allows(snapshot.context()) {
            return Err(Failure::UnsupportedContext);
        }
        native::capture(snapshot.bytes(), snapshot.context())
            .map_err(|_| Failure::UnsupportedContext)
    }
    pub(crate) fn is_api(&self) -> bool {
        matches!(
            self.credential,
            Credential::Api(_) | Credential::ApiWithNative { .. }
        )
    }
    pub(crate) fn api_token(&self) -> Result<&str, Failure> {
        match &self.credential {
            Credential::Api(token) | Credential::ApiWithNative { token, .. } => Ok(token),
            Credential::NativeOnly(_) => Err(Failure::UnsupportedContext),
        }
    }
    pub(crate) fn clear_api_token(&mut self) -> Result<(), Failure> {
        match &mut self.credential {
            Credential::Api(token) | Credential::ApiWithNative { token, .. } => {
                token.zeroize();
                Ok(())
            }
            Credential::NativeOnly(_) => Err(Failure::UnsupportedContext),
        }
    }
    fn native_snapshot(&self) -> Option<&StoredNativeSnapshot> {
        match &self.credential {
            Credential::NativeOnly(snapshot) | Credential::ApiWithNative { snapshot, .. } => {
                Some(snapshot)
            }
            Credential::Api(_) => None,
        }
    }
    pub(super) fn validate_credential(&self) -> Result<(), Failure> {
        if let Some(snapshot) = self.native_snapshot() {
            if snapshot.identity() != self.uid || snapshot.context().realm != self.domain.host() {
                return Err(Failure::StorageUnavailable);
            }
        }
        Ok(())
    }
    /// Original account reauthorization keeps the already-held snapshot, keyed by
    /// exact realm+UID. Retention does not authenticate its opaque contents.
    pub(in crate::workbuddy) fn replace_api_preserving_native(
        &mut self,
        mut replacement: Account,
    ) -> Result<(), Failure> {
        if self.uid != replacement.uid || self.domain != replacement.domain {
            return Err(Failure::UnsupportedContext);
        }
        let Credential::Api(token) = &mut replacement.credential else {
            return Err(Failure::UnsupportedContext);
        };
        if let Some(snapshot) = self.native_snapshot() {
            replacement.credential = Credential::ApiWithNative {
                token: std::mem::take(token),
                snapshot: snapshot.clone(),
            };
        }
        *self = replacement;
        Ok(())
    }
    /// Keep the legacy API digest exact; include retained native state in the
    /// existing HTTP owner fingerprint so late writes cannot erase a newer image.
    pub(in crate::workbuddy) fn http_fingerprint(&self) -> Result<[u8; 32], Failure> {
        let token = self.api_token()?;
        let Some(snapshot) = self.native_snapshot() else {
            return Ok(Sha256::digest(token.as_bytes()).into());
        };
        Ok(fingerprint_with_native(token, snapshot))
    }
}
pub(super) fn fingerprint_with_native(token: &str, snapshot: &StoredNativeSnapshot) -> [u8; 32] {
    let mut hash = Sha256::new();
    for value in [
        b"workbuddy:api-native:v1".as_slice(),
        token.as_bytes(),
        snapshot.context().product.as_bytes(),
        snapshot.context().region.as_bytes(),
        snapshot.context().realm.as_bytes(),
        snapshot.context().source.as_bytes(),
        snapshot.bytes(),
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    if let Some(incarnation) = snapshot.incarnation {
        for value in [
            b"native-incarnation:v2".as_slice(),
            incarnation.as_bytes().as_slice(),
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value);
        }
    }
    hash.finalize().into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAccount {
    uid: String,
    domain: Domain,
    label: String,
    #[serde(default, deserialize_with = "present")]
    token: Option<String>,
    #[serde(default, deserialize_with = "present")]
    native: Option<WireNative>,
    #[serde(default, deserialize_with = "present")]
    credential_version: Option<u32>,
}
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
impl Drop for WireAccount {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireNative {
    version: u32,
    context: NativeContext,
    snapshot: String,
    #[serde(default, deserialize_with = "present")]
    incarnation: Option<String>,
}
impl Drop for WireNative {
    fn drop(&mut self) {
        self.snapshot.zeroize();
    }
}
fn decode_native<E: serde::de::Error>(
    record: &WireNative,
    uid: &str,
    domain: Domain,
) -> Result<StoredNativeSnapshot, E> {
    let incarnation = match (record.version, record.incarnation.as_deref()) {
        (1, None) => None,
        (2, Some(value)) => {
            let id =
                uuid::Uuid::parse_str(value).map_err(|_| E::custom("invalid native lifetime"))?;
            if id.get_version_num() != 4
                || id.get_variant() != uuid::Variant::RFC4122
                || id.hyphenated().to_string() != value
            {
                return Err(E::custom("invalid native lifetime"));
            }
            Some(id)
        }
        _ => return Err(E::custom("unsupported native credential lifetime version")),
    };
    let bytes = Zeroizing::new(
        STANDARD
            .decode(&record.snapshot)
            .map_err(|_| E::custom("invalid native snapshot"))?,
    );
    if STANDARD.encode(bytes.as_slice()) != record.snapshot {
        return Err(E::custom("invalid native snapshot encoding"));
    }
    let identity = native::validate_stored_identity(&bytes, &record.context)
        .map_err(|_| E::custom("invalid native snapshot context"))?;
    let snapshot = StoredNativeSnapshot {
        context: record.context.clone(),
        identity,
        bytes,
        incarnation,
    };
    if snapshot.identity() != uid || snapshot.context().realm != domain.host() {
        return Err(E::custom("native identity mismatch"));
    }
    Ok(snapshot)
}
impl<'de> Deserialize<'de> for Account {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut wire = WireAccount::deserialize(d)?;
        let token = wire.token.take().map(Zeroizing::new);
        let credential = match (token, wire.native.as_ref(), wire.credential_version) {
            (Some(mut token), None, None) => Credential::Api(std::mem::take(&mut *token)),
            (None, Some(record), None) => {
                Credential::NativeOnly(decode_native::<D::Error>(record, &wire.uid, wire.domain)?)
            }
            (Some(mut token), Some(record), Some(1)) => {
                let snapshot = decode_native::<D::Error>(record, &wire.uid, wire.domain)?;
                Credential::ApiWithNative {
                    token: std::mem::take(&mut *token),
                    snapshot,
                }
            }
            _ => {
                return Err(D::Error::custom(
                    "missing, ambiguous or unsupported account credential",
                ))
            }
        };
        Ok(Self {
            uid: wire.uid.clone(),
            domain: wire.domain,
            label: wire.label.clone(),
            credential,
        })
    }
}
impl Serialize for Account {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = s.serialize_struct(
            "Account",
            if matches!(self.credential, Credential::ApiWithNative { .. }) {
                6
            } else {
                4
            },
        )?;
        state.serialize_field("uid", &self.uid)?;
        state.serialize_field("domain", &self.domain)?;
        state.serialize_field("label", &self.label)?;
        if let Ok(token) = self.api_token() {
            state.serialize_field("token", token)?;
        }
        if let Some(snapshot) = self.native_snapshot() {
            #[derive(Serialize)]
            struct Record<'a> {
                version: u32,
                context: &'a NativeContext,
                snapshot: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                incarnation: Option<String>,
            }
            let encoded = Zeroizing::new(STANDARD.encode(snapshot.bytes()));
            state.serialize_field(
                "native",
                &Record {
                    version: if snapshot.incarnation.is_some() { 2 } else { 1 },
                    context: snapshot.context(),
                    snapshot: &encoded,
                    incarnation: snapshot.incarnation.map(|id| id.hyphenated().to_string()),
                },
            )?;
        }
        if matches!(self.credential, Credential::ApiWithNative { .. }) {
            state.serialize_field("credential_version", &1u32)?;
        }
        state.end()
    }
}
