use super::bundle::{open_bundle, BundleFailure};
use super::checkpoint::ProfileCatalog;
use super::core::{AccountSnapshot, CredentialDocument, OAuthFamily};
use super::native::NativeCipher;
use std::collections::BTreeMap;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewRow {
    pub index: usize,
    pub id: Option<String>,
    pub label: Option<String>,
    pub family: Option<&'static str>,
    pub duplicate: bool,
    pub ambiguous: bool,
    pub error: Option<&'static str>,
}
pub(super) struct InspectedBundle {
    pub rows: Vec<PreviewRow>,
    pub accounts: Vec<Option<AccountSnapshot>>,
}
pub(super) fn inspect(
    file: &[u8],
    password: &str,
    native: &NativeCipher,
    catalog: &ProfileCatalog,
) -> Result<InspectedBundle, BundleFailure> {
    let opened = open_bundle(file, password)?;
    let mut rows = Vec::new();
    let mut accounts = Vec::new();
    let mut seen = BTreeMap::<String, usize>::new();
    for (index, entry) in opened.entries()?.into_iter().enumerate() {
        let mut row = PreviewRow {
            index,
            id: None,
            label: None,
            family: None,
            duplicate: false,
            ambiguous: false,
            error: None,
        };
        let snapshot = entry
            .map_err(|_| "invalidEntry")
            .and_then(|entry| {
                let document = CredentialDocument::parse(entry.credentials.get().as_bytes())
                    .map_err(|_| "invalidEntry")?;
                native
                    .inspect(&document)
                    .map_err(|_| "incompatibleCredentials")
            })
            .and_then(|snapshot| {
                row.id = Some(snapshot.identity().opaque_id());
                row.family = Some(match snapshot.identity().family() {
                    OAuthFamily::Zai => "zai",
                    OAuthFamily::BigModel => "bigmodel",
                });
                row.duplicate = catalog.get(snapshot.identity()).is_some();
                row.label = native
                    .profile_label(&snapshot)
                    .map_err(|_| "incompatibleCredentials")?
                    .and_then(|label| label.chars().next().map(|first| format!("{first}…")));
                Ok(snapshot)
            });
        match snapshot {
            Ok(snapshot) => {
                *seen.entry(snapshot.identity().opaque_id()).or_default() += 1;
                accounts.push(Some(snapshot));
            }
            Err(reason) => {
                // Clear partial metadata on any error; renderer cannot select it.
                row.id = None;
                row.label = None;
                row.family = None;
                row.duplicate = false;
                row.error = Some(reason);
                accounts.push(None);
            }
        }
        rows.push(row);
    }
    for row in &mut rows {
        row.ambiguous = row
            .id
            .as_ref()
            .is_some_and(|id| seen.get(id).is_some_and(|count| *count > 1));
    }
    Ok(InspectedBundle { rows, accounts })
}
#[cfg(test)]
#[path = "bundle_import_tests.rs"]
mod tests;
