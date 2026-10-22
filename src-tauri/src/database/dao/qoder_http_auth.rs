//! Private subordinate credential storage; no UI/public Provider DTO access.
use crate::secrets::{
    inventory::{open_db, seal_db},
    VaultContext,
};
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

// Shared original reservation body: explicit recovery changes admission, not
// the identity/PAT owner or how establishment gets its durable lease.
fn next_establishment(
    source: SourceBinding,
    previous: Option<&Record>,
    epoch: u64,
    pat: Zeroizing<String>,
    intent: EstablishIntent,
) -> Result<(Record, EstablishLease), AppError> {
    let generation = previous
        .map(Record::next_generation)
        .transpose()?
        .unwrap_or(1);
    let (identity, mut machine_id) = previous
        .map(Record::identity_and_machine)
        .unwrap_or_else(|| (None, uuid::Uuid::new_v4().to_string()));
    let expected_uid = match intent {
        EstablishIntent::SameIdentity => identity.map(|i| i.uid),
        EstablishIntent::Rebind { previous_uid } => {
            if identity.as_ref().map(|i| i.uid.as_str()) != Some(previous_uid.as_str()) {
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
    let lease = EstablishLease {
        stamp: Stamp {
            source,
            generation,
            vault_epoch: epoch,
        },
        attempt_id,
        expected_uid,
        machine_id,
        pat,
    };
    Ok((record, lease))
}

fn establish_phase_matches(record: &Record, lease: &EstablishLease) -> Result<(), AppError> {
    match &record.phase {
        Phase::Establishing {
            attempt_id,
            expected_uid,
            machine_id,
        } if *attempt_id == lease.attempt_id
            && *expected_uid == lease.expected_uid
            && *machine_id == lease.machine_id =>
        {
            Ok(())
        }
        _ => Err(denied("stale_owner")),
    }
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

    pub(crate) fn qoder_http_identity_context(
        &self,
        provider_id: &str,
        now_ms: i64,
    ) -> Result<IdentityContext, AppError> {
        self.with_qoder_http(provider_id, |tx, vault, _, _, source, _| {
            let (_, record) = read_on(tx, vault, provider_id)?;
            if let Some(record) = &record {
                source_matches(record, &source)?;
            }
            Ok(IdentityContext::from_record(
                &source,
                record.as_ref(),
                now_ms,
            ))
        })
    }

    /// Read-only private preparation; no confirmation token or credential leaves
    /// the existing service task. The source is loaded from the original row.
    pub(crate) fn qoder_http_prepare_identity_action(
        &self,
        request: &IdentityActionRequest,
        now_ms: i64,
    ) -> Result<PreparedIdentityAction, AppError> {
        if now_ms < 0 {
            return Err(denied("clock_invalid"));
        }
        self.with_qoder_http(
            &request.declaration.provider_id,
            |tx, vault, epoch, provider, source, _| {
                source.admit(&request.declaration)?;
                let (_, record) = read_on(tx, vault, &source.provider_id)?;
                if let Some(record) = &record {
                    source_matches(record, &source)?;
                }
                let context = IdentityContext::from_record(&source, record.as_ref(), now_ms);
                let (intent, confirmation) = match request.action {
                    IdentityAction::Establish => match context.status {
                        IdentityStatus::Ready | IdentityStatus::RefreshNeeded => {
                            return Ok(PreparedIdentityAction::Complete(context))
                        }
                        IdentityStatus::Unestablished | IdentityStatus::ReestablishRequired => {
                            (EstablishIntent::SameIdentity, None)
                        }
                        _ => return Ok(PreparedIdentityAction::Changed(context)),
                    },
                    IdentityAction::Recover => {
                        if !matches!(
                            context.status,
                            IdentityStatus::Pending | IdentityStatus::Unknown
                        ) {
                            return Ok(PreparedIdentityAction::Changed(context));
                        }
                        (
                            EstablishIntent::SameIdentity,
                            Some(IdentityConfirmation {
                                kind: IdentityConfirmationKind::Recover,
                                provider_name: provider.name,
                            }),
                        )
                    }
                    IdentityAction::Rebind => {
                        let Some(identity) =
                            record.as_ref().and_then(|r| r.identity_and_machine().0)
                        else {
                            return Ok(PreparedIdentityAction::Changed(context));
                        };
                        (
                            EstablishIntent::Rebind {
                                previous_uid: identity.uid,
                            },
                            Some(IdentityConfirmation {
                                kind: IdentityConfirmationKind::Rebind,
                                provider_name: provider.name,
                            }),
                        )
                    }
                };
                Ok(PreparedIdentityAction::Establish {
                    receipt: IdentityActionReceipt {
                        source,
                        vault_epoch: epoch,
                        generation: record.as_ref().map(|r| r.generation),
                        phase: IdentityPhaseStamp::of(record.as_ref()),
                        intent,
                    },
                    confirmation,
                })
            },
        )
    }

    /// Exact receipt validation and replacement with Establishing are ONE
    /// original transaction. Never compose abandon_attempt then begin_establish.
    pub(crate) fn qoder_http_reserve_identity_action(
        &self,
        receipt: IdentityActionReceipt,
    ) -> Result<EstablishLease, AppError> {
        let provider_id = receipt.source.provider_id.clone();
        self.with_qoder_http(&provider_id, |tx, vault, epoch, _, source, pat| {
            if source != receipt.source {
                return Err(denied("source_changed"));
            }
            let (raw, record) = read_on(tx, vault, &provider_id)?;
            if let Some(record) = &record {
                source_matches(record, &source)?;
            }
            if epoch != receipt.vault_epoch
                || record.as_ref().map(|r| r.generation) != receipt.generation
                || IdentityPhaseStamp::of(record.as_ref()) != receipt.phase
            {
                return Err(denied("stale_owner"));
            }
            let (next, lease) =
                next_establishment(source, record.as_ref(), epoch, pat, receipt.intent)?;
            publish_on(tx, vault, &provider_id, &raw, &next)?;
            Ok(lease)
        })
    }

    /// Final pre-GET admission for ephemeral device sessions. A cancelled,
    /// replaced, locked, or cross-realm lease must not reach the poll transport.
    pub(crate) fn check_qoder_http_establish(
        &self,
        lease: &EstablishLease,
    ) -> Result<(), AppError> {
        self.with_qoder_http(
            &lease.stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (_, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, &lease.stamp)?;
                establish_phase_matches(&record, lease)
            },
        )
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
                establish_phase_matches(&record, lease)?;
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

    /// A proven device-auth rejection retires only the exact original request
    /// owner. A late 401/403 cannot revoke a newer grant or in-flight refresh.
    pub(crate) fn qoder_http_reject_device_snapshot(
        &self,
        snapshot: &RequestSnapshot,
    ) -> Result<(), AppError> {
        if snapshot.stamp.source.source != SourceKind::ChinaQoderDevice {
            return Err(denied("unsupported_source"));
        }
        self.with_qoder_http(
            &snapshot.stamp.source.provider_id,
            |tx, vault, epoch, _, source, _| {
                let (raw, record) = read_on(tx, vault, &source.provider_id)?;
                let record = record.ok_or_else(|| denied("stale_owner"))?;
                stamp_matches(&record, &source, epoch, &snapshot.stamp)?;
                let identity = match &record.phase {
                    Phase::Ready { identity, tokens }
                        if identity.uid == snapshot.uid()
                            && identity.machine_id == snapshot.machine_id()
                            && tokens.access_token == snapshot.access_token() =>
                    {
                        identity.clone()
                    }
                    _ => return Err(denied("stale_owner")),
                };
                let unknown = Record::new(
                    source,
                    record.next_generation()?,
                    Phase::Unknown {
                        attempt_id: uuid::Uuid::new_v4().to_string(),
                        machine_id: identity.machine_id.clone(),
                        identity: Some(identity),
                    },
                );
                publish_on(tx, vault, &unknown.source.provider_id, &raw, &unknown)
            },
        )
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
