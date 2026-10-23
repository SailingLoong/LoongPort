//! Persistent local credentials. Provider and account secrets remain with their owners.
use super::{policy, CallerIdentity, CredentialRef};
use crate::{
    database::{lock_conn, Database},
    error::AppError,
    secrets::inventory::{open_db, seal_db},
};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::Sha256;
use zeroize::Zeroizing;

const TABLE: &str = "loongport_client_credentials";
const PREFIX: &str = "lp_client_";

pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS loongport_client_credentials (
        id TEXT PRIMARY KEY NOT NULL,
        client_id TEXT NOT NULL,
        route_app TEXT NOT NULL,
        allowed_models TEXT NOT NULL,
        secret TEXT NOT NULL,
        secret_digest TEXT NOT NULL UNIQUE,
        revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1))
    );",
    )?;
    let logs_exist: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='proxy_request_logs' AND type='table')", [], |row| row.get(0))?;
    if logs_exist {
        for column in ["client_id", "client_credential_id"] {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('proxy_request_logs') WHERE name=?1)", [column], |row| row.get(0))?;
            if !exists {
                conn.execute(
                    &format!("ALTER TABLE proxy_request_logs ADD COLUMN {column} TEXT"),
                    [],
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    conn.prepare("SELECT id, client_id, route_app, allowed_models, revoked, secret, secret_digest FROM loongport_client_credentials LIMIT 0")?;
    Ok(())
}

fn denied() -> AppError {
    AppError::Config("client_access.denied".into())
}

fn read_reference(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRef> {
    let models: String = row.get(3)?;
    let allowed_models = serde_json::from_str(&models).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(CredentialRef {
        id: row.get(0)?,
        client_id: row.get(1)?,
        route_app: row.get(2)?,
        allowed_models,
        revoked: row.get(4)?,
    })
}

fn lookup(conn: &Connection, id: &str) -> Result<(CredentialRef, String), AppError> {
    conn.query_row("SELECT id, client_id, route_app, allowed_models, revoked, secret FROM loongport_client_credentials WHERE id=?1", [id], |row| Ok((read_reference(row)?, row.get(5)?))).optional()?.ok_or_else(denied)
}

impl Database {
    /// Retained native checkpoints keep their authenticated historical layout.
    /// This is classification, not write admission: callers must run before an
    /// AppWrite/session guard, and the original writer still proves every IO.
    pub(crate) fn retained_legacy_client_projection(&self) -> Result<bool, AppError> {
        let session = self.secret_session();
        let epoch = session.lifecycle_epoch();
        let stored = {
            let conn = lock_conn!(self.conn);
            crate::database::loongport_schema::read_stored_version(&conn)?
        };
        if !(24..28).contains(&stored) {
            return Ok(false);
        }
        if !crate::mode::operation::uses_upstream4_schema(self)? {
            return Err(denied());
        }
        // Authentication above holds vault -> connection, and drops both before
        // this recheck. A lock/reunlock must never revive that classification.
        let _vault = session.read()?;
        let conn = lock_conn!(self.conn);
        if session.lifecycle_epoch() != epoch
            || crate::database::loongport_schema::read_stored_version(&conn)? != stored
            || Database::get_user_version(&conn)? != crate::database::UPSTREAM4_SCHEMA_VERSION
        {
            return Err(denied());
        }
        let marker: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key='client_access_migration_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let table: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
            [TABLE],
            |row| row.get(0),
        )?;
        // Neither a damaged marker nor a new table is a legacy witness.
        if marker.is_some() || table {
            return Err(denied());
        }
        session.ensure_available()?;
        if session.lifecycle_epoch() != epoch {
            return Err(denied());
        }
        Ok(true)
    }

    /// Published only after every managed CLI projection has been migrated.
    /// New client configuration must remain unavailable until this admission succeeds.
    pub(crate) fn client_access_ready(&self) -> Result<bool, AppError> {
        Ok(self
            .get_setting("client_access_migration_version")?
            .as_deref()
            == Some("1"))
    }

    pub(crate) fn list_client_credentials(&self) -> Result<Vec<CredentialRef>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn.prepare("SELECT id, client_id, route_app, allowed_models, revoked FROM loongport_client_credentials ORDER BY client_id, id")?;
        let rows = stmt
            .query_map([], read_reference)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn issue_client_credential(
        &self,
        client_id: &str,
        route_app: &str,
        allowed_models: &[String],
    ) -> Result<CredentialRef, AppError> {
        self.issue_client_credential_with_id(
            uuid::Uuid::new_v4().to_string(),
            client_id,
            route_app,
            allowed_models,
            None,
        )
    }

    /// Lifecycle-owned issuance is idempotent and never revives a revoked grant.
    pub(crate) fn ensure_builtin_client_credential(
        &self,
        client_id: &str,
        route_app: &str,
        models: &[String],
    ) -> Result<CredentialRef, AppError> {
        let reference = self.issue_client_credential_with_id(
            format!("builtin:{client_id}:{route_app}"),
            client_id,
            route_app,
            models,
            None,
        )?;
        if reference.revoked {
            return Err(denied());
        }
        Ok(reference)
    }

    /// Preserve an existing high-entropy local credential while moving its owner.
    pub(crate) fn adopt_builtin_client_credential(
        &self,
        client_id: &str,
        route_app: &str,
        key: &str,
    ) -> Result<CredentialRef, AppError> {
        if key.len() < 32 {
            return Err(denied());
        }
        let reference = self.issue_client_credential_with_id(
            format!("builtin:{client_id}:{route_app}"),
            client_id,
            route_app,
            &[],
            Some(key),
        )?;
        if reference.revoked
            || self.resolve_client_credential(&reference.id, client_id, route_app)? != key
        {
            return Err(denied());
        }
        Ok(reference)
    }

    pub(crate) fn builtin_client_key(
        &self,
        client_id: &str,
        route_app: &str,
    ) -> Result<String, AppError> {
        self.resolve_client_credential(
            &format!("builtin:{client_id}:{route_app}"),
            client_id,
            route_app,
        )
    }

    /// Called only by the managed configuration lifecycle, never by a read path.
    pub(crate) fn provision_managed_client_credential(
        &self,
        route_app: &str,
    ) -> Result<(), AppError> {
        if self.retained_legacy_client_projection()? {
            if !super::managed::CLI_APPS.contains(&route_app) {
                return Err(denied());
            }
            return Ok(());
        }
        self.ensure_builtin_client_credential(&format!("managed-{route_app}"), route_app, &[])
            .map(|_| ())
    }

    /// An absent credential identifies the pre-migration projection. Once admitted,
    /// missing/revoked credentials are errors and never restore legacy access.
    pub(crate) fn managed_client_key(&self, route_app: &str) -> Result<Option<String>, AppError> {
        if self.retained_legacy_client_projection()? {
            if !super::managed::CLI_APPS.contains(&route_app) {
                return Err(denied());
            }
            return Ok(Some(super::managed::LEGACY_KEY.into()));
        }
        let client_id = format!("managed-{route_app}");
        let identity = format!("builtin:{client_id}:{route_app}");
        let present = {
            let conn = lock_conn!(self.conn);
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM loongport_client_credentials WHERE id=?1)",
                [&identity],
                |row| row.get::<_, bool>(0),
            )?
        };
        if present {
            return self
                .resolve_client_credential(&identity, &client_id, route_app)
                .map(Some);
        }
        if self.client_access_ready()? {
            return Err(denied());
        }
        Ok(None)
    }

    /// Restoration recognizes the issued projection even after revocation.
    /// This is an ownership witness and never authorizes a request.
    pub(crate) fn owns_managed_client_key(
        &self,
        route_app: &str,
        key: &str,
    ) -> Result<bool, AppError> {
        if self.retained_legacy_client_projection()? {
            if !super::managed::CLI_APPS.contains(&route_app) {
                return Err(denied());
            }
            // Recognition is not authentication; no historical grant owns this key.
            return Ok(false);
        }
        let id = format!("builtin:managed-{route_app}:{route_app}");
        let digest = hex::encode(<Sha256 as sha2::Digest>::digest(key.as_bytes()));
        let conn = lock_conn!(self.conn);
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM loongport_client_credentials WHERE id=?1 AND route_app=?2 AND secret_digest=?3)", params![id, route_app, digest], |row| row.get(0))?)
    }

    fn issue_client_credential_with_id(
        &self,
        id: String,
        client_id: &str,
        route_app: &str,
        allowed_models: &[String],
        existing_key: Option<&str>,
    ) -> Result<CredentialRef, AppError> {
        if client_id.is_empty()
            || client_id.len() > 128
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !crate::app_config::AppType::all().any(|app| app.as_str() == route_app)
            || allowed_models.iter().any(|model| model.trim().is_empty())
        {
            return Err(AppError::InvalidInput("client_access.invalid_scope".into()));
        }
        let vault = self.secrets.read()?;
        let mut random = Zeroizing::new([0u8; 32]);
        getrandom::fill(random.as_mut())
            .map_err(|_| AppError::Config("client_access.random_unavailable".into()))?;
        let key = Zeroizing::new(existing_key.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "{PREFIX}{}.{}",
                id.replace(':', "_"),
                hex::encode(random.as_ref())
            )
        }));
        let digest = hex::encode(<Sha256 as sha2::Digest>::digest(key.as_bytes()));
        let sealed = seal_db(&vault, TABLE, "secret", &[&id], &key)?;
        let mut models = allowed_models.to_vec();
        models.sort();
        models.dedup();
        let conn = lock_conn!(self.conn);
        conn.execute("INSERT OR IGNORE INTO loongport_client_credentials (id, client_id, route_app, allowed_models, secret, secret_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, client_id, route_app, serde_json::to_string(&models).map_err(|error| AppError::Config(error.to_string()))?, sealed, digest])?;
        let (reference, _) = lookup(&conn, &id)?;
        if reference.client_id != client_id
            || reference.route_app != route_app
            || reference.allowed_models != models
        {
            return Err(denied());
        }
        Ok(reference)
    }

    pub(crate) fn resolve_client_credential(
        &self,
        id: &str,
        client_id: &str,
        route_app: &str,
    ) -> Result<String, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let (reference, sealed) = lookup(&conn, id)?;
        if reference.revoked || reference.client_id != client_id || reference.route_app != route_app
        {
            return Err(denied());
        }
        open_db(&vault, TABLE, "secret", &[id], &sealed)
    }

    pub(crate) fn authenticate_client(
        &self,
        key: &str,
        route_app: &str,
        model: Option<&str>,
    ) -> Result<CallerIdentity, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let digest = hex::encode(<Sha256 as sha2::Digest>::digest(key.as_bytes()));
        let id: String = conn
            .query_row(
                "SELECT id FROM loongport_client_credentials WHERE secret_digest=?1",
                [digest],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(denied)?;
        let (reference, sealed) = lookup(&conn, &id)?;
        let expected = Zeroizing::new(open_db(&vault, TABLE, "secret", &[&id], &sealed)?);
        // Compare MACs through the crypto library's constant-time verifier.
        let mut mac = Hmac::<Sha256>::new_from_slice(expected.as_bytes()).map_err(|_| denied())?;
        mac.update(b"loongport-client-auth");
        let mut supplied = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
        supplied.update(b"loongport-client-auth");
        mac.verify_slice(&supplied.finalize().into_bytes())
            .map_err(|_| denied())?;
        if !policy::permits(
            reference.revoked,
            &reference.route_app,
            route_app,
            &reference.allowed_models,
            model,
        ) {
            return Err(denied());
        }
        Ok(CallerIdentity {
            client_id: reference.client_id,
            credential_id: reference.id,
            route_app: reference.route_app,
            requested_model: model.map(str::to_owned),
        })
    }

    /// Recheck after capacity waits: a request never retains a revoked grant.
    pub(crate) fn authorize_client_identity(
        &self,
        caller: &CallerIdentity,
        route_app: &str,
        model: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let (reference, _) = lookup(&conn, &caller.credential_id)?;
        if reference.client_id != caller.client_id
            || reference.route_app != caller.route_app
            || !policy::permits(
                reference.revoked,
                &reference.route_app,
                route_app,
                &reference.allowed_models,
                model,
            )
        {
            return Err(denied());
        }
        Ok(())
    }

    pub(crate) fn revoke_client_credential(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        if conn.execute(
            "UPDATE loongport_client_credentials SET revoked=1 WHERE id=?1",
            [id],
        )? == 0
        {
            return Err(denied());
        }
        Ok(())
    }
}
