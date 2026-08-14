use std::collections::HashMap;

use rusqlite::params;

use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::{api, backend::BackendKind, creds::Relay, newapi, provision},
};

#[derive(Debug, Clone, PartialEq)]
pub struct RateUpdate {
    pub provider_id: String,
    pub rate_multiplier: Option<f64>,
}

fn sub2api_rate_updates(
    relay: &Relay,
    groups: Vec<api::Group>,
    user_rates: HashMap<i64, f64>,
) -> Vec<RateUpdate> {
    groups
        .into_iter()
        .map(|group| {
            let rate_multiplier = user_rates
                .get(&group.id)
                .copied()
                .filter(|rate| rate.is_finite())
                .or_else(|| {
                    group
                        .rate_multiplier
                        .is_finite()
                        .then_some(group.rate_multiplier)
                });
            RateUpdate {
                provider_id: provision::provider_id_for(
                    &relay.site_origin,
                    relay.account_id,
                    group.id,
                ),
                rate_multiplier,
            }
        })
        .collect()
}

fn newapi_rate_updates(relay: &Relay, groups: Vec<newapi::Group>) -> Vec<RateUpdate> {
    let account_id = relay
        .account_id
        .expect("authenticated relay required for NewAPI pricing");
    groups
        .into_iter()
        .map(|group| RateUpdate {
            provider_id: provision::newapi_provider_id_for(
                &relay.site_origin,
                account_id,
                &group.identity.0,
            ),
            rate_multiplier: group.rate_multiplier.filter(|rate| rate.is_finite()),
        })
        .collect()
}

pub async fn fetch_rate_updates(relay: &Relay) -> Result<Vec<RateUpdate>, AppError> {
    let account_id = relay
        .account_id
        .ok_or_else(|| AppError::InvalidInput("未登录中转站不能刷新倍率".into()))?;
    match relay.backend_kind {
        BackendKind::Sub2Api => {
            let client = api::Client::new(
                &relay.site_origin,
                &relay.auth_token,
                Some(account_id),
                relay.user_agent.as_deref(),
                relay.cf_clearance.as_deref(),
            )?;
            let groups = client.list_groups().await?;
            let user_rates = match client.user_group_rates().await {
                Ok(rates) => rates,
                Err(error) => {
                    log::warn!(
                        "中转站 {} 获取用户专属倍率失败，回落分组默认倍率: {error}",
                        relay.site_origin
                    );
                    HashMap::new()
                }
            };
            Ok(sub2api_rate_updates(relay, groups, user_rates))
        }
        BackendKind::NewApi => {
            let client = newapi::NewApiClient::with_account_id(
                &relay.site_origin,
                &relay.auth_token,
                account_id,
            )?;
            Ok(newapi_rate_updates(relay, client.groups().await?))
        }
    }
}

