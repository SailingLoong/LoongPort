//! Single configuration owner: preview, durable encrypted intent, conflict check,
//! atomic publication and readback. Queries never resume an interrupted operation.
use super::registry;
use crate::{
    error::AppError,
    live::{
        engine::{self, DeviceStore, LiveFile},
        patch::WholeFile,
    },
    secrets::{owned_file::DeviceFile, VaultContext},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::RwLockReadGuard};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub client_id: String,
    pub credential_ref: String,
    pub route_app: String,
    pub models: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub token: String,
    pub changed: bool,
    pub model_count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct Active {
    path_identity: String,
    connection: Connection,
    ownership: Value,
}
#[derive(Serialize, Deserialize)]
struct Intent {
    path_identity: String,
    pre: Option<String>,
    post: Option<String>,
    next: Option<Active>,
}
#[derive(Default, Serialize, Deserialize)]
struct Record {
    active: Option<Active>,
    pending: Option<Intent>,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    clients: BTreeMap<String, Record>,
}

pub(crate) struct Configuration {
    store: DeviceStore,
}
fn error(code: &str) -> AppError {
    AppError::Config(code.into())
}
fn file() -> Result<DeviceFile, AppError> {
    DeviceFile::registered("client-connections.json")
}

impl Configuration {
    pub fn new(store: DeviceStore) -> Self {
        Self { store }
    }
    fn load(&self, vault: &RwLockReadGuard<'_, VaultContext>) -> Result<State, AppError> {
        self.store
            .read_device(vault, &file()?)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| error("clients.invalid_state")))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    fn save(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        state: &State,
    ) -> Result<(), AppError> {
        self.store.write_device(
            vault,
            &file()?,
            &serde_json::to_vec(state).map_err(|_| error("clients.invalid_state"))?,
        )
    }
    pub fn connection(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        id: &str,
        path: &Path,
    ) -> Result<(Option<Connection>, bool, &'static str), AppError> {
        let _lock = engine::lock_app("client-configurations");
        let state = self.load(vault)?;
        let Some(record) = state.clients.get(id) else {
            return Ok((None, false, "notConfigured"));
        };
        let status = if record.pending.is_some() {
            "recoveryRequired"
        } else if let Some(active) = &record.active {
            let current = engine::read_current(path)?;
            if active.path_identity == engine::sha256_hex(path.to_string_lossy().as_bytes())
                && registry::get(id)?
                    .adapter
                    .matches(current.as_deref(), &active.ownership)
            {
                "configured"
            } else {
                "configurationChanged"
            }
        } else {
            "notConfigured"
        };
        Ok((
            record
                .active
                .as_ref()
                .map(|active| active.connection.clone()),
            record.pending.is_some(),
            status,
        ))
    }
    fn plan(
        &self,
        state: &State,
        path: &Path,
        request: &Connection,
        desired: &Value,
    ) -> Result<(engine::Planned, Active, String, usize), AppError> {
        let current = engine::read_current(path)?;
        let record = state.clients.get(&request.client_id);
        if record.is_some_and(|r| r.pending.is_some()) {
            return Err(error("clients.recovery_required"));
        }
        let active = record.and_then(|r| r.active.as_ref());
        let adapter = registry::get(&request.client_id)?.adapter;
        let path_identity = engine::sha256_hex(path.to_string_lossy().as_bytes());
        if active.is_some_and(|a| a.path_identity != path_identity) {
            return Err(error("clients.configuration_changed"));
        }
        let projection = adapter
            .apply(current.as_deref(), active.map(|a| &a.ownership), desired)
            .map_err(|e| error(&e))?;
        let next = Active {
            path_identity,
            connection: request.clone(),
            ownership: projection.ownership,
        };
        let plan = engine::plan_from(
            &LiveFile::private(path),
            &WholeFile::Write(projection.bytes),
            current,
        )?;
        let token = engine::sha256_hex(
            &serde_json::to_vec(&(request, &plan.pre, &plan.planned))
                .map_err(|_| error("clients.invalid_request"))?,
        );
        Ok((plan, next, token, projection.model_count))
    }
    pub fn preview(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        path: &Path,
        request: &Connection,
        desired: &Value,
    ) -> Result<Preview, AppError> {
        let _lock = engine::lock_app("client-configurations");
        let (plan, _, token, model_count) =
            self.plan(&self.load(vault)?, path, request, desired)?;
        Ok(Preview {
            token,
            changed: !plan.is_noop(),
            model_count,
        })
    }
    pub fn apply(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        path: &Path,
        request: &Connection,
        desired: &Value,
        token: &str,
    ) -> Result<(), AppError> {
        let _lock = engine::lock_app("client-configurations");
        let mut state = self.load(vault)?;
        let (plan, next, expected, _) = self.plan(&state, path, request, desired)?;
        if token != expected {
            return Err(error("clients.configuration_changed"));
        }
        self.commit(vault, &mut state, &request.client_id, plan, Some(next))
    }
    fn commit(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        state: &mut State,
        id: &str,
        plan: engine::Planned,
        next: Option<Active>,
    ) -> Result<(), AppError> {
        engine::ensure_first_write_backup(&self.store, vault, &plan.file.path, plan.pre_bytes())?;
        state.clients.entry(id.into()).or_default().pending = Some(Intent {
            path_identity: engine::sha256_hex(plan.file.path.to_string_lossy().as_bytes()),
            pre: plan.pre.clone(),
            post: plan.planned.clone(),
            next,
        });
        self.save(vault, state)?;
        let staged = engine::stage(&plan)?;
        if engine::digest(engine::read_current(&plan.file.path)?.as_deref()) != plan.pre {
            if let Some(stage) = staged {
                let _ = std::fs::remove_file(stage.tmp_path());
            }
            state.clients.get_mut(id).unwrap().pending = None;
            self.save(vault, state)?;
            return Err(error("clients.configuration_changed"));
        }
        if let Some(stage) = staged {
            stage.commit()?;
        } else if plan.pre.is_some() {
            std::fs::remove_file(&plan.file.path).map_err(|e| AppError::io(&plan.file.path, e))?;
        }
        engine::sync_parent(&plan.file.path)?;
        if engine::digest(engine::read_current(&plan.file.path)?.as_deref()) != plan.planned {
            return Err(error("clients.readback_failed"));
        }
        let record = state.clients.get_mut(id).unwrap();
        record.active = record.pending.take().unwrap().next;
        self.save(vault, state)
    }
    pub fn recover(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        path: &Path,
        id: &str,
    ) -> Result<(), AppError> {
        let _lock = engine::lock_app("client-configurations");
        let mut state = self.load(vault)?;
        let Some(record) = state.clients.get_mut(id) else {
            return Ok(());
        };
        let Some(intent) = &record.pending else {
            return Ok(());
        };
        if intent.path_identity != engine::sha256_hex(path.to_string_lossy().as_bytes()) {
            return Err(error("clients.configuration_changed"));
        }
        let current = engine::digest(engine::read_current(path)?.as_deref());
        if current == intent.post {
            record.active = record.pending.take().unwrap().next;
        } else if current == intent.pre {
            record.pending = None;
        } else {
            return Err(error("clients.configuration_changed"));
        }
        self.save(vault, &state)
    }
    pub fn disconnect(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        path: &Path,
        id: &str,
    ) -> Result<(), AppError> {
        let _lock = engine::lock_app("client-configurations");
        let mut state = self.load(vault)?;
        let Some(record) = state.clients.get(id) else {
            return Ok(());
        };
        if record.pending.is_some() {
            return Err(error("clients.recovery_required"));
        }
        let Some(active) = &record.active else {
            return Ok(());
        };
        let current = engine::read_current(path)?;
        if active.path_identity != engine::sha256_hex(path.to_string_lossy().as_bytes()) {
            return Err(error("clients.configuration_changed"));
        }
        let restored = registry::get(id)?
            .adapter
            .disconnect(current.as_deref(), &active.ownership)
            .map_err(|e| error(&e))?;
        let patch = match restored {
            Some(bytes) => WholeFile::Write(bytes),
            None => WholeFile::Delete,
        };
        let plan = engine::plan_from(&LiveFile::private(path), &patch, current)?;
        self.commit(vault, &mut state, id, plan, None)
    }
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
