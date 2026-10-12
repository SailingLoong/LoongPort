//! Bound-byte proof exercises the original writer's semantics, without invoking it.
use super::*;
use crate::secrets::{testing::TestHome, VaultContext};
use serde_json::json;

fn row() -> Provider {
    Provider::with_id(
        "synthetic".into(),
        "Synthetic".into(),
        json!({
            "auth":{"OPENAI_API_KEY":"synthetic-key"},
            "config":"model = 'synthetic-model'\nmodel_provider = 'original'\n[model_providers.original]\nname = 'Synthetic'\nbase_url = 'https://synthetic.example.invalid/v1'\nwire_api = 'responses'\n"
        }),
        None,
    )
}
fn config() -> String {
    "model = 'synthetic-model'\nmodel_provider = 'custom'\n[model_providers.custom]\nname = 'Synthetic'\nbase_url = 'https://synthetic.example.invalid/v1'\nwire_api = 'responses'\nexperimental_bearer_token = 'synthetic-key'\n".into()
}
fn login(account: &str) -> Value {
    json!({"auth_mode":"chatgpt", "OPENAI_API_KEY":null, "tokens":{
        "id_token":"synthetic-id", "access_token":format!("synthetic-access-{account}"),
        "refresh_token":format!("synthetic-refresh-{account}"), "account_id":account}})
}
fn pre(config: &str, auth: Option<&Value>) -> Vec<Option<Vec<u8>>> {
    vec![
        auth.map(|v| serde_json::to_vec(v).unwrap()),
        Some(config.as_bytes().to_vec()),
        None,
        None,
        None,
    ]
}
fn proof(
    row: &Provider,
    settings: &crate::settings::AppSettings,
    vault: &VaultContext,
    pre: Vec<Option<Vec<u8>>>,
) -> Option<bool> {
    native_completion_match(
        row,
        &[(row.id.clone(), row.clone())].into_iter().collect(),
        settings,
        vault,
        pre,
        None,
        Some(&("127.0.0.1".into(), 15721)),
    )
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bound_codex_proof_checks_route_auth_and_catalog_without_discovery() {
    let home = TestHome::new().unwrap();
    let vault = VaultContext::generate().unwrap();
    let row = row();
    let settings = crate::settings::AppSettings::default();
    let config = config();
    // Deliberately invalid external content: proof must not import or parse it.
    let external = home.path().join("external.json");
    std::fs::write(&external, b"not a catalog").unwrap();
    let before = std::fs::read(&external).unwrap();
    assert_eq!(
        proof(&row, &settings, &vault, pre(&config, None)),
        Some(true)
    );
    for changed in [
        config.replace("synthetic-model", "wrong-model"),
        config.replace("synthetic-key", "wrong-key"),
        config.replace("synthetic.example.invalid", "other.example.invalid"),
        config.replace("model_provider = 'custom'", "model_provider = 'original'"),
        format!("openai_base_url = 'http://127.0.0.1:15721/v1'\n{config}"),
        format!("{config}requires_openai_auth = true\n"),
        format!("{config}[model_providers.original]\nbase_url = 'https://synthetic.example.invalid/v1'\n"),
        format!("profile = 'selected'\n{config}[profiles.selected]\nmodel_provider = 'other'\n"),
    ] {
        assert_ne!(proof(&row, &settings, &vault, pre(&changed, None)), Some(true), "{changed}");
    }
    let native = login("synthetic-native");
    assert_eq!(
        proof(&row, &settings, &vault, pre(&config, Some(&native))),
        Some(true)
    );
    // Proven old third-party residue is not the user's independently retained login.
    assert_eq!(
        proof(&row, &settings, &vault, pre(&config, Some(&row_auth(&row)))),
        Some(false)
    );
    let unrelated =
        format!("{config}[ui]\ntheme = 'keep'\n[mcp_servers.local]\ncommand = 'synthetic'\n");
    assert_eq!(
        proof(&row, &settings, &vault, pre(&unrelated, None)),
        Some(true)
    );
    let pointer = format!(
        "model_catalog_json = {}\n",
        serde_json::to_string(&external.to_string_lossy()).unwrap()
    );
    assert_eq!(
        proof(
            &row,
            &settings,
            &vault,
            pre(&format!("{pointer}{config}"), None)
        ),
        Some(true)
    );
    let mut claimed = row.clone();
    claimed.settings_config["config"] = format!(
        "{pointer}{}",
        row.settings_config["config"].as_str().unwrap()
    )
    .into();
    assert_eq!(
        proof(
            &claimed,
            &settings,
            &vault,
            pre(&format!("{pointer}{config}"), None)
        ),
        Some(true)
    );
    assert_eq!(
        proof(&claimed, &settings, &vault, pre(&config, None)),
        Some(false)
    );
    for catalog in [None, Some(b"{}".to_vec()), Some(b"not-json".to_vec())] {
        let mut input = pre(
            &format!(
                "model_catalog_json = '{}'\n{config}",
                crate::live::project::codex::CATALOG_FILENAME
            ),
            None,
        );
        input[2] = catalog;
        assert_ne!(proof(&row, &settings, &vault, input), Some(true));
    }
    let written: state::Written = serde_json::from_value(json!({"codex":{"version":1,"auth":{"account_id":"synthetic-managed","last_refresh_ms":1,"digest":"0".repeat(64)}}})).unwrap();
    let rows = [(row.id.clone(), row.clone())].into_iter().collect();
    assert_eq!(
        native_completion_match(
            &row,
            &rows,
            &settings,
            &vault,
            pre(&config, None),
            Some(&written),
            None
        ),
        None
    );
    let mut managed = row.clone();
    managed.meta = Some(serde_json::from_value(json!({"authBinding":{"source":"managed_account","authProvider":"codex_oauth","accountId":"synthetic-managed"}})).unwrap());
    assert_eq!(
        managed_account(&managed).as_deref(),
        Some("synthetic-managed")
    );
    assert_eq!(proof(&managed, &settings, &vault, pre(&config, None)), None);
    for name in [
        crate::live::project::codex::CATALOG_FILENAME,
        "cc-switch-model-catalog.json",
    ] {
        for pointer in [name.to_string(), format!("  link/{name}  ")] {
            let mut managed_name = row.clone();
            managed_name.settings_config["config"] = format!(
                "model_catalog_json = '{pointer}'\n{}",
                row.settings_config["config"].as_str().unwrap()
            )
            .into();
            assert_eq!(
                proof(&managed_name, &settings, &vault, pre(&config, None)),
                None
            );
            assert_eq!(
                proof(
                    &row,
                    &settings,
                    &vault,
                    pre(&format!("model_catalog_json = '{pointer}'\n{config}"), None)
                ),
                None
            );
        }
    }
    let mut generated = row.clone();
    generated.settings_config["modelCatalog"] = json!({"models":[{"model":"synthetic-model"}]});
    assert_eq!(
        proof(&generated, &settings, &vault, pre(&config, None)),
        None
    );
    for store in ["keyring", "auto", "ephemeral", "future"] {
        assert_eq!(
            proof(
                &row,
                &settings,
                &vault,
                pre(
                    &format!("cli_auth_credentials_store = '{store}'\n{config}"),
                    None
                )
            ),
            None
        );
    }
    for auth in [b"null".as_slice(), b"[]", b"{\"x\":1,\"x\":2}", b"not-json"] {
        let mut input = pre(&config, None);
        input[0] = Some(auth.to_vec());
        assert_eq!(proof(&row, &settings, &vault, input), None);
    }
    for (index, bytes) in [
        (2, b"not-json".as_slice()),
        (3, br#"{"version":2,"account_id":"synthetic"}"#),
        (4, b"not-encrypted"),
    ] {
        let mut input = pre(&config, None);
        input[index] = Some(bytes.to_vec());
        assert_eq!(proof(&row, &settings, &vault, input), None);
    }
    let mut unknown_stash = pre(&config, None);
    unknown_stash[4] = Some(
        DeviceFile::registered(STASH_FILENAME)
            .unwrap()
            .encode(&vault, br#"{"future":true}"#)
            .unwrap(),
    );
    assert_eq!(proof(&row, &settings, &vault, unknown_stash), None);
    assert_eq!(std::fs::read(&external).unwrap(), before);
    assert!(!get_codex_config_path().exists());
    assert!(!get_codex_model_catalog_path().exists());
    assert!(!DeviceStore::for_device()
        .path_for(&DeviceFile::registered(STASH_FILENAME).unwrap())
        .exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bound_codex_proof_uses_original_auth_placement_and_official_login_semantics() {
    let _home = TestHome::new().unwrap();
    let vault = VaultContext::generate().unwrap();
    let row = row();
    let settings = crate::settings::AppSettings {
        preserve_codex_official_auth_on_switch: false,
        ..Default::default()
    };
    let config = config().replace(
        "experimental_bearer_token = 'synthetic-key'",
        "requires_openai_auth = true",
    );
    assert_eq!(
        proof(&row, &settings, &vault, pre(&config, Some(&row_auth(&row)))),
        Some(true)
    );
    for auth in [None, Some(json!({})), Some(login("synthetic-native"))] {
        assert_eq!(
            proof(&row, &settings, &vault, pre(&config, auth.as_ref())),
            Some(false)
        );
    }
    assert_eq!(
        proof(
            &row,
            &settings,
            &vault,
            pre(
                &format!("{config}experimental_bearer_token = 'synthetic-key'\n"),
                Some(&row_auth(&row))
            )
        ),
        Some(false)
    );
    let mut official = Provider::with_id(
        "official".into(),
        "Synthetic official".into(),
        json!({"auth":{}, "config":"model = 'synthetic-model'\n"}),
        None,
    );
    official.category = Some("official".into());
    for unify in [false, true] {
        let settings = crate::settings::AppSettings {
            unify_codex_session_history: unify,
            ..Default::default()
        };
        let config = if unify {
            "model = 'synthetic-model'\nmodel_provider = 'custom'\n[model_providers.custom]\nname = 'OpenAI'\nrequires_openai_auth = true\nsupports_websockets = true\nwire_api = 'responses'\n"
        } else {
            "model = 'synthetic-model'\n"
        };
        for auth in [None, Some(login("synthetic-live"))] {
            assert_eq!(
                proof(&official, &settings, &vault, pre(config, auth.as_ref())),
                Some(true)
            );
        }
        let opposite = crate::settings::AppSettings {
            unify_codex_session_history: !unify,
            ..settings.clone()
        };
        assert_eq!(
            proof(&official, &opposite, &vault, pre(config, None)),
            Some(false)
        );
        assert_ne!(
            proof(
                &official,
                &settings,
                &vault,
                pre(
                    &format!("openai_base_url = 'http://127.0.0.1:15721/v1'\n{config}"),
                    None
                )
            ),
            Some(true)
        );
        let a = login("synthetic-a");
        let b = login("synthetic-b");
        let mut named = official.clone();
        named.settings_config["auth"] = a.clone();
        // Missing stash is seeded from the row by the existing login owner.
        assert_eq!(
            proof(&named, &settings, &vault, pre(config, Some(&a))),
            Some(true)
        );
        assert_eq!(
            proof(&named, &settings, &vault, pre(config, Some(&b))),
            Some(false)
        );
        // An existing empty stash invalidates the old row snapshot. Original
        // owner intentionally follows current native login instead of replaying it.
        let mut input = pre(config, Some(&b));
        input[4] = Some(
            DeviceFile::registered(STASH_FILENAME)
                .unwrap()
                .encode(&vault, &serde_json::to_vec(&LoginStash::default()).unwrap())
                .unwrap(),
        );
        assert_eq!(proof(&named, &settings, &vault, input), Some(true));
        let mut rotated = a.clone();
        rotated["tokens"]["access_token"] = "synthetic-rotated".into();
        assert_eq!(
            proof(&named, &settings, &vault, pre(config, Some(&rotated))),
            Some(true)
        );
    }
}

#[cfg(unix)]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bound_codex_proof_defers_unbound_managed_name_path_ownership() {
    let home = TestHome::new().unwrap();
    let vault = VaultContext::generate().unwrap();
    let settings = crate::settings::AppSettings::default();
    let mut row = row();
    let name = crate::live::project::codex::CATALOG_FILENAME;
    row.settings_config["config"] = format!(
        "model_catalog_json = 'link/{name}'\n{}",
        row.settings_config["config"].as_str().unwrap()
    )
    .into();
    let base = get_codex_config_dir();
    let inside = base.join("inside");
    let outside = home.path().join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(inside.join(name), b"{}").unwrap();
    std::fs::write(outside.join(name), b"{}").unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&inside, &link).unwrap();
    let bound = pre(&config(), None);
    let first = proof(&row, &settings, &vault, bound.clone());
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let second = proof(&row, &settings, &vault, bound);
    println!("managed-name path proof with identical bound inputs: {first:?} / {second:?}");
    assert_eq!(first, None);
    assert_eq!(second, None);
}
