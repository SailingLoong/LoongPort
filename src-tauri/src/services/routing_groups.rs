//! Reference-only shared routing group owner consumed by the existing native router.
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS loongport_routing_groups(id TEXT PRIMARY KEY NOT NULL,name TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision > 0),members TEXT NOT NULL); CREATE TABLE IF NOT EXISTS loongport_application_route_bindings(app_type TEXT PRIMARY KEY NOT NULL,target TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision > 0));";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Member {
    pub source_id: String,
    pub model: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub id: String,
    pub revision: i64,
    pub members: Vec<Member>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Conflict,
    Invalid,
    Unresolved,
    Storage,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Candidate {
    pub provider_id: String,
    pub model: String,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    pub revision: i64,
    pub candidates: Vec<Candidate>,
}
// Explicit migration contribution only: reads and writes never create schema.
// Only the existing fresh-schema/migration owners may install SCHEMA.
pub(crate) fn validate(id: &str, members: &[Member]) -> Result<(), Error> {
    let canonical = |value: &str| !value.is_empty() && value.trim() == value;
    if !canonical(id) || members.is_empty() {
        return Err(Error::Invalid);
    }
    let mut seen = HashSet::new();
    for member in members {
        if !canonical(&member.source_id)
            || !canonical(&member.model)
            || !seen.insert((&member.source_id, &member.model))
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

/// Create with no expected revision, or replace exactly the reviewed revision.
/// The connection belongs to the existing database owner; no second database is opened.
pub fn save(
    db: &mut Connection,
    id: &str,
    expected: Option<i64>,
    members: &[Member],
) -> Result<Group, Error> {
    save_named(db, id, expected, id, members)
}
pub fn save_named(
    db: &mut Connection,
    id: &str,
    expected: Option<i64>,
    name: &str,
    members: &[Member],
) -> Result<Group, Error> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| Error::Storage)?;
    let result = save_named_on(&tx, id, expected, name, members)?;
    tx.commit().map_err(|_| Error::Storage)?;
    Ok(result)
}

pub(crate) fn save_named_on(
    db: &Connection,
    id: &str,
    expected: Option<i64>,
    name: &str,
    members: &[Member],
) -> Result<Group, Error> {
    if name.trim().is_empty() || name.trim() != name {
        return Err(Error::Invalid);
    }
    validate(id, members)?;
    if expected.is_some_and(|revision| revision <= 0) {
        return Err(Error::Invalid);
    }
    let revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Invalid)?;
    let encoded = serde_json::to_string(members).map_err(|_| Error::Storage)?;
    let changed = if let Some(expected) = expected {
        db.execute("UPDATE loongport_routing_groups SET revision=?1, members=?2,name=?5 WHERE id=?3 AND revision=?4",
            rusqlite::params![revision, encoded, id, expected,name])
    } else {
        db.execute("INSERT INTO loongport_routing_groups(id,revision,members,name) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
            rusqlite::params![id, revision, encoded,name])
    }.map_err(|_| Error::Storage)?;
    if changed != 1 {
        return Err(Error::Conflict);
    }
    let result = Group {
        name: name.into(),
        id: id.into(),
        revision,
        members: members.to_vec(),
    };
    if read(db, id)?.as_ref() != Some(&result) {
        return Err(Error::Storage);
    }
    Ok(result)
}

/// Missing is distinct from corrupt/unavailable. No migration or refresh on read.
pub fn read(db: &Connection, id: &str) -> Result<Option<Group>, Error> {
    let stored = db
        .query_row(
            "SELECT revision,members,name FROM loongport_routing_groups WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    let Some((revision, encoded, name)) = stored else {
        return Ok(None);
    };
    let members: Vec<Member> = serde_json::from_str(&encoded).map_err(|_| Error::Storage)?;
    if revision <= 0
        || name.trim().is_empty()
        || name.trim() != name
        || validate(id, &members).is_err()
    {
        return Err(Error::Storage);
    }
    Ok(Some(Group {
        name,
        id: id.into(),
        revision,
        members,
    }))
}

/// Called only by explicit fresh-schema/migration owners, never by reads.
/// A verified native source may map to distinct original rows in each app.
pub(crate) struct ResolvedReference {
    pub source_id: String,
    pub provider_id: String,
}
pub(crate) fn resolve_references(
    group: Group,
    mut reference: impl FnMut(&Member) -> Result<ResolvedReference, Error>,
) -> Result<Resolved, Error> {
    let mut candidates = Vec::with_capacity(group.members.len());
    for member in group.members {
        let mapped = reference(&member)?;
        if mapped.source_id != member.source_id || mapped.provider_id.is_empty() {
            return Err(Error::Unresolved);
        }
        candidates.push(Candidate {
            provider_id: mapped.provider_id,
            model: member.model,
        });
    }
    Ok(Resolved {
        revision: group.revision,
        candidates,
    })
}

