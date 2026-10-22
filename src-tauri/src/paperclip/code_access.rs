//! Resolves authorized local access before preparing a private, temporary Codex home.
//! The protocol runner never discovers user credentials or personal configuration.
use super::{
    code_runtime::{CodeTaskRecord, CodeTaskRequest},
    codex_runner, Owner,
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command, time::Duration};

const CLIENT_ID: &str = "loongport-code-agents";
const KEY_ENV: &str = "LOONGPORT_CODE_TASK_KEY";

impl Owner {
    fn code_root(&self) -> PathBuf {
        crate::config::get_app_config_dir()
            .join("agent-code-tasks")
            .join(&self.owner)
    }
    pub(crate) fn code_access_options(&self) -> Result<Value, String> {
        let refs = self
            .state
            .db
            .list_client_credentials()
            .map_err(|_| "agents.access_unavailable")?;
        Ok(json!(refs
            .into_iter()
            .filter(|r| !r.revoked && r.client_id == CLIENT_ID && r.route_app == "codex")
            .collect::<Vec<_>>()))
    }
    /// Only the explicit authorize action may create a new grant.
    pub(crate) fn authorize_code_access(&self) -> Result<Value, String> {
        let db = &self.state.db;
        if !db
            .client_access_ready()
            .map_err(|_| "agents.access_unavailable")?
        {
            return Err("agents.access_unavailable".into());
        }
        let existing = db
            .list_client_credentials()
            .map_err(|_| "agents.access_unavailable")?
            .into_iter()
            .find(|reference| {
                !reference.revoked
                    && reference.client_id == CLIENT_ID
                    && reference.route_app == "codex"
            });
        let reference = match existing {
            Some(reference) => reference,
            None => db
                .issue_client_credential(CLIENT_ID, "codex", &[])
                .map_err(|_| "agents.access_unavailable")?,
        };
        serde_json::to_value(reference).map_err(|_| "agents.access_unavailable".into())
    }
    pub(crate) fn revoke_code_access(&self, credential_id: &str) -> Result<(), String> {
        let db = &self.state.db;
        db.resolve_client_credential(credential_id, CLIENT_ID, "codex")
            .map_err(|_| "agents.access_denied")?;
        db.revoke_client_credential(credential_id)
            .map_err(|_| "agents.access_unavailable".into())
    }
    pub(crate) fn code_models(&self) -> Result<Vec<String>, String> {
        let chain = crate::proxy::application_routing::chain_providers(&self.state.db, "codex")
            .map_err(|_| "agents.access_unavailable")?;
        let mut models = chain
            .iter()
            .flat_map(crate::relay::model_catalog::available_models)
            .collect::<Vec<_>>();
        models.sort();
        models.dedup();
        Ok(models)
    }

    pub(crate) fn create_code_task(
        &self,
        request: CodeTaskRequest,
    ) -> Result<CodeTaskRecord, String> {
        if request.route_app != "codex"
            || request
                .model
                .as_deref()
                .is_none_or(|model| model.trim().is_empty() || model.len() > 128)
        {
            return Err("agents.code_route_unsupported".into());
        }
        // Admission precedes any worktree side effect; recheck again at the execution boundary.
        self.code_key(&request.credential_id, request.model.as_deref())?;
        let owner = self.clone();
        let credential_id = request.credential_id.clone();
        self.control.code_tasks.start(
            &self.code_root(),
            request,
            move |cwd, input, model, cancel| {
                let key = owner.code_key(&credential_id, model)?;
                let runtime = owner.runtime.as_ref().ok_or("agents.runtime_unavailable")?;
                if !runtime.block_on(owner.state.proxy_service.is_running()) {
                    return Err("agents.gateway_not_running".into());
                }
                let (_, endpoint) =
                    runtime.block_on(owner.state.proxy_service.build_proxy_urls_existing())?;
                let url = url::Url::parse(&endpoint).map_err(|_| "agents.gateway_not_local")?;
                if url.scheme() != "http"
                    || !url.host_str().is_some_and(|host| {
                        host.trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                    })
                {
                    return Err("agents.gateway_not_local".into());
                }
                let home_parent = owner.code_root().join("runtime-homes");
                crate::config::ensure_private_directory(&home_parent)
                    .map_err(|_| "agents.runtime_home_unavailable")?;
                let home = tempfile::Builder::new()
                    .prefix("codex-")
                    .tempdir_in(home_parent)
                    .map_err(|_| "agents.runtime_home_unavailable")?;
                let config = runtime_config(&endpoint, model, home.path());
                let config =
                    toml::to_string(&config).map_err(|_| "agents.runtime_configuration_failed")?;
                crate::config::atomic_write_private(
                    &home.path().join("config.toml"),
                    config.as_bytes(),
                )
                .map_err(|_| "agents.runtime_configuration_failed")?;
                let executable = crate::commands::locate_default_tool(
                    "codex",
                    crate::process::CommandDeadline::from_timeout(Some(Duration::from_secs(5))),
                )?;
                let mut command = isolated_command(executable, home.path(), &endpoint, cwd);
                command.env(KEY_ENV, key);
                let result = codex_runner::run(
                    command,
                    cwd,
                    input,
                    model,
                    cancel,
                    Duration::from_secs(30 * 60),
                );
                if result.as_ref().is_ok_and(|result| !result.process_stopped) {
                    // Retain runtime state when process-tree cleanup cannot be confirmed.
                    let _ = home.keep();
                }
                result
            },
        )
    }
    fn code_key(&self, credential_id: &str, model: Option<&str>) -> Result<String, String> {
        let db = &self.state.db;
        let key = db
            .resolve_client_credential(credential_id, CLIENT_ID, "codex")
            .map_err(|_| "agents.access_denied")?;
        db.authenticate_client(&key, "codex", model)
            .map_err(|_| "agents.access_denied")?;
        let chain = crate::proxy::application_routing::chain_providers(db, "codex")
            .map_err(|_| "agents.code_route_unsupported")?;
        if chain.is_empty()
            || chain.iter().any(|provider| {
                provider.uses_managed_account_auth()
                    || crate::proxy::providers::is_codex_official_provider(provider)
            })
        {
            return Err("agents.code_route_unsupported".into());
        }
        Ok(key)
    }
    pub(crate) fn list_code_tasks(&self) -> Result<Vec<CodeTaskRecord>, String> {
        self.control.code_tasks.list(&self.code_root())
    }
    pub(crate) fn cancel_code_task(&self, id: &str) -> Result<Vec<CodeTaskRecord>, String> {
        self.control.code_tasks.cancel(id)?;
        self.list_code_tasks()
    }
}

