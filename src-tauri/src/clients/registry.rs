//! Adding a downstream client adds a descriptor and adapter, never a provider app.
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub route_apps: &'static [&'static str],
}

pub struct Registration {
    pub descriptor: ClientDescriptor,
    directory: &'static str,
    pub adapter: &'static dyn Adapter,
    override_env: Option<&'static str>,
}

pub static CLIENTS: &[Registration] = &[
    Registration {
        descriptor: ClientDescriptor {
            id: "workbuddy",
            name: "WorkBuddy",
            route_apps: &["codex"],
        },
        directory: ".workbuddy",
        adapter: &super::workbuddy::WorkBuddy,
        override_env: Some("WORKBUDDY_CONFIG_DIR"),
    },
    Registration {
        descriptor: ClientDescriptor {
            id: "workbuddy-ai",
            name: "WorkBuddy AI",
            route_apps: &["codex"],
        },
        directory: ".workbuddy-ai",
        adapter: &super::workbuddy::WorkBuddy,
        override_env: None,
    },
];

pub trait Adapter: Sync {
    fn project(
        &self,
        current: Option<&[u8]>,
        entries: &[serde_json::Value],
    ) -> Result<Vec<u8>, String>;
    fn restore(
        &self,
        current: &[u8],
        original: Option<&[u8]>,
        installed: &[serde_json::Value],
    ) -> Result<Vec<u8>, String>;
    fn entries(&self, current: Option<&[u8]>) -> Result<Vec<serde_json::Value>, String>;
    fn projection(&self, endpoint: &str, secret: &str, models: &[String])
        -> Vec<serde_json::Value>;
}

impl Registration {
    pub fn local_path(&self) -> PathBuf {
        let override_dir = self
            .override_env
            .and_then(std::env::var_os)
            .map(PathBuf::from);
        self.path(&crate::config::get_home_dir(), override_dir.as_deref())
    }

    pub fn path(&self, home: &Path, override_dir: Option<&Path>) -> PathBuf {
        if self.override_env.is_some() {
            if let Some(path) = override_dir {
                return path.join("models.json");
            }
        }
        home.join(self.directory).join("models.json")
    }
}

pub fn get(id: &str) -> Result<&'static Registration, crate::error::AppError> {
    CLIENTS
        .iter()
        .find(|entry| entry.descriptor.id == id)
        .ok_or_else(|| crate::error::AppError::Config("clients.unknown_client".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editions_resolve_separately_even_with_domestic_override() {
        let home = Path::new("/fixture");
        let custom = Path::new("/custom");
        assert_eq!(
            get("workbuddy").unwrap().path(home, None),
            home.join(".workbuddy/models.json")
        );
        assert_eq!(
            get("workbuddy").unwrap().path(home, Some(custom)),
            custom.join("models.json")
        );
        assert_eq!(
            get("workbuddy-ai").unwrap().path(home, Some(custom)),
            home.join(".workbuddy-ai/models.json")
        );
    }
}
