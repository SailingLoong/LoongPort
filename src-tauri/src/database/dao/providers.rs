use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::secrets::{
    inventory::{open_db, seal_db},
    VaultContext,
};
use indexmap::IndexMap;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet};

pub(crate) fn insert_endpoint_on_tx(
    tx: &Transaction<'_>,
    vault: &VaultContext,
    provider: &str,
    app: &str,
    url: &str,
    added_at: i64,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO provider_endpoints(provider_id,app_type,url,added_at) VALUES (?1,?2,'',?3)",
        params![provider, app, added_at],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    let id = tx.last_insert_rowid();
    let ciphertext = seal_db(
        vault,
        "provider_endpoints",
        "url",
        &[&id.to_string(), provider, app],
        url,
    )?;
    tx.execute(
        "UPDATE provider_endpoints SET url=?1 WHERE id=?2",
        params![ciphertext, id],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

fn read_available_models(raw: Option<String>) -> rusqlite::Result<Option<Vec<String>>> {
    raw.map(|raw| {
        serde_json::from_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    })
    .transpose()
}

#[cfg(test)]
mod encrypted_endpoints_tests {
    use super::*;

    #[test]
    fn endpoint_urls_are_encrypted_and_removal_uses_the_plaintext_contract() {
        let db = Database::memory().unwrap();
        let provider = Provider::with_id(
            "fixture".into(),
            "Fixture".into(),
            serde_json::json!({}),
            None,
        );
        db.save_provider("codex", &provider).unwrap();
        let url = "https://user:endpoint-canary@example.invalid/v1?token=query-canary";
        db.add_custom_endpoint("codex", "fixture", url).unwrap();
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT url FROM provider_endpoints", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains("endpoint-canary"));
        assert!(!raw.contains("query-canary"));
        let loaded = db.get_provider_by_id("fixture", "codex").unwrap().unwrap();
        assert!(loaded.meta.unwrap().custom_endpoints.contains_key(url));
        db.remove_custom_endpoint("codex", "fixture", url).unwrap();
        assert!(db
            .get_provider_by_id("fixture", "codex")
            .unwrap()
            .unwrap()
            .meta
            .unwrap()
            .custom_endpoints
            .is_empty());
    }

    #[test]
    fn moving_an_endpoint_to_another_provider_does_not_authenticate() {
        let db = Database::memory().unwrap();
        for id in ["first", "second"] {
            db.save_provider(
                "codex",
                &Provider::with_id(id.into(), "Fixture".into(), serde_json::json!({}), None),
            )
            .unwrap();
        }
        db.add_custom_endpoint("codex", "first", "https://example.invalid")
            .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute("UPDATE provider_endpoints SET provider_id='second'", [])
            .unwrap();
        assert!(db.get_provider_by_id("second", "codex").is_err());
    }
}

type OmoProviderRow = (
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    String,
);

impl Database {
    fn decode_provider_json<T: serde::de::DeserializeOwned + Default>(
        vault: &VaultContext,
        column: &str,
        id: &str,
        app_type: &str,
        raw: &str,
    ) -> Result<T, AppError> {
        let plaintext = open_db(vault, "providers", column, &[id, app_type], raw)?;
        if plaintext.is_empty() {
            return Ok(T::default());
        }
        serde_json::from_str(&plaintext)
            .map_err(|_| AppError::Database(format!("Invalid provider {column}")))
    }

    pub(crate) fn get_provider_meta_on_connection(
        conn: &rusqlite::Connection,
        vault: &VaultContext,
        id: &str,
        app_type: &str,
    ) -> Result<Option<ProviderMeta>, AppError> {
        let raw: Option<String> = conn
            .query_row(
                "SELECT meta FROM providers WHERE id=?1 AND app_type=?2",
                params![id, app_type],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| Self::decode_provider_json(vault, "meta", id, app_type, &raw))
            .transpose()
    }

    pub fn get_all_providers(
        &self,
        app_type: &str,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        Self::get_all_providers_on_connection(&conn, &vault, app_type)
    }

    /// Read current-provider state and provider rows through an active transaction.
    ///
    /// Callers that use the result to decide writes must begin the transaction before
    /// calling this helper so the decision is based on one database snapshot.
    pub(crate) fn get_provider_snapshot_in_transaction(
        transaction: &Transaction<'_>,
        vault: &VaultContext,
        app_type: &str,
    ) -> Result<(Option<String>, IndexMap<String, Provider>), AppError> {
        Ok((
            Self::get_current_provider_on_connection(transaction, app_type)?,
            Self::get_all_providers_on_connection(transaction, vault, app_type)?,
        ))
    }

    fn get_all_providers_on_connection(
        conn: &rusqlite::Connection,
        vault: &VaultContext,
        app_type: &str,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let mut stmt = conn.prepare(
            "SELECT id, name, settings_config, website_url, category, created_at, sort_index, notes, icon, icon_color, meta, in_failover_queue, available_models
             FROM providers WHERE app_type = ?1
             ORDER BY COALESCE(sort_index, 999999), created_at ASC, id ASC"
        ).map_err(|e| AppError::Database(e.to_string()))?;

        let provider_iter = stmt
            .query_map(params![app_type], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let settings_config_str: String = row.get(2)?;
                let website_url: Option<String> = row.get(3)?;
                let category: Option<String> = row.get(4)?;
                let created_at: Option<i64> = row.get(5)?;
                let sort_index: Option<usize> =
                    row.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as usize);
                let notes: Option<String> = row.get(7)?;
                let icon: Option<String> = row.get(8)?;
                let icon_color: Option<String> = row.get(9)?;
                let meta_str: String = row.get(10)?;
                let in_failover_queue: bool = row.get(11)?;
                let available_models = read_available_models(row.get(12)?)?;

                Ok((
                    id,
                    Provider {
                        id: "".to_string(), // Placeholder, set below
                        name,
                        settings_config: serde_json::Value::Null,
                        website_url,
                        category,
                        created_at,
                        sort_index,
                        notes,
                        meta: None,
                        icon,
                        icon_color,
                        in_failover_queue,
                        available_models,
                    },
                    settings_config_str,
                    meta_str,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut providers = IndexMap::new();
        for provider_res in provider_iter {
            let (id, mut provider, settings, meta) = provider_res?;
            provider.settings_config =
                Self::decode_provider_json(vault, "settings_config", &id, app_type, &settings)?;
            provider.meta = Some(Self::decode_provider_json(
                vault, "meta", &id, app_type, &meta,
            )?);
            provider.id = id.clone();

            if let Some(meta) = &mut provider.meta {
                meta.custom_endpoints =
                    Self::get_endpoints_on_connection(conn, vault, &id, app_type)?;
            }

            providers.insert(id, provider);
        }

        Ok(providers)
    }

    fn get_endpoints_on_connection(
        conn: &rusqlite::Connection,
        vault: &VaultContext,
        id: &str,
        app_type: &str,
    ) -> Result<HashMap<String, crate::settings::CustomEndpoint>, AppError> {
        let mut stmt = conn.prepare("SELECT id,url,added_at FROM provider_endpoints WHERE provider_id=?1 AND app_type=?2 ORDER BY added_at ASC,id ASC").map_err(|e|AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![id, app_type], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut endpoints = HashMap::new();
        for row in rows {
            let (endpoint_id, ciphertext, added_at) =
                row.map_err(|e| AppError::Database(e.to_string()))?;
            let url = open_db(
                vault,
                "provider_endpoints",
                "url",
                &[&endpoint_id.to_string(), id, app_type],
                &ciphertext,
            )?;
            endpoints.insert(
                url.clone(),
                crate::settings::CustomEndpoint {
                    url,
                    added_at: added_at.unwrap_or(0),
                    last_used: None,
                },
            );
        }
        Ok(endpoints)
    }

    pub fn get_current_provider(&self, app_type: &str) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::get_current_provider_on_connection(&conn, app_type)
    }

    fn get_current_provider_on_connection(
        conn: &rusqlite::Connection,
        app_type: &str,
    ) -> Result<Option<String>, AppError> {
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1 AND is_current = 1 LIMIT 1")
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut rows = stmt
            .query(params![app_type])
            .map_err(|e| AppError::Database(e.to_string()))?;

        if let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            Ok(Some(
                row.get(0).map_err(|e| AppError::Database(e.to_string()))?,
            ))
        } else {
            Ok(None)
        }
    }

    pub fn get_provider_by_id(
        &self,
        id: &str,
        app_type: &str,
    ) -> Result<Option<Provider>, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let result = conn.query_row(
            "SELECT name, settings_config, website_url, category, created_at, sort_index, notes, icon, icon_color, meta, in_failover_queue, available_models
             FROM providers WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
            |row| {
                let name: String = row.get(0)?;
                let settings_config_str: String = row.get(1)?;
                let website_url: Option<String> = row.get(2)?;
                let category: Option<String> = row.get(3)?;
                let created_at: Option<i64> = row.get(4)?;
                let sort_index: Option<usize> =
                    row.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as usize);
                let notes: Option<String> = row.get(6)?;
                let icon: Option<String> = row.get(7)?;
                let icon_color: Option<String> = row.get(8)?;
                let meta_str: String = row.get(9)?;
                let in_failover_queue: bool = row.get(10)?;
                let available_models = read_available_models(row.get(11)?)?;

                Ok((Provider {
                    id: id.to_string(),
                    name,
                    settings_config: serde_json::Value::Null,
                    website_url,
                    category,
                    created_at,
                    sort_index,
                    notes,
                    meta: None,
                    icon,
                    icon_color,
                    in_failover_queue,
                    available_models,
                }, settings_config_str, meta_str))
            },
        );

        match result {
            Ok((mut provider, settings, meta)) => {
                provider.settings_config =
                    Self::decode_provider_json(&vault, "settings_config", id, app_type, &settings)?;
                provider.meta = Some(Self::decode_provider_json(
                    &vault, "meta", id, app_type, &meta,
                )?);
                if let Some(meta) = &mut provider.meta {
                    meta.custom_endpoints =
                        Self::get_endpoints_on_connection(&conn, &vault, id, app_type)?;
                }
                Ok(Some(provider))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    pub fn save_provider(&self, app_type: &str, provider: &Provider) -> Result<(), AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut meta_clone = provider.meta.clone().unwrap_or_default();
        let endpoints = std::mem::take(&mut meta_clone.custom_endpoints);
        let settings = seal_db(
            &vault,
            "providers",
            "settings_config",
            &[&provider.id, app_type],
            &serde_json::to_string(&provider.settings_config)
                .map_err(|e| AppError::Database(e.to_string()))?,
        )?;
        let meta = seal_db(
            &vault,
            "providers",
            "meta",
            &[&provider.id, app_type],
            &serde_json::to_string(&meta_clone).map_err(|e| AppError::Database(e.to_string()))?,
        )?;

        let existing: Option<(bool, bool)> = tx
            .query_row(
                "SELECT is_current, in_failover_queue FROM providers WHERE id = ?1 AND app_type = ?2",
                params![provider.id, app_type],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let is_update = existing.is_some();
        let (is_current, in_failover_queue) =
            existing.unwrap_or((false, provider.in_failover_queue));

        if is_update {
            tx.execute(
                "UPDATE providers SET
                    name = ?1,
                    settings_config = ?2,
                    website_url = ?3,
                    category = ?4,
                    created_at = ?5,
                    sort_index = ?6,
                    notes = ?7,
                    icon = ?8,
                    icon_color = ?9,
                    meta = ?10,
                    is_current = ?11,
                    in_failover_queue = ?12
                WHERE id = ?13 AND app_type = ?14",
                params![
                    provider.name,
                    settings,
                    provider.website_url,
                    provider.category,
                    provider.created_at,
                    provider.sort_index.map(|v| v as i64),
                    provider.notes,
                    provider.icon,
                    provider.icon_color,
                    meta,
                    is_current,
                    in_failover_queue,
                    provider.id,
                    app_type,
                ],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        } else {
            tx.execute(
                "INSERT INTO providers (
                    id, app_type, name, settings_config, website_url, category,
                    created_at, sort_index, notes, icon, icon_color, meta, is_current, in_failover_queue
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    provider.id,
                    app_type,
                    provider.name,
                    settings,
                    provider.website_url,
                    provider.category,
                    provider.created_at,
                    provider.sort_index.map(|v| v as i64),
                    provider.notes,
                    provider.icon,
                    provider.icon_color,
                    meta,
                    is_current,
                    in_failover_queue,
                ],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

            for (url, endpoint) in endpoints {
                insert_endpoint_on_tx(
                    &tx,
                    &vault,
                    &provider.id,
                    app_type,
                    &url,
                    endpoint.added_at,
                )?;
            }
        }

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        // 释放连接锁再追加链成员：note_provider_created 内部要拿同一把锁，
        // conn 守卫活到函数尾的话这里就死锁了。
        drop(conn);

        // 链成员资格：新档位（插入分支）自动进链垫底——上游新增默认排在最后生效。
        // 只挂创建、不挂更新：被用户应用出链的档位刷新后不能爬回链里。
        // 追加失败只警告不阻断——主写入已成功，链是辅助状态（与 record_attempt 同款尽力而为）。
        if !is_update {
            if let Err(e) = crate::proxy::application_routing::note_provider_created(
                self,
                app_type,
                &provider.id,
            ) {
                log::warn!("Could not append new provider to application chain: {e}");
            }
        }
        Ok(())
    }

    pub fn delete_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM providers WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_current_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "UPDATE providers SET is_current = 0 WHERE app_type = ?1",
            params![app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        tx.execute(
            "UPDATE providers SET is_current = 1 WHERE id = ?1 AND app_type = ?2",
            params![id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 读「已手工维护」标记。行不存在按 `false` 处理（调用方都应知道行在，
    /// 这条只是防御：读不到不等于报错）。
    pub fn get_user_edited(&self, app_type: &str, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let edited: Option<bool> = conn
            .query_row(
                "SELECT user_edited FROM providers WHERE id = ?1 AND app_type = ?2",
                params![id, app_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(edited.unwrap_or(false))
    }

    /// 写「已手工维护」标记。置位：手工编辑保存；复位：「恢复默认配置」。
    pub fn set_user_edited(&self, app_type: &str, id: &str, value: bool) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE providers SET user_edited = ?1 WHERE id = ?2 AND app_type = ?3",
            params![value, id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Only the relay catalog owner writes this column; regular provider saves preserve it.
    pub(crate) fn set_available_models(
        &self,
        app: &str,
        id: &str,
        models: &[String],
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE providers SET available_models=?1 WHERE app_type=?2 AND id=?3",
            params![
                serde_json::to_string(models)
                    .map_err(|error| AppError::Database(error.to_string()))?,
                app,
                id
            ],
        )?;
        Ok(())
    }

    /// Complete a missing-inventory fetch only if the record still has its original binding.
    /// Comparing settings is conservative: concurrent user edits defer repair to the next pass.
    pub(crate) fn fill_missing_available_models(
        &self,
        app: &str,
        original: &Provider,
        expected_relay: &crate::relay::creds::RelayAccount,
        models: &[String],
    ) -> Result<bool, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let binding: Option<(String, String, Option<i64>)> = conn
            .query_row(
                "SELECT site_origin, api_base_url, account_id FROM loongport_relay WHERE id=?1",
                [expected_relay.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if binding
            != Some((
                expected_relay.site_origin.clone(),
                expected_relay.api_base_url.clone(),
                expected_relay.account_id,
            ))
        {
            return Ok(false);
        }
        let current: Option<(String, Option<String>, String)> = conn.query_row(
            "SELECT settings_config, website_url, meta FROM providers WHERE app_type=?1 AND id=?2 AND available_models IS NULL",
            params![app, original.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        let Some((settings, website, meta)) = current else {
            return Ok(false);
        };
        let settings: serde_json::Value =
            Self::decode_provider_json(&vault, "settings_config", &original.id, app, &settings)?;
        let meta: ProviderMeta =
            Self::decode_provider_json(&vault, "meta", &original.id, app, &meta)?;
        if settings != original.settings_config
            || website != original.website_url
            || meta.loongport_account_id
                != original
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.loongport_account_id)
        {
            return Ok(false);
        }
        let count = conn.execute("UPDATE providers SET available_models=?1 WHERE app_type=?2 AND id=?3 AND available_models IS NULL",
            params![serde_json::to_string(models).map_err(|error| AppError::Database(error.to_string()))?, app, original.id])?;
        Ok(count == 1)
    }

    /// 读档位的存库倍率。`None` = 还没查过 / 行不存在，**不是 0** ——
    /// UI 拿到 `None` 显示「倍率未知」，显示成 0 会让用户以为这是最便宜的一档。
    ///
    /// 值由 provision 写入（那一步本来就在拉分组），所以「刷新倍率」= 重拉分组。
    pub fn get_tier_rate_multiplier(
        &self,
        app_type: &str,
        id: &str,
    ) -> Result<Option<f64>, AppError> {
        let conn = lock_conn!(self.conn);
        let rate: Option<Option<f64>> = conn
            .query_row(
                "SELECT tier_rate_multiplier FROM providers WHERE id = ?1 AND app_type = ?2",
                params![id, app_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(rate.flatten())
    }

    /// 写档位的存库倍率。`None` 表示这次也没查到 —— 照样写下去，
    /// 让「查不到」这件事覆盖掉一个可能已经过时的旧值。
    pub fn set_tier_rate_multiplier(
        &self,
        app_type: &str,
        id: &str,
        value: Option<f64>,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE providers SET tier_rate_multiplier = ?1 WHERE id = ?2 AND app_type = ?3",
            params![value, id, app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 批量读一个应用下全部档位的存库倍率（自动模式「价格最低」策略用）。
    ///
    /// 只返回有值的行：key 不在 map 里 = 倍率未知（排序时当最贵处理，
    /// 见 `proxy::auto_strategy`），免得调用方到处拆 `Option`。
    pub fn get_tier_rate_multipliers(
        &self,
        app_type: &str,
    ) -> Result<HashMap<String, f64>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, tier_rate_multiplier FROM providers
                 WHERE app_type = ?1 AND tier_rate_multiplier IS NOT NULL",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![app_type], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut result = HashMap::new();
        for row in rows {
            let (id, multiplier) = row.map_err(|e| AppError::Database(e.to_string()))?;
            result.insert(id, multiplier);
        }
        Ok(result)
    }

    pub fn update_provider_settings_config(
        &self,
        app_type: &str,
        provider_id: &str,
        settings_config: &serde_json::Value,
    ) -> Result<(), AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE providers SET settings_config = ?1 WHERE id = ?2 AND app_type = ?3",
            params![
                seal_db(
                    &vault,
                    "providers",
                    "settings_config",
                    &[provider_id, app_type],
                    &serde_json::to_string(settings_config)
                        .map_err(|e| AppError::Database(e.to_string()))?
                )?,
                provider_id,
                app_type
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn add_custom_endpoint(
        &self,
        app_type: &str,
        provider_id: &str,
        url: &str,
    ) -> Result<(), AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        let added_at = chrono::Utc::now().timestamp_millis();
        insert_endpoint_on_tx(&tx, &vault, provider_id, app_type, url, added_at)?;
        tx.commit().map_err(|e| AppError::Database(e.to_string()))
    }

    pub fn remove_custom_endpoint(
        &self,
        app_type: &str,
        provider_id: &str,
        url: &str,
    ) -> Result<(), AppError> {
        let vault = self.secrets.read()?;
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut stmt = tx
            .prepare("SELECT id,url FROM provider_endpoints WHERE provider_id=?1 AND app_type=?2")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider_id, app_type], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        drop(stmt);
        for (id, ciphertext) in rows {
            if open_db(
                &vault,
                "provider_endpoints",
                "url",
                &[&id.to_string(), provider_id, app_type],
                &ciphertext,
            )? == url
            {
                tx.execute("DELETE FROM provider_endpoints WHERE id=?1", [id])
                    .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_omo_provider_current(
        &self,
        app_type: &str,
        provider_id: &str,
        category: &str,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        tx.execute(
            "UPDATE providers SET is_current = 0 WHERE app_type = ?1 AND category = ?2",
            params![app_type, category],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        // OMO ↔ OMO Slim mutually exclusive: deactivate the opposite category
        let opposite = match category {
            "omo" => Some("omo-slim"),
            "omo-slim" => Some("omo"),
            _ => None,
        };
        if let Some(opp) = opposite {
            tx.execute(
                "UPDATE providers SET is_current = 0 WHERE app_type = ?1 AND category = ?2",
                params![app_type, opp],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        let updated = tx
            .execute(
                "UPDATE providers SET is_current = 1 WHERE id = ?1 AND app_type = ?2 AND category = ?3",
                params![provider_id, app_type, category],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        if updated != 1 {
            return Err(AppError::Database(format!(
                "Failed to set {category} provider current: provider '{provider_id}' not found in app '{app_type}'"
            )));
        }
        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn is_omo_provider_current(
        &self,
        app_type: &str,
        provider_id: &str,
        category: &str,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        match conn.query_row(
            "SELECT is_current FROM providers
             WHERE id = ?1 AND app_type = ?2 AND category = ?3",
            params![provider_id, app_type, category],
            |row| row.get(0),
        ) {
            Ok(is_current) => Ok(is_current),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    pub fn clear_omo_provider_current(
        &self,
        app_type: &str,
        provider_id: &str,
        category: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE providers SET is_current = 0
             WHERE id = ?1 AND app_type = ?2 AND category = ?3",
            params![provider_id, app_type, category],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_current_omo_provider(
        &self,
        app_type: &str,
        category: &str,
    ) -> Result<Option<Provider>, AppError> {
        let vault = self.secrets.read()?;
        let conn = lock_conn!(self.conn);
        let row_data: Result<OmoProviderRow, rusqlite::Error> = conn.query_row(
            "SELECT id, name, settings_config, category, created_at, sort_index, notes, meta
             FROM providers
             WHERE app_type = ?1 AND category = ?2 AND is_current = 1
             LIMIT 1",
            params![app_type, category],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        );

        let (id, name, settings_config_str, _row_category, created_at, sort_index, notes, meta_str) =
            match row_data {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => return Err(AppError::Database(e.to_string())),
            };

        let settings_config = Self::decode_provider_json(
            &vault,
            "settings_config",
            &id,
            app_type,
            &settings_config_str,
        )?;
        let meta: ProviderMeta =
            Self::decode_provider_json(&vault, "meta", &id, app_type, &meta_str)?;

        Ok(Some(Provider {
            id,
            name,
            settings_config,
            website_url: None,
            category: Some(category.to_string()),
            created_at,
            sort_index: sort_index.map(|v| v.max(0) as usize),
            notes,
            meta: Some(meta),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
            available_models: None,
        }))
    }

    /// 判断 providers 表是否为空（全 app_type 一起算）。
    ///
    /// 用于区分"全新安装"和"升级用户"：在启动流程 import/seed 之前调用。
    /// 使用 `EXISTS` 短路查询，比 `COUNT(*)` 在将来表变大时更高效。
    pub fn is_providers_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let exists: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM providers)", [], |row| {
                row.get(0)
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(!exists)
    }

    /// 仅获取指定 app 下所有 provider 的 id 集合。
    ///
    /// 比 `get_all_providers` 轻量得多：只读 id 列、无 endpoint 子查询。
    /// 用于只需要做存在性检查的场景（如 additive 模式的 live 同步去重）。
    pub fn get_provider_ids(&self, app_type: &str) -> Result<HashSet<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![app_type], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut ids = HashSet::new();
        for row in rows {
            ids.insert(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(ids)
    }

    /// 判断指定 app 下是否已存在任意 provider。
    ///
    /// 启动阶段的 live import 需要使用这个更严格的判断：
    /// 只要该 app 已经有任何 provider（包括官方 seed），就不应再自动导入 `default`。
    pub fn has_any_provider_for_app(&self, app_type: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM providers WHERE app_type = ?1)",
                params![app_type],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(exists)
    }

    /// 判断指定 app 下是否存在非官方种子的供应商。
    ///
    /// 比 `get_all_providers` 轻量得多：只读 id 列、无 endpoint 子查询、首条命中即返回。
    /// 用于 `import_default_config` 决定是否跳过 live 导入。
    pub fn has_non_official_seed_provider(&self, app_type: &str) -> Result<bool, AppError> {
        use crate::database::dao::providers_seed::is_official_seed_id;
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut rows = stmt
            .query(params![app_type])
            .map_err(|e| AppError::Database(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let id: String = row.get(0).map_err(|e| AppError::Database(e.to_string()))?;
            if !is_official_seed_id(&id) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 计算指定 app 下一个可用的 sort_index（追加到末尾）。
    fn next_sort_index_for_app(&self, app_type: &str) -> Result<usize, AppError> {
        let conn = lock_conn!(self.conn);
        let max: Option<i64> = conn
            .query_row(
                "SELECT MAX(sort_index) FROM providers WHERE app_type = ?1",
                params![app_type],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(max.map(|v| (v + 1) as usize).unwrap_or(0))
    }

    /// 启动时调用：补齐缺失的官方预设供应商（Claude / Codex / Gemini）。
    ///
    /// 使用 settings flag `official_providers_seeded` 保证每个数据库只执行一次：
    /// - 全新用户：seed 三条官方预设
    /// - 老用户升级：同样会触发一次（flag 不存在），追加到末尾，不影响已有排序
    /// - 用户删除 seed 后：不再重建（flag 已为 true），尊重用户意图
    ///
    /// 与 `Database::save_provider` 的 UPSERT 语义配合，即使被意外重复调用
    /// 也不会覆盖用户当前激活的供应商（is_current 字段会被保留）。
    pub fn init_default_official_providers(&self) -> Result<usize, AppError> {
        use crate::database::dao::providers_seed::OFFICIAL_SEEDS;

        if self
            .get_bool_flag("official_providers_seeded")
            .unwrap_or(false)
        {
            return Ok(0);
        }

        let mut inserted = 0_usize;
        let now_ms = chrono::Utc::now().timestamp_millis();

        for seed in OFFICIAL_SEEDS {
            let app_type_str = seed.app_type.as_str();

            // 若该 id 已存在（极端情况：用户曾手动用过同 id），跳过
            if self.get_provider_by_id(seed.id, app_type_str)?.is_some() {
                continue;
            }

            let next_sort_index = self.next_sort_index_for_app(app_type_str)?;

            let settings_config: serde_json::Value =
                serde_json::from_str(seed.settings_config_json).map_err(|e| {
                    AppError::Database(format!("Seed JSON parse failed for {}: {e}", seed.id))
                })?;

            let mut provider = Provider::with_id(
                seed.id.to_string(),
                seed.name.to_string(),
                settings_config,
                Some(seed.website_url.to_string()),
            );
            provider.category = Some("official".to_string());
            provider.icon = Some(seed.icon.to_string());
            provider.icon_color = Some(seed.icon_color.to_string());
            provider.sort_index = Some(next_sort_index);
            provider.created_at = Some(now_ms);

            self.save_provider(app_type_str, &provider)?;
            inserted += 1;
            log::info!(
                "✓ Seeded official provider: {} ({})",
                seed.name,
                app_type_str
            );
        }

        // 即使 inserted=0（例如用户手动创建过同 id）也设置 flag 防止反复检查
        self.set_setting("official_providers_seeded", "true")?;

        Ok(inserted)
    }

    /// 按 id 兜底插入单条 official seed（仅当目标表中该 id 不存在时插入）。
    ///
    /// 与 `init_default_official_providers` 不同：
    /// - 不触碰 `official_providers_seeded` 全局 flag，是 on-demand 修复
    /// - 只处理一条 seed，由调用方决定 id + app_type
    /// - 已存在则尊重用户自定义，不覆盖
    ///
    /// 返回 Ok(true) 表示插入了新行，Ok(false) 表示已存在被跳过。
    pub fn ensure_official_seed_by_id(
        &self,
        seed_id: &str,
        app_type: crate::app_config::AppType,
    ) -> Result<bool, AppError> {
        use crate::database::dao::providers_seed::OFFICIAL_SEEDS;

        let seed = OFFICIAL_SEEDS
            .iter()
            .find(|s| s.id == seed_id && s.app_type == app_type)
            .ok_or_else(|| {
                AppError::Database(format!(
                    "unknown official seed: id={seed_id}, app_type={}",
                    app_type.as_str()
                ))
            })?;

        let app_type_str = seed.app_type.as_str();

        if self.get_provider_by_id(seed_id, app_type_str)?.is_some() {
            return Ok(false);
        }

        let settings_config: serde_json::Value = serde_json::from_str(seed.settings_config_json)
            .map_err(|e| {
                AppError::Database(format!("Seed JSON parse failed for {}: {e}", seed.id))
            })?;

        let next_sort_index = self.next_sort_index_for_app(app_type_str)?;
        let now_ms = chrono::Utc::now().timestamp_millis();

        let mut provider = Provider::with_id(
            seed.id.to_string(),
            seed.name.to_string(),
            settings_config,
            Some(seed.website_url.to_string()),
        );
        provider.category = Some("official".to_string());
        provider.icon = Some(seed.icon.to_string());
        provider.icon_color = Some(seed.icon_color.to_string());
        provider.sort_index = Some(next_sort_index);
        provider.created_at = Some(now_ms);

        self.save_provider(app_type_str, &provider)?;

        Ok(true)
    }
}

#[cfg(test)]
mod ensure_official_seed_tests {
    use crate::app_config::AppType;
    use crate::database::{
        Database, CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID, CODEX_OFFICIAL_PROVIDER_ID,
        GROKBUILD_OFFICIAL_PROVIDER_ID,
    };

    #[test]
    fn ensure_inserts_when_missing() {
        let db = Database::memory().expect("memory db");
        let inserted = db
            .ensure_official_seed_by_id(CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID, AppType::ClaudeDesktop)
            .expect("ensure ok");
        assert!(inserted, "should insert when missing");

        let provider = db
            .get_provider_by_id(
                CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                AppType::ClaudeDesktop.as_str(),
            )
            .expect("query ok")
            .expect("provider exists after ensure");

        assert_eq!(provider.id, CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID);
        assert_eq!(provider.name, "Claude Desktop Official");
        assert_eq!(provider.category.as_deref(), Some("official"));
        assert_eq!(provider.icon.as_deref(), Some("anthropic"));
        assert_eq!(provider.icon_color.as_deref(), Some("#D4915D"));
    }

    #[test]
    fn ensure_skips_when_present_and_preserves_customization() {
        let db = Database::memory().expect("memory db");
        db.init_default_official_providers().expect("seed");

        let mut renamed = db
            .get_provider_by_id(
                CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                AppType::ClaudeDesktop.as_str(),
            )
            .expect("query ok")
            .expect("seed present");
        renamed.name = "My Custom Backup".to_string();
        db.save_provider(AppType::ClaudeDesktop.as_str(), &renamed)
            .expect("save customization");

        let inserted = db
            .ensure_official_seed_by_id(CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID, AppType::ClaudeDesktop)
            .expect("ensure ok");
        assert!(!inserted, "should skip when present");

        let after = db
            .get_provider_by_id(
                CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                AppType::ClaudeDesktop.as_str(),
            )
            .expect("query ok")
            .expect("still present");
        assert_eq!(
            after.name, "My Custom Backup",
            "customization must not be overwritten"
        );
    }

    #[test]
    fn ensure_recreates_codex_official_seed_after_deletion() {
        let db = Database::memory().expect("memory db");
        db.init_default_official_providers().expect("seed");
        db.delete_provider(AppType::Codex.as_str(), CODEX_OFFICIAL_PROVIDER_ID)
            .expect("delete Codex official");

        let inserted = db
            .ensure_official_seed_by_id(CODEX_OFFICIAL_PROVIDER_ID, AppType::Codex)
            .expect("ensure Codex official");
        assert!(inserted);
        let provider = db
            .get_provider_by_id(CODEX_OFFICIAL_PROVIDER_ID, AppType::Codex.as_str())
            .expect("query")
            .expect("Codex official restored");
        assert_eq!(provider.category.as_deref(), Some("official"));
        assert_eq!(provider.settings_config["auth"], serde_json::json!({}));
    }

    #[test]
    fn ensure_recreates_grokbuild_official_seed_after_deletion() {
        let db = Database::memory().expect("memory db");
        db.init_default_official_providers().expect("seed");
        db.delete_provider(AppType::GrokBuild.as_str(), GROKBUILD_OFFICIAL_PROVIDER_ID)
            .expect("delete Grok Build official");

        let inserted = db
            .ensure_official_seed_by_id(GROKBUILD_OFFICIAL_PROVIDER_ID, AppType::GrokBuild)
            .expect("ensure Grok Build official");
        assert!(inserted);
        let provider = db
            .get_provider_by_id(GROKBUILD_OFFICIAL_PROVIDER_ID, AppType::GrokBuild.as_str())
            .expect("query")
            .expect("Grok Build official restored");
        assert_eq!(provider.category.as_deref(), Some("official"));
        // 空 config：切换时不注入自定义模型表，Grok CLI 回落到自带 OAuth 登录
        assert_eq!(provider.settings_config["config"], serde_json::json!(""));
    }

    #[test]
    fn ensure_rejects_unknown_seed() {
        let db = Database::memory().expect("memory db");
        let result = db.ensure_official_seed_by_id("nonexistent-id", AppType::ClaudeDesktop);
        assert!(result.is_err(), "unknown seed id should be Err");
    }

    #[test]
    fn ensure_rejects_seed_app_type_mismatch() {
        let db = Database::memory().expect("memory db");
        let result =
            db.ensure_official_seed_by_id(CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID, AppType::Claude);
        assert!(result.is_err(), "(id, app_type) mismatch should be Err");
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_provider_metadata_and_omo_use_the_full_primary_key() {
        let db = Database::memory().unwrap();
        let mut provider = Provider::with_id(
            "shared-id".into(),
            "Provider".into(),
            json!({"api_key":"omo-canary"}),
            None,
        );
        provider.category = Some("omo".into());
        provider.meta = Some(serde_json::from_value(json!({"usage_script":{"enabled":true,"language":"javascript","code":"return []", "apiKey":"usage-canary"}})).unwrap());
        db.save_provider("opencode", &provider).unwrap();
        db.save_provider("claude", &provider).unwrap();
        db.set_omo_provider_current("opencode", &provider.id, "omo")
            .unwrap();
        let current = db
            .get_current_omo_provider("opencode", "omo")
            .unwrap()
            .unwrap();
        assert_eq!(current.settings_config, provider.settings_config);
        assert_eq!(
            current
                .meta
                .unwrap()
                .usage_script
                .unwrap()
                .api_key
                .as_deref(),
            Some("usage-canary")
        );
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT meta FROM providers WHERE id='shared-id' AND app_type='opencode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.contains("usage-canary"));
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE providers SET meta=?1 WHERE id='shared-id' AND app_type='claude'",
                [raw],
            )
            .unwrap();
        assert!(db.get_provider_by_id(&provider.id, "claude").is_err());
    }

    #[test]
    fn provider_secrets_roundtrip_and_reject_swapped_ciphertext() {
        let db = Database::memory().unwrap();
        let provider = Provider::with_id(
            "protected-provider".into(),
            "Provider".into(),
            json!({"api_key":"provider-canary"}),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        let updated = json!({"api_key":"updated-canary"});
        db.update_provider_settings_config("claude", &provider.id, &updated)
            .unwrap();
        assert_eq!(
            db.get_provider_by_id(&provider.id, "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            updated
        );
        db.save_provider("claude", &provider).unwrap();
        assert_eq!(
            db.get_provider_by_id(&provider.id, "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
        assert_eq!(
            db.get_all_providers("claude").unwrap()[&provider.id].settings_config,
            provider.settings_config
        );
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT settings_config FROM providers WHERE id=?1",
                [&provider.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.contains("provider-canary"));
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE providers SET meta=settings_config WHERE id=?1",
                [&provider.id],
            )
            .unwrap();
        assert!(db.get_provider_by_id(&provider.id, "claude").is_err());
        assert!(db.get_all_providers("claude").is_err());
    }
}
