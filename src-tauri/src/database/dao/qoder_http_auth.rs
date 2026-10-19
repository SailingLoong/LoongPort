//! Private subordinate credential storage; no UI/public Provider DTO access.
use crate::secrets::{
    inventory::{open_db, seal_db},
    VaultContext,
};
use crate::services::provider::qoder_daily::http::PatDeclaration;
use crate::services::provider::qoder_http_auth::*;
use crate::{database::Database, error::AppError, provider::Provider};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use zeroize::Zeroizing;

pub(crate) const COLUMN: &str = "qoder_http_secret";
pub(crate) const INTRODUCED_VERSION: i32 = 27;

pub(crate) fn has_column(conn: &Connection) -> Result<bool, AppError> {
    let mut statement = conn.prepare("PRAGMA table_xinfo(providers)")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == COLUMN {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(crate) fn create_column(conn: &Connection) -> Result<(), AppError> {
    if !has_column(conn)? {
        conn.execute_batch(
            "ALTER TABLE providers ADD COLUMN qoder_http_secret TEXT NOT NULL DEFAULT '';",
        )?;
    }
    validate_column(conn)
}
pub(crate) fn validate_column(conn: &Connection) -> Result<(), AppError> {
    let mut statement = conn.prepare("PRAGMA table_xinfo(providers)")?;
    let rows = statement.query_map([], |r| {
        Ok((
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;
    for row in rows {
        let (name, kind, not_null, default, primary_key, hidden) = row?;
        if name == COLUMN {
            return if kind.eq_ignore_ascii_case("TEXT")
                && not_null == 1
                && default.as_deref() == Some("''")
                && primary_key == 0
                && hidden == 0
            {
                Ok(())
            } else {
                Err(denied("invalid_schema"))
            };
        }
    }
    Err(denied("schema_migration_required"))
}
/// Every snapshot import/restore must discard ALL derived states, including Ready.
/// An internal stored generation cannot establish that a backup refresh is unused.
pub(crate) fn retire_all_on(conn: &Connection) -> Result<(), AppError> {
    validate_column(conn)?;
    conn.execute(
        "UPDATE providers SET qoder_http_secret='' WHERE qoder_http_secret<>''",
        [],
    )?;
    Ok(())
}
pub(crate) fn retire_on(conn: &Connection, app: &str, id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE providers SET qoder_http_secret='' WHERE id=?1 AND app_type=?2",
        params![id, app],
    )?;
    Ok(())
}

fn read_on(
    conn: &Connection,
    vault: &VaultContext,
    id: &str,
) -> Result<(String, Option<Record>), AppError> {
    let raw: String = conn.query_row(
        "SELECT qoder_http_secret FROM providers WHERE id=?1 AND app_type='codex'",
        [id],
        |r| r.get(0),
    )?;
    let plaintext = Zeroizing::new(open_db(vault, "providers", COLUMN, &[id, "codex"], &raw)?);
    Ok((raw, Record::decode(&plaintext)?))
}
fn publish_on(
    conn: &Connection,
    vault: &VaultContext,
    id: &str,
    previous: &str,
    next: &Record,
) -> Result<(), AppError> {
    let plaintext =
        Zeroizing::new(serde_json::to_string(next).map_err(|_| denied("invalid_record"))?);
    let sealed = seal_db(vault, "providers", COLUMN, &[id, "codex"], &plaintext)?;
    let changed = conn.execute("UPDATE providers SET qoder_http_secret=?1 WHERE id=?2 AND app_type='codex' AND qoder_http_secret=?3", params![sealed, id, previous])?;
    if changed != 1 {
        return Err(denied("stale_owner"));
    }
    Ok(())
}
fn source_matches(record: &Record, source: &SourceBinding) -> Result<(), AppError> {
    if record.source != *source {
        return Err(denied("source_changed"));
    }
    Ok(())
}
fn stamp_matches(
    record: &Record,
    source: &SourceBinding,
    epoch: u64,
    stamp: &Stamp,
) -> Result<(), AppError> {
    source_matches(record, source)?;
    if stamp.source != *source
        || record.generation != stamp.generation
        || epoch != stamp.vault_epoch
    {
        return Err(denied("stale_owner"));
    }
    Ok(())
}
fn snapshot(record: &Record, epoch: u64) -> Result<RequestSnapshot, AppError> {
    let Phase::Ready { identity, tokens } = &record.phase else {
        return Err(denied("reestablish_required"));
    };
    Ok(RequestSnapshot::new(
        Stamp {
            source: record.source.clone(),
            generation: record.generation,
            vault_epoch: epoch,
        },
        identity.clone(),
        tokens,
    ))
}

impl Database {
    /// All owner operations have the existing lock order: vault, connection,
    /// immediate transaction. The returned lease never contains a lock guard.
    fn with_qoder_http<T>(
        &self,
        provider_id: &str,
        body: impl FnOnce(
            &Transaction<'_>,
            &VaultContext,
            u64,
            Provider,
            SourceBinding,
            Zeroizing<String>,
        ) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let vault = self.secrets.read()?;
        let epoch = self.secrets.lifecycle_epoch();
        let mut conn = self.conn.lock()?;
        validate_column(&conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let provider = Self::get_provider_by_id_on_connection(&tx, &vault, provider_id, "codex")?
            .ok_or_else(|| denied("source_changed"))?;
        let (source, pat) = SourceBinding::from_provider(&provider)?;
        let result = body(&tx, &vault, epoch, provider, source, pat)?;
        self.secrets.ensure_available()?;
        if self.secrets.lifecycle_epoch() != epoch {
            return Err(denied("stale_owner"));
        }
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn qoder_http_begin_establish(
        &self,
        declaration: &PatDeclaration,
        intent: EstablishIntent,
    ) -> Result<EstablishLease, AppError> {
        self.with_qoder_http(
            &declaration.provider_id,
            |tx, vault, epoch, _, source, pat| {
                source.admit(declaration)?;
                let (raw, previous) = read_on(tx, vault, &source.provider_id)?;
                if let Some(previous) = &previous {
                    source_matches(previous, &source)?;
                    if matches!(
                        previous.phase,
                        Phase::Establishing { .. } | Phase::RefreshPending { .. }
                    ) {
                        return Err(denied("attempt_in_progress"));
                    }
                }
                let generation = previous
                    .as_ref()
                    .map(Record::next_generation)
                    .transpose()?
                    .unwrap_or(1);
                let (identity, mut machine_id) = previous
                    .as_ref()
                    .map(Record::identity_and_machine)
                    .unwrap_or_else(|| (None, uuid::Uuid::new_v4().to_string()));
                let expected_uid = match intent {
                    EstablishIntent::SameIdentity => identity.map(|i| i.uid),
                    EstablishIntent::Rebind { previous_uid } => {
                        if identity.as_ref().map(|i| i.uid.as_str()) != Some(previous_uid.as_str())
                        {
                            return Err(denied("rebind_subject_changed"));
                        }
                        machine_id = uuid::Uuid::new_v4().to_string();
                        None
                    }
                };
                let attempt_id = uuid::Uuid::new_v4().to_string();
                let record = Record::new(
                    source.clone(),
                    generation,
                    Phase::Establishing {
                        attempt_id: attempt_id.clone(),
                        expected_uid: expected_uid.clone(),
                        machine_id: machine_id.clone(),
                    },
                );
                publish_on(tx, vault, &source.provider_id, &raw, &record)?;
                Ok(EstablishLease {
                    stamp: Stamp {
                        source,
                        generation,
                        vault_epoch: epoch,
                    },
                    attempt_id,
                    expected_uid,
                    machine_id,
                    pat,
                })
            },
        )
    }

    pub(crate) fn qoder_http_acquire(
        &self,
        expected: &SourceBinding,
        now_ms: i64,
    ) -> Result<Acquisition, AppError> {
        if now_ms < 0 {
            return Err(denied("clock_invalid"));
        }
        let provider_id = &expected.provider_id;
        self.with_qoder_http(provider_id, |tx, vault, epoch, _, source, _| {
            // Bind the request's already-selected Provider before spending a
            // refresh for the current row. This comparison and Pending CAS
            // share the same original Immediate transaction (no TOCTOU gap).
            if source != *expected {
                return Err(denied("selected_source_changed"));
            }
            let (raw, record) = read_on(tx, vault, provider_id)?;
            let record = record.ok_or_else(|| denied("source_declaration_required"))?;
            source_matches(&record, &source)?;
            let Phase::Ready { identity, tokens } = &record.phase else {
                return Err(denied(match &record.phase {
                    Phase::Establishing { .. } | Phase::RefreshPending { .. } => {
                        "attempt_in_progress"
                    }
                    _ => "outcome_unknown_reestablish_required",
                }));
            };
            if tokens.expires_at_ms > now_ms.saturating_add(REFRESH_MARGIN_MS) {
                return Ok(Acquisition::Ready(snapshot(&record, epoch)?));
            }
            // PAT exchange may legitimately omit a refresh token. It is usable
            // until the refresh boundary, then needs explicit establishment.
            if tokens.refresh_token.trim().is_empty() {
                return Err(denied("reestablish_required"));
            }
            let generation = record.next_generation()?;
            let attempt_id = uuid::Uuid::new_v4().to_string();
            let lease = RefreshLease {
                stamp: Stamp {
                    source: source.clone(),
                    generation,
                    vault_epoch: epoch,
                },
                attempt_id: attempt_id.clone(),
                identity: identity.clone(),
                refresh_token: Zeroizing::new(tokens.refresh_token.clone()),
            };
            let pending = Record::new(
                source,
                generation,
                Phase::RefreshPending {
                    attempt_id,
                    identity: identity.clone(),
                },
            );
            publish_on(tx, vault, provider_id, &raw, &pending)?;
            Ok(Acquisition::Refresh(lease))
        })
    }

    pub(crate) fn qoder_http_publish_establish(
        &self,
        lease: &EstablishLease,
        response: Established,
        now_ms: i64,
    ) -> Result<RequestSnapshot, AppError> {
        validate_tokens(&response.tokens, now_ms)?;
        if response.uid.trim().is_empty()
            || lease
                .expected_uid
                .as_ref()
                .is_some_and(|uid| *uid != response.uid)
        {
            return Err(denied("identity_rebind_required"));
        }
        self.with_qoder_http(
            &lease.stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (raw, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, &lease.stamp)?;
                match &record.phase {
                    Phase::Establishing {
                        attempt_id,
                        expected_uid,
                        machine_id,
                    } if *attempt_id == lease.attempt_id
                        && *expected_uid == lease.expected_uid
                        && *machine_id == lease.machine_id => {}
                    _ => return Err(denied("stale_owner")),
                }
                let ready = Record::new(
                    source,
                    record.next_generation()?,
                    Phase::Ready {
                        identity: Identity {
                            uid: response.uid,
                            machine_id: lease.machine_id.clone(),
                        },
                        tokens: response.tokens,
                    },
                );
                publish_on(tx, vault, &ready.source.provider_id, &raw, &ready)?;
                snapshot(&ready, epoch)
            },
        )
    }

    pub(crate) fn qoder_http_publish_refresh(
        &self,
        lease: &RefreshLease,
        tokens: TokenPair,
        now_ms: i64,
    ) -> Result<RequestSnapshot, AppError> {
        validate_tokens(&tokens, now_ms)?;
        self.with_qoder_http(
            &lease.stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (raw, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, &lease.stamp)?;
                match &record.phase {
                    Phase::RefreshPending {
                        attempt_id,
                        identity,
                    } if *attempt_id == lease.attempt_id
                        && identity.uid == lease.identity.uid
                        && identity.machine_id == lease.identity.machine_id => {}
                    _ => return Err(denied("stale_owner")),
                }
                let ready = Record::new(
                    source,
                    record.next_generation()?,
                    Phase::Ready {
                        identity: lease.identity.clone(),
                        tokens,
                    },
                );
                publish_on(tx, vault, &ready.source.provider_id, &raw, &ready)?;
                snapshot(&ready, epoch)
            },
        )
    }

    pub(crate) fn qoder_http_mark_unknown(
        &self,
        stamp: &Stamp,
        attempt: &str,
    ) -> Result<(), AppError> {
        self.with_qoder_http(
            &stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (raw, record) = read_on(tx, vault, &source.provider_id)?;
                let mut record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, stamp)?;
                match &record.phase {
                    Phase::Establishing { attempt_id, .. }
                    | Phase::RefreshPending { attempt_id, .. }
                        if attempt_id == attempt => {}
                    _ => return Err(denied("stale_owner")),
                }
                let (identity, machine_id) = record.identity_and_machine();
                record.phase = Phase::Unknown {
                    attempt_id: attempt.into(),
                    identity,
                    machine_id,
                };
                publish_on(tx, vault, &source.provider_id, &raw, &record)
            },
        )
    }

    /// Capture the exact pending attempt BEFORE asking for user recovery
    /// confirmation. This receipt has no frontend serialization or token fields.
    pub(crate) fn qoder_http_prepare_recovery(
        &self,
        declaration: &PatDeclaration,
    ) -> Result<RecoveryReceipt, AppError> {
        self.with_qoder_http(
            &declaration.provider_id,
            |tx, vault, epoch, _, source, _| {
                source.admit(declaration)?;
                let (_, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("reestablish_required"))?;
                source_matches(&record, &source)?;
                let attempt_id = match &record.phase {
                    Phase::Establishing { attempt_id, .. }
                    | Phase::RefreshPending { attempt_id, .. } => attempt_id.clone(),
                    _ => return Err(denied("no_pending_attempt")),
                };
                Ok(RecoveryReceipt {
                    stamp: Stamp {
                        source,
                        generation: record.generation,
                        vault_epoch: epoch,
                    },
                    attempt_id,
                })
            },
        )
    }

    /// Only an explicit confirmation tied to this captured receipt may abandon
    /// a submitted attempt. Source, original epoch, generation AND attempt ID
    /// are checked under the original transaction. Late approval cannot affect
    /// a newer attempt on the same Provider/PAT.
    pub(crate) fn qoder_http_abandon_attempt(
        &self,
        receipt: &RecoveryReceipt,
    ) -> Result<(), AppError> {
        self.qoder_http_mark_unknown(&receipt.stamp, &receipt.attempt_id)
    }

    pub(crate) fn check_qoder_http_snapshot(
        &self,
        snapshot: &RequestSnapshot,
        now_ms: i64,
    ) -> Result<(), AppError> {
        if now_ms < 0 || snapshot.expires_at_ms() <= now_ms {
            return Err(denied("snapshot_expired"));
        }
        self.check_qoder_http_snapshot_fence(snapshot)
    }

    /// In-flight response authorization: natural access-token expiry only gates
    /// new submissions. Explicit owner/source/epoch/generation changes still
    /// revoke already-submitted responses through the existing response lease.
    pub(crate) fn check_qoder_http_snapshot_fence(
        &self,
        snapshot: &RequestSnapshot,
    ) -> Result<(), AppError> {
        self.with_qoder_http(
            &snapshot.stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (_, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, &snapshot.stamp)?;
                match record.phase {
                    Phase::Ready { identity, tokens }
                        if identity.uid == snapshot.uid()
                            && identity.machine_id == snapshot.machine_id()
                            && tokens.access_token == snapshot.access_token() =>
                    {
                        Ok(())
                    }
                    _ => Err(denied("stale_owner")),
                }
            },
        )
    }
}