fn runtime_config(endpoint: &str, model: Option<&str>, home: &std::path::Path) -> Value {
    let mut config = json!({
        "model_provider":"loongport-task", "approval_policy":"never", "sandbox_mode":"workspace-write",
        "cli_auth_credentials_store":"file",
        "model_providers":{"loongport-task":{"name":"LoongPort","base_url":endpoint,"wire_api":"responses","env_key":KEY_ENV,"requires_openai_auth":false}},
        "shell_environment_policy":{"inherit":"none","set":{"PATH":std::env::var("PATH").unwrap_or_default(),"HOME":home}},
        "mcp_servers":{}, "sandbox_workspace_write":{"network_access":false}
    });
    if let Some(model) = model {
        config["model"] = json!(model);
    }
    config
}

fn isolated_command(
    executable: PathBuf,
    home: &std::path::Path,
    endpoint: &str,
    cwd: &std::path::Path,
) -> Command {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut command = Command::new(executable);
    // CLI overrides take precedence over repository config layers, including untrusted MCP/plugin declarations.
    command.args(["app-server", "--stdio", "--strict-config", "-c", "mcp_servers={}", "-c", "plugins={}", "-c", "model_provider=\"loongport-task\"", "-c", "shell_environment_policy.inherit=\"none\""])
        .arg("-c").arg(format!("shell_environment_policy.set={{PATH={},HOME={}}}", toml::Value::String(path.clone()), toml::Value::String(home.to_string_lossy().into_owned())))
        .arg("-c").arg(format!("model_providers.loongport-task={{name=\"LoongPort\",base_url={},wire_api=\"responses\",env_key=\"{KEY_ENV}\",requires_openai_auth=false}}", toml::Value::String(endpoint.into())))
        .arg("-c").arg(format!("projects.{}.trust_level=\"untrusted\"", toml::Value::String(cwd.to_string_lossy().into_owned())))
        .env_clear().env("PATH", path).env("HOME", home).env("CODEX_HOME", home).env("XDG_CONFIG_HOME", home);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_do_not_issue_grants_and_revoked_grants_need_explicit_authorization() {
        let (_runtime, state, owner) = super::super::tests::fixture();
        state
            .db
            .set_setting("client_access_migration_version", "1")
            .unwrap();
        assert_eq!(owner.code_access_options().unwrap(), json!([]));
        assert!(state.db.list_client_credentials().unwrap().is_empty());
        let first = owner.authorize_code_access().unwrap();
        assert_eq!(owner.authorize_code_access().unwrap(), first);
        owner
            .revoke_code_access(first["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(owner.code_access_options().unwrap(), json!([]));
        assert!(state.db.list_client_credentials().unwrap()[0].revoked);
        let second = owner.authorize_code_access().unwrap();
        assert_ne!(first["id"], second["id"]);
    }
    #[test]
    fn runtime_environment_and_configuration_exclude_personal_credentials() {
        let home = tempfile::tempdir().unwrap();
        let command = isolated_command(
            PathBuf::from("codex"),
            home.path(),
            "http://127.0.0.1:1234/v1",
            home.path(),
        );
        let environment = command.get_envs().collect::<Vec<_>>();
        assert_eq!(environment.len(), 4);
        assert!(environment.iter().all(|(name, _)| [
            "HOME",
            "CODEX_HOME",
            "XDG_CONFIG_HOME",
            "PATH"
        ]
        .iter()
        .any(|allowed| *name == std::ffi::OsStr::new(allowed))));
        assert!(command.get_args().any(|value| value == "mcp_servers={}"));
        assert!(command.get_args().any(|value| value == "plugins={}"));
        let config = runtime_config(
            "http://127.0.0.1:1234/v1",
            Some("custom-model"),
            home.path(),
        );
        assert_eq!(config["shell_environment_policy"]["inherit"], "none");
        assert_eq!(
            config["model_providers"]["loongport-task"]["requires_openai_auth"],
            false
        );
        assert!(config["mcp_servers"].as_object().unwrap().is_empty());
    }
}