pub fn create_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RouteTarget {
    Manual {
        #[serde(rename = "sourceId")]
        source_id: String,
        model: String,
    },
    OrderedGroup {
        #[serde(rename = "groupId")]
        group_id: String,
    },
}
impl RouteTarget {
    pub fn validate(&self) -> Result<(), Error> {
        let canonical = |v: &str| !v.is_empty() && v.trim() == v;
        match self {
            Self::Manual { source_id, model } if canonical(source_id) && canonical(model) => Ok(()),
            Self::OrderedGroup { group_id } if canonical(group_id) => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub target: RouteTarget,
    pub revision: i64,
}
pub fn binding(db: &Connection, app: &str) -> Result<Option<Binding>, Error> {
    let found = db
        .query_row(
            "SELECT target,revision FROM loongport_application_route_bindings WHERE app_type=?1",
            [app],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    found
        .map(|(encoded, revision)| {
            let target: RouteTarget = serde_json::from_str(&encoded).map_err(|_| Error::Storage)?;
            target.validate().map_err(|_| Error::Storage)?;
            if revision <= 0 {
                return Err(Error::Storage);
            }
            Ok(Binding { target, revision })
        })
        .transpose()
}
pub fn bindings(db: &Connection) -> Result<Vec<(String, Binding)>, Error> {
    let mut statement = db
        .prepare("SELECT app_type FROM loongport_application_route_bindings ORDER BY app_type")
        .map_err(|_| Error::Storage)?;
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| Error::Storage)?;
    ids.map(|id| {
        let id = id.map_err(|_| Error::Storage)?;
        let value = binding(db, &id)?.ok_or(Error::Storage)?;
        Ok((id, value))
    })
    .collect()
}
pub fn list(db: &Connection) -> Result<Vec<Group>, Error> {
    let mut statement = db
        .prepare("SELECT id FROM loongport_routing_groups ORDER BY id")
        .map_err(|_| Error::Storage)?;
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| Error::Storage)?;
    ids.map(|id| read(db, &id.map_err(|_| Error::Storage)?)?.ok_or(Error::Storage))
        .collect()
}
pub fn bind(
    db: &mut Connection,
    app: &str,
    group_id: &str,
    expected: Option<i64>,
    group_revision: i64,
) -> Result<Binding, Error> {
    set_target(
        db,
        app,
        RouteTarget::OrderedGroup {
            group_id: group_id.into(),
        },
        expected,
        Some(group_revision),
    )
}
pub fn set_target(
    db: &mut Connection,
    app: &str,
    target: RouteTarget,
    expected: Option<i64>,
    group_revision: Option<i64>,
) -> Result<Binding, Error> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| Error::Storage)?;
    let result = set_target_on(&tx, app, target, expected, group_revision)?;
    tx.commit().map_err(|_| Error::Storage)?;
    Ok(result)
}

pub(crate) fn set_target_on(
    db: &Connection,
    app: &str,
    target: RouteTarget,
    expected: Option<i64>,
    group_revision: Option<i64>,
) -> Result<Binding, Error> {
    if app.is_empty() || app.trim() != app || expected.is_some_and(|r| r <= 0) {
        return Err(Error::Invalid);
    }
    target.validate()?;
    let revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Invalid)?;
    match &target {
        RouteTarget::OrderedGroup { group_id } => {
            if read(db, group_id)?.is_none_or(|g| Some(g.revision) != group_revision) {
                return Err(Error::Conflict);
            }
        }
        RouteTarget::Manual { .. } => {
            if group_revision.is_some() {
                return Err(Error::Invalid);
            }
        }
    }
    let encoded = serde_json::to_string(&target).map_err(|_| Error::Invalid)?;
    let changed=if let Some(expected)=expected {
        db.execute("UPDATE loongport_application_route_bindings SET target=?1,revision=?2 WHERE app_type=?3 AND revision=?4",rusqlite::params![encoded,revision,app,expected])
    } else {db.execute("INSERT INTO loongport_application_route_bindings(app_type,target,revision) VALUES(?1,?2,?3) ON CONFLICT(app_type) DO NOTHING",rusqlite::params![app,encoded,revision])}.map_err(|_|Error::Storage)?;
    if changed != 1 {
        return Err(Error::Conflict);
    }
    let result = Binding { target, revision };
    if binding(db, app)?.as_ref() != Some(&result) {
        return Err(Error::Storage);
    }
    Ok(result)
}
/// Full column probe and data validation; readers never repair installed state.
pub fn validate_schema(db: &Connection) -> Result<(), Error> {
    db.prepare("SELECT id,name,revision,members FROM loongport_routing_groups LIMIT 0")
        .map_err(|_| Error::Storage)?;
    db.prepare("SELECT app_type,target,revision FROM loongport_application_route_bindings LIMIT 0")
        .map_err(|_| Error::Storage)?;
    list(db)?;
    for (app, bound) in bindings(db)? {
        if app.trim().is_empty() || app.trim() != app {
            return Err(Error::Storage);
        }
        if let RouteTarget::OrderedGroup { group_id } = bound.target {
            read(db, &group_id)?.ok_or(Error::Storage)?;
        }
    }
    Ok(())
}
