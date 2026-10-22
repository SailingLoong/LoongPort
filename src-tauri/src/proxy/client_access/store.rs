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
        revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1))
    );",
    )?;
    Ok(())
}

pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    conn.prepare("SELECT id, client_id, route_app, allowed_models, revoked, secret FROM loongport_client_credentials LIMIT 0")?;
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
        let id = uuid::Uuid::new_v4().to_string();
        let mut random = Zeroizing::new([0u8; 32]);
        getrandom::fill(random.as_mut())
            .map_err(|_| AppError::Config("client_access.random_unavailable".into()))?;
        let key = Zeroizing::new(format!("{PREFIX}{id}.{}", hex::encode(random.as_ref())));
        let sealed = seal_db(&vault, TABLE, "secret", &[&id], &key)?;
        let mut models = allowed_models.to_vec();
        models.sort();
        models.dedup();
        let conn = lock_conn!(self.conn);
        conn.execute("INSERT INTO loongport_client_credentials (id, client_id, route_app, allowed_models, secret) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, client_id, route_app, serde_json::to_string(&models).map_err(|error| AppError::Config(error.to_string()))?, sealed])?;
        Ok(CredentialRef {
            id,
            client_id: client_id.into(),
            route_app: route_app.into(),
            allowed_models: models,
            revoked: false,
        })
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
        let id = key
            .strip_prefix(PREFIX)
            .and_then(|value| value.split_once('.'))
            .map(|(id, _)| id)
            .ok_or_else(denied)?;
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let (reference, sealed) = lookup(&conn, id)?;
        let expected = Zeroizing::new(open_db(&vault, TABLE, "secret", &[id], &sealed)?);
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
