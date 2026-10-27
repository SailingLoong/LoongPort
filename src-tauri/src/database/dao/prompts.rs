//! 提示词数据访问对象
//!
//! 提供提示词（Prompt）的 CRUD 操作。

use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::prompt::Prompt;
use indexmap::IndexMap;
use rusqlite::{params, Connection};

fn get_prompts_on(conn: &Connection, app_type: &str) -> Result<IndexMap<String, Prompt>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, content, description, enabled, created_at, updated_at
             FROM prompts WHERE app_type = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

    let prompt_iter = stmt
        .query_map(params![app_type], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let content: String = row.get(2)?;
            let description: Option<String> = row.get(3)?;
            let enabled: bool = row.get(4)?;
            let created_at: Option<i64> = row.get(5)?;
            let updated_at: Option<i64> = row.get(6)?;

            Ok((
                id.clone(),
                Prompt {
                    id,
                    name,
                    content,
                    description,
                    enabled,
                    created_at,
                    updated_at,
                },
            ))
        })
        .map_err(|e| AppError::Database(e.to_string()))?;

    let mut prompts = IndexMap::new();
    for prompt_res in prompt_iter {
        let (id, prompt) = prompt_res.map_err(|e| AppError::Database(e.to_string()))?;
        prompts.insert(id, prompt);
    }
    Ok(prompts)
}

fn save_prompt_on(conn: &Connection, app_type: &str, prompt: &Prompt) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR REPLACE INTO prompts (
                id, app_type, name, content, description, enabled, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            prompt.id,
            app_type,
            prompt.name,
            prompt.content,
            prompt.description,
            prompt.enabled,
            prompt.created_at,
            prompt.updated_at,
        ],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

fn delete_prompt_on(conn: &Connection, app_type: &str, id: &str) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM prompts WHERE id = ?1 AND app_type = ?2",
        params![id, app_type],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

impl Database {
    /// 获取指定应用类型的所有提示词
    pub fn get_prompts(&self, app_type: &str) -> Result<IndexMap<String, Prompt>, AppError> {
        let conn = lock_conn!(self.conn);
        get_prompts_on(&conn, app_type)
    }

    /// 保存提示词
    pub fn save_prompt(&self, app_type: &str, prompt: &Prompt) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        save_prompt_on(&conn, app_type, prompt)
    }

    /// 删除提示词
    pub fn delete_prompt(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        delete_prompt_on(&conn, app_type, id)
    }

    /// Retain the existing connection owner through one save/native/compensation
    /// operation, so compensation cannot overwrite an intervening DAO write.
    /// Saves remain autocommit: no new database commit can fail after native success.
    /// The callback must perform only native I/O, with paths and file guards already
    /// resolved; it must not reenter the database or acquire another owner.
    /// This is not a transaction over multi-row enable operations or external writers.
    pub(crate) fn save_prompt_with_native_update(
        &self,
        app: &AppType,
        stored: &Prompt,
        update_native: impl FnOnce(&IndexMap<String, Prompt>) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let previous_prompts = get_prompts_on(&conn, app.as_str())?;
        save_prompt_on(&conn, app.as_str(), stored)?;
        if let Err(native_error) = update_native(&previous_prompts) {
            let rollback = match previous_prompts.get(&stored.id) {
                Some(previous) => save_prompt_on(&conn, app.as_str(), previous),
                None => delete_prompt_on(&conn, app.as_str(), &stored.id),
            };
            if let Err(rollback_error) = rollback {
                return Err(AppError::Message(format!(
                    "{app:?} prompt update failed ({native_error}); database rollback also failed: {rollback_error}"
                )));
            }
            return Err(native_error);
        }
        Ok(())
    }
}