pub fn apply_rate_updates(db: &Database, updates: &[RateUpdate]) -> Result<usize, AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let mut changed = 0;
    for update in updates {
        changed += tx.execute(
            "UPDATE providers SET tier_rate_multiplier = ?1 WHERE id = ?2",
            params![update.rate_multiplier, update.provider_id],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::relay::{
        api,
        backend::BackendKind,
        creds::Relay,
        newapi::{Group as NewApiGroup, GroupIdentity},
        provision,
    };
    use axum::{
        extract::State,
        http::{Method, Uri},
        response::IntoResponse,
        routing::get,
        Json, Router,
    };

    fn relay(backend_kind: BackendKind) -> Relay {
        Relay {
            id: 1,
            site_origin: "https://relay.example".into(),
            site_name: "Relay".into(),
            backend_kind,
            api_base_url: "https://relay.example/v1".into(),
            account_id: Some(7),
            account_label: "Account".into(),
            login_identifier: "user@example.com".into(),
            auth_token: "access-token".into(),
            refresh_token: None,
            token_expires_at: None,
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
            sort_index: 0,
        }
    }

    async fn spawn_read_only_server(
        backend_kind: BackendKind,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        type Requests = Arc<Mutex<Vec<String>>>;

        async fn sub2api_groups(State(requests): State<Requests>) -> Json<serde_json::Value> {
            requests
                .lock()
                .unwrap()
                .push("GET /api/v1/groups/available".into());
            Json(serde_json::json!({
                "code": 0,
                "message": "success",
                "data": [{
                    "id": 42,
                    "name": "OpenAI",
                    "platform": "openai",
                    "rate_multiplier": 1.2,
                    "status": "active"
                }]
            }))
        }

        async fn sub2api_rates(State(requests): State<Requests>) -> Json<serde_json::Value> {
            requests
                .lock()
                .unwrap()
                .push("GET /api/v1/groups/rates".into());
            Json(serde_json::json!({
                "code": 0,
                "message": "success",
                "data": {"42": 0.8}
            }))
        }

        async fn newapi_groups(State(requests): State<Requests>) -> Json<serde_json::Value> {
            requests
                .lock()
                .unwrap()
                .push("GET /api/user/self/groups".into());
            Json(serde_json::json!({
                "success": true,
                "message": "",
                "data": {"Pro / 特价": {"ratio": 0.75, "desc": "paid"}}
            }))
        }

        async fn unexpected(
            State(requests): State<Requests>,
            method: Method,
            uri: Uri,
        ) -> impl IntoResponse {
            requests
                .lock()
                .unwrap()
                .push(format!("{method} {}", uri.path()));
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = match backend_kind {
            BackendKind::Sub2Api => Router::new()
                .route("/api/v1/groups/available", get(sub2api_groups))
                .route("/api/v1/groups/rates", get(sub2api_rates)),
            BackendKind::NewApi => Router::new().route("/api/user/self/groups", get(newapi_groups)),
        }
        .fallback(unexpected)
        .with_state(Arc::clone(&requests));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (origin, requests)
    }

    #[test]
    fn sub2api_rates_map_to_existing_provider_ids_without_keys() {
        let relay = relay(BackendKind::Sub2Api);
        let groups = vec![api::Group {
            id: 42,
            name: "OpenAI".into(),
            platform: "openai".into(),
            rate_multiplier: 1.2,
            status: "active".into(),
            allow_image_generation: false,
        }];
        let user_rates = HashMap::from([(42, 0.8)]);

        let updates = sub2api_rate_updates(&relay, groups, user_rates);

        assert_eq!(
            updates,
            vec![RateUpdate {
                provider_id: provision::provider_id_for(&relay.site_origin, Some(7), 42),
                rate_multiplier: Some(0.8),
            }]
        );
    }

    #[test]
    fn sub2api_rates_fall_back_to_the_finite_group_default() {
        let relay = relay(BackendKind::Sub2Api);
        let groups = vec![api::Group {
            id: 42,
            name: "OpenAI".into(),
            platform: "openai".into(),
            rate_multiplier: 1.2,
            status: "active".into(),
            allow_image_generation: false,
        }];

        let updates = sub2api_rate_updates(&relay, groups, HashMap::new());

        assert_eq!(updates[0].rate_multiplier, Some(1.2));
    }

    #[test]
    fn newapi_rates_use_raw_group_identity() {
        let relay = relay(BackendKind::NewApi);
        let groups = vec![NewApiGroup {
            identity: GroupIdentity("Pro / 特价".into()),
            name: "Pro".into(),
            rate_multiplier: Some(0.75),
            description: String::new(),
        }];

        let updates = newapi_rate_updates(&relay, groups);

        assert_eq!(
            updates[0],
            RateUpdate {
                provider_id: provision::newapi_provider_id_for(&relay.site_origin, 7, "Pro / 特价"),
                rate_multiplier: Some(0.75),
            }
        );
    }

    #[tokio::test]
    async fn sub2api_fetch_reads_only_groups_and_rates() {
        let (origin, requests) = spawn_read_only_server(BackendKind::Sub2Api).await;
        let mut relay = relay(BackendKind::Sub2Api);
        relay.site_origin = origin;

        let updates = fetch_rate_updates(&relay).await.unwrap();

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].rate_multiplier, Some(0.8));
        assert_eq!(
            *requests.lock().unwrap(),
            ["GET /api/v1/groups/available", "GET /api/v1/groups/rates"]
        );
    }

    #[tokio::test]
    async fn newapi_fetch_reads_only_groups() {
        let (origin, requests) = spawn_read_only_server(BackendKind::NewApi).await;
        let mut relay = relay(BackendKind::NewApi);
        relay.site_origin = origin;

        let updates = fetch_rate_updates(&relay).await.unwrap();

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].rate_multiplier, Some(0.75));
        assert_eq!(*requests.lock().unwrap(), ["GET /api/user/self/groups"]);
    }

    #[test]
    fn applying_rates_updates_only_existing_multiplier_columns() -> Result<(), AppError> {
        let db = Database::memory().unwrap();
        let conn = lock_conn!(db.conn);
        conn.execute_batch(
            "INSERT INTO providers
                (id, app_type, name, settings_config, website_url, category, notes, meta, is_current, tier_rate_multiplier)
             VALUES
                ('managed-a', 'codex', 'Codex', '{\"token\":\"keep\"}', 'https://a.example', 'relay', 'note-a', '{\"source\":\"keep\"}', 1, 1.5),
                ('managed-a', 'claude', 'Claude', '{\"token\":\"keep-too\"}', 'https://a.example', 'relay', 'note-b', '{\"source\":\"keep-too\"}', 0, 1.6),
                ('unrelated', 'codex', 'Other', '{\"token\":\"other\"}', 'https://b.example', 'manual', 'note-c', '{\"source\":\"other\"}', 0, 2.0);",
        )
        .unwrap();
        let before: Vec<(String, String, String, String, String, i64)> = conn
            .prepare(
                "SELECT id, app_type, settings_config, meta, notes, is_current
                 FROM providers ORDER BY id, app_type",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(conn);

        let changed = apply_rate_updates(
            &db,
            &[
                RateUpdate {
                    provider_id: "managed-a".into(),
                    rate_multiplier: Some(0.8),
                },
                RateUpdate {
                    provider_id: "missing".into(),
                    rate_multiplier: Some(0.4),
                },
            ],
        )
        .unwrap();

        assert_eq!(changed, 2);
        let conn = lock_conn!(db.conn);
        let after: Vec<(String, String, String, String, String, i64)> = conn
            .prepare(
                "SELECT id, app_type, settings_config, meta, notes, is_current
                 FROM providers ORDER BY id, app_type",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(
            conn.query_row(
                "SELECT tier_rate_multiplier FROM providers WHERE id='managed-a' AND app_type='codex'",
                [],
                |row| row.get::<_, Option<f64>>(0),
            )
            .unwrap(),
            Some(0.8)
        );
        assert_eq!(
            conn.query_row(
                "SELECT tier_rate_multiplier FROM providers WHERE id='managed-a' AND app_type='claude'",
                [],
                |row| row.get::<_, Option<f64>>(0),
            )
            .unwrap(),
            Some(0.8)
        );
        assert_eq!(
            conn.query_row(
                "SELECT tier_rate_multiplier FROM providers WHERE id='unrelated' AND app_type='codex'",
                [],
                |row| row.get::<_, Option<f64>>(0),
            )
            .unwrap(),
            Some(2.0)
        );
        Ok(())
    }
}
