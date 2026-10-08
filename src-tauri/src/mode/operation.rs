//! Multi-file intent, publication and restart recovery from cc-switch v4.0.2.
//! LoongPort adds encrypted journal I/O, trusted path/privacy admission and strict
//! file readback. External changes or missing staging retain the original pending
//! operation as verification-required; target state is not silently committed.
//! The caller holds the app lock and existing SecretSession read guard throughout.
//! Target callbacks are idempotent and must reuse that guard (no nested session read).

use std::fs;
use std::path::PathBuf;

use crate::config_file_io::commit_staged;
use crate::error::AppError;
use crate::live::engine::{
    digest, ensure_first_write_backup, plan, plan_from, read_current, stage, validate_file_path,
    AppWriteGuard, DeviceStore, LiveFile, Planned,
};
use crate::live::patch::{LivePatch, LiveWriteError};
use crate::secrets::VaultContext;
use std::collections::HashSet;
use std::sync::RwLockReadGuard;

use super::state::{self, Pending, PendingFile, PendingTarget};

/// 发布时发现文件被改过，最多以新内容为底重算几次。
const MAX_REPLANS: usize = 3;

/// 操作里的一个文件：以当前内容为底，用 `patch` 算出新内容。
pub(crate) struct FileChange<'a> {
    pub file: LiveFile,
    pub patch: &'a dyn LivePatch,
}

/// 文件都写完之后落定状态（比如改指针）。必须可以重复执行：崩溃恢复可能再跑一次。
pub(crate) type CommitTarget<'a> = &'a dyn Fn(&PendingTarget) -> Result<(), AppError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    /// 还没开始发布，丢弃了。
    Discarded,
    /// 补完了剩下的文件和状态。
    RolledForward,
    /// 补完了其余文件和状态；这些文件被外部改过（或临时文件不可用），保持原样。
    VerificationRequired { paths: Vec<PathBuf> },
    /// 还没开始发布，这些文件就被外部改过了：丢弃这次操作，什么都没改。
    Abandoned { paths: Vec<PathBuf> },
}

#[derive(Debug, Default)]
pub(crate) struct OperationReport {
    /// 实际改动的文件。
    pub changed: Vec<PathBuf>,
    /// 开始前补完或放弃的上一次未完成操作。
    pub recovered: Option<RecoveryOutcome>,
}

/// 执行一次操作。调用方持有这个应用的写锁。
pub(crate) fn run(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    op: &str,
    changes: &[FileChange<'_>],
    target: PendingTarget,
    commit_target: CommitTarget<'_>,
) -> Result<OperationReport, AppError> {
    let mut report = OperationReport {
        recovered: recover(
            store,
            vault,
            guard,
            &changes.iter().map(|c| c.file.clone()).collect::<Vec<_>>(),
            commit_target,
        )?,
        ..OperationReport::default()
    };

    if matches!(
        report.recovered,
        Some(RecoveryOutcome::VerificationRequired { .. })
    ) {
        return Err(verification_required());
    }
    validate_operation(op, &target)?;
    validate_admitted(&changes.iter().map(|c| c.file.clone()).collect::<Vec<_>>())?;

    // 1. 在内存里算好每个文件；任何一个解析失败都不写。
    let mut plans = Vec::with_capacity(changes.len());
    for change in changes {
        let planned = plan(&change.file, change.patch)?;
        plans.push((planned, change.patch));
    }

    // 2. 备好所有临时文件（要删的文件没有）；失败就清掉，什么都没改。
    let mut staged = Vec::with_capacity(plans.len());
    for (planned, _) in &plans {
        if planned.is_noop() {
            staged.push(None);
            continue;
        }
        match stage(planned) {
            Ok(write) => staged.push(write.map(|write| write.tmp_path().to_path_buf())),
            Err(err) => {
                discard_all(&staged);
                return Err(err);
            }
        }
    }
    failpoint::hit("staged")?;

    // 3. 写下意图。从这里起，失败都留着 pending 等前滚。
    let mut pending = Pending {
        op: op.to_string(),
        files: plans
            .iter()
            .zip(&staged)
            .map(|((planned, _), staged)| pending_file(planned, staged.clone()))
            .collect(),
        target,
        published: false,
        extra: Default::default(),
    };
    if let Err(err) = state::set_pending(store, vault, guard.app(), Some(pending.clone())) {
        discard_all(&staged);
        return Err(err);
    }
    failpoint::hit("pending")?;

    // 4. 逐个发布：rename 前最后重读一次，被改过就以新内容为底重算。
    let mut published_any = false;
    for (index, (planned, patch)) in plans.iter().enumerate() {
        // No-op snapshots remain readback witnesses, but are never rewritten.
        if planned.is_noop() {
            continue;
        }
        let mut current_planned = planned.clone();
        let mut replans = 0;
        loop {
            failpoint::before_publish(index, &current_planned.file.path);
            let current = read_current(&current_planned.file.path)?;
            if digest(current.as_deref()) == current_planned.pre {
                ensure_first_write_backup(
                    store,
                    vault,
                    &current_planned.file.path,
                    current.as_deref(),
                )?;
                // 换进第一个文件之前先记下「已开始发布」，记不下来就不发布：换进去之后才记的
                // 话，中间崩溃、这个文件又被客户端改掉（Codex 刷新登录），恢复时就分不出发布
                // 开始过没有，还没发布的登录暂存会被当成没用的丢掉。
                if !pending.published {
                    pending.published = true;
                    let marked = failpoint::hit("mark").and_then(|()| {
                        state::set_pending(store, vault, guard.app(), Some(pending.clone()))
                    });
                    if let Err(err) = marked {
                        drop_unpublished(store, vault, guard, &pending);
                        return Err(err);
                    }
                    failpoint::hit("marked")?;
                }
                // Once the durable marker exists, preserve intent on every
                // publication error: rename may have succeeded before directory
                // sync failed, even when this is the first file.
                publish(index, &pending.files[index])?;
                published_any = true;
                report.changed.push(current_planned.file.path.clone());
                break;
            }

            replans += 1;
            if replans > MAX_REPLANS {
                return Err(give_up_on_conflict(
                    store,
                    vault,
                    guard,
                    &pending,
                    published_any,
                    &current_planned.file.path,
                ));
            }
            // 以新内容为底重算失败（新内容解析不了，或补丁拒绝在它上面改）：还没发布过
            // 任何文件就整体放弃，已发布过就留着 pending 等前滚。
            let replanned = match plan_from(&current_planned.file, *patch, current) {
                Ok(replanned) => replanned,
                Err(err) => {
                    if !published_any {
                        drop_unpublished(store, vault, guard, &pending);
                    }
                    return Err(err.into());
                }
            };
            if replanned.is_noop() {
                // 外部写入的结果恰好就是目标内容。
                discard_staged(&pending.files[index])?;
                pending.files[index] = pending_file(&replanned, None);
                state::set_pending(store, vault, guard.app(), Some(pending.clone()))?;
                break;
            }
            let old = pending.files[index].clone();
            let new_staged = stage(&replanned)?.map(|write| write.tmp_path().to_path_buf());
            pending.files[index] = pending_file(&replanned, new_staged);
            state::set_pending(store, vault, guard.app(), Some(pending.clone()))?;
            discard_staged(&old)?;
            current_planned = replanned;
        }
        failpoint::hit(&format!("published:{index}"))?;
    }

    // Even a target-only change needs durable intent before a callback which may
    // commit more than one existing owner (for example settings plus database).
    if !pending.published {
        pending.published = true;
        state::set_pending(store, vault, guard.app(), Some(pending.clone()))?;
    }
    // LoongPort: exact readback is mandatory before publishing target state.
    // Externally refreshed credentials never become a successful old snapshot.
    if !unverified_files(&pending)?.is_empty() {
        return Err(verification_required());
    }
    // 5. 落定状态，再清掉意图。
    commit_target(&pending.target).map_err(|err| {
        AppError::Message(format!(
            "文件已写入，但状态更新失败，将在下次操作或启动时补完: {err}"
        ))
    })?;
    failpoint::hit("target")?;
    if !unverified_files(&pending)?.is_empty() {
        return Err(verification_required());
    }
    state::set_pending(store, vault, guard.app(), None)?;
    Ok(report)
}

/// 补完或放弃这个应用上一次未完成的操作。调用方持有这个应用的写锁。
pub(crate) fn recover(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    admitted_files: &[LiveFile],
    commit_target: CommitTarget<'_>,
) -> Result<Option<RecoveryOutcome>, AppError> {
    let Some(mut pending) = state::pending(store, vault, guard.app())? else {
        return Ok(None);
    };

    validate_pending(&pending, admitted_files)?;

    enum At {
        Pre,
        Planned,
        Elsewhere,
    }
    let mut positions = Vec::with_capacity(pending.files.len());
    for file in &pending.files {
        let current = digest(read_current(&file.path)?.as_deref());
        positions.push(if current == file.planned {
            At::Planned
        } else if current == file.pre {
            At::Pre
        } else {
            At::Elsewhere
        });
    }
    let elsewhere = || -> Vec<PathBuf> {
        pending
            .files
            .iter()
            .zip(&positions)
            .filter(|(_, at)| matches!(at, At::Elsewhere))
            .map(|(file, _)| file.path.clone())
            .collect()
    };

    let changed_file_published = pending
        .files
        .iter()
        .zip(&positions)
        .any(|(file, at)| file.pre != file.planned && matches!(at, At::Planned));
    if !pending.published && !changed_file_published {
        discard_pending_files(&pending)?;
        state::set_pending(store, vault, guard.app(), None)?;
        let paths = elsewhere();
        if paths.is_empty() {
            log::info!("[{}] 丢弃未开始发布的操作 {}", guard.app(), pending.op);
            return Ok(Some(RecoveryOutcome::Discarded));
        }
        log::warn!(
            "[{}] 上次未完成的操作 {} 还没开始发布，这些文件就被外部修改了，丢弃: {paths:?}",
            guard.app(),
            pending.op
        );
        return Ok(Some(RecoveryOutcome::Abandoned { paths }));
    }

    let mut skipped = elsewhere();
    // A planned hash can come from an external writer before run recorded its
    // publication marker. Recovery must make the forward decision durable before
    // publishing anything or invoking a potentially partial target callback.
    if !pending.published {
        pending.published = true;
        failpoint::hit("mark")?;
        state::set_pending(store, vault, guard.app(), Some(pending.clone()))?;
        failpoint::hit("marked")?;
    }
    for (index, (file, at)) in pending.files.iter().zip(&positions).enumerate() {
        if !matches!(at, At::Pre) {
            continue;
        }
        let staged_ok = match &file.staged {
            Some(staged) => digest(read_current(staged)?.as_deref()) == file.planned,
            None => file.planned.is_none(),
        };
        if !staged_ok {
            skipped.push(file.path.clone());
            continue;
        }
        failpoint::before_publish(index, &file.path);
        let current = read_current(&file.path)?;
        if digest(current.as_deref()) != file.pre {
            skipped.push(file.path.clone());
            continue;
        }
        ensure_first_write_backup(store, vault, &file.path, current.as_deref())?;
        publish(index, file)?;
    }
    // Upstream completes the target despite skipped files. LoongPort must retain
    // the same intent and block dependent writers until every file proves its
    // target. No old token is copied over a newer externally written generation.
    skipped.extend(unverified_files(&pending)?);
    skipped.sort();
    skipped.dedup();
    if !skipped.is_empty() {
        return Ok(Some(RecoveryOutcome::VerificationRequired {
            paths: skipped,
        }));
    }
    failpoint::hit("recover:target")?;
    commit_target(&pending.target)?;
    let paths = unverified_files(&pending)?;
    if !paths.is_empty() {
        return Ok(Some(RecoveryOutcome::VerificationRequired { paths }));
    }
    discard_pending_files(&pending)?;
    state::set_pending(store, vault, guard.app(), None)?;
    Ok(Some(RecoveryOutcome::RolledForward))
}

/// 应用的写入函数拿到写锁后、读任何文件之前调用。
///
/// 调用方在拿锁之前按指针算好了 live 现在归谁、要删哪些独有字段。这时才补完上一次的
/// 操作，指针、模式和文件可能已经变了，照旧写下去会留下上一家的独有字段、把刚补完的
/// 文件当成外部修改。所以补完过就停下，让调用方按新状态重来（入口处先调 [`settle`]
/// 的不会走到这一步）。
pub(crate) fn recover_before_write(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    admitted_files: &[LiveFile],
    commit_target: CommitTarget<'_>,
) -> Result<(), AppError> {
    match recover(store, vault, guard, admitted_files, commit_target)? {
        // 丢弃的操作什么都没改（指针、模式都没动）。
        None | Some(RecoveryOutcome::Discarded | RecoveryOutcome::Abandoned { .. }) => Ok(()),
        Some(RecoveryOutcome::VerificationRequired { .. }) => Err(verification_required()),
        Some(outcome) => {
            log::info!("[{}] 写入前补完了上一次的操作: {outcome:?}", guard.app());
            Err(AppError::localized(
                "live.recovered_before_write",
                "上一次没做完的写入刚刚补完，当前状态已经变了。这次什么都没改，请重新操作一次",
                "An unfinished write from last time was just completed, so the current state has changed. Nothing was changed this time; please try again",
            ))
        }
    }
}

fn pending_file(planned: &Planned, staged: Option<PathBuf>) -> PendingFile {
    PendingFile {
        private: Some(planned.file.private),
        extra: Default::default(),
        path: planned.file.path.clone(),
        pre: planned.pre.clone(),
        planned: planned.planned.clone(),
        staged,
    }
}

/// 发布一个文件：用临时文件替换目标，或者删掉它。
fn publish(index: usize, file: &PendingFile) -> Result<(), AppError> {
    validate_file_path(&file.path)?;
    let private = file.private.ok_or_else(invalid_pending)?;
    let result = match &file.staged {
        Some(staged) => {
            validate_file_path(staged)?;
            if digest(read_current(staged)?.as_deref()) != file.planned {
                return Err(verification_required());
            }
            if private {
                crate::config_file_io::ensure_private_file(staged)?;
            }
            verify_preimage(index, file)?;
            commit_staged(staged, &file.path, private)
        }
        None if file.planned.is_none() => {
            verify_preimage(index, file)?;
            match fs::remove_file(&file.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(AppError::io(&file.path, error)),
            }
        }
        None => Err(invalid_pending()),
    };
    result?;
    failpoint::hit("publish:durability")?;
    crate::live::engine::sync_parent(&file.path)
}

/// Backup and journal fsync can take time. Check again after that work, just
/// before replacing the native file, rather than publishing a stale credential.
fn verify_preimage(index: usize, file: &PendingFile) -> Result<(), AppError> {
    failpoint::before_publish(index, &file.path);
    if digest(read_current(&file.path)?.as_deref()) != file.pre {
        return Err(LiveWriteError::Conflict {
            path: file.path.clone(),
        }
        .into());
    }
    Ok(())
}

fn discard_all(staged: &[Option<PathBuf>]) {
    for path in staged.iter().flatten() {
        let _ = fs::remove_file(path);
    }
}

fn discard_staged(file: &PendingFile) -> Result<(), AppError> {
    if let Some(staged) = &file.staged {
        validate_file_path(staged)?;
        let Some(bytes) = read_current(staged)? else {
            return Ok(());
        };
        if file.private.is_none() || digest(Some(&bytes)) != file.planned {
            return Err(invalid_pending());
        }
        failpoint::hit("discard")?;
        fs::remove_file(staged).map_err(|error| AppError::io(staged, error))?;
        crate::live::engine::sync_parent(staged)?;
    }
    Ok(())
}

fn discard_pending_files(pending: &Pending) -> Result<(), AppError> {
    for file in &pending.files {
        discard_staged(file)?;
    }
    Ok(())
}

/// 还没发布过任何文件时放弃：删掉临时文件和 pending，什么都没改。
fn drop_unpublished(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    pending: &Pending,
) {
    if let Err(error) = discard_pending_files(pending) {
        log::warn!("live staging cleanup requires verification: {error}");
        return;
    }
    if let Err(err) = state::set_pending(store, vault, guard.app(), None) {
        log::warn!("清除写前意图失败: {err}");
    }
}

/// 一直冲突：还没发布过任何文件就整体放弃（什么都没改）；已经发布过就留着 pending
/// 等前滚。
fn give_up_on_conflict(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    pending: &Pending,
    published_any: bool,
    path: &std::path::Path,
) -> AppError {
    if !published_any {
        drop_unpublished(store, vault, guard, pending);
    }
    LiveWriteError::Conflict {
        path: path.to_path_buf(),
    }
    .into()
}

/// 测试用的故障注入点：模拟进程在某一步崩溃（直接返回错误，不做任何清理）。
pub(crate) mod failpoint {
    #[cfg(any(test, feature = "test-hooks"))]
    use std::cell::RefCell;
    use std::path::Path;

    use crate::error::AppError;

    #[cfg(any(test, feature = "test-hooks"))]
    type PublishHook = Box<dyn FnMut(usize, &Path)>;

    #[cfg(any(test, feature = "test-hooks"))]
    thread_local! {
        static CRASH_AT: RefCell<Option<String>> = const { RefCell::new(None) };
        static BEFORE_PUBLISH: RefCell<Option<PublishHook>> =
            const { RefCell::new(None) };
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn crash_at(point: Option<&str>) {
        CRASH_AT.with(|slot| *slot.borrow_mut() = point.map(str::to_string));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn on_before_publish(hook: Option<PublishHook>) {
        BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = hook);
    }

    pub(crate) fn hit(point: &str) -> Result<(), AppError> {
        #[cfg(any(test, feature = "test-hooks"))]
        if CRASH_AT.with(|slot| slot.borrow().as_deref() == Some(point)) {
            return Err(AppError::Message(format!("injected crash at {point}")));
        }
        let _ = point;
        Ok(())
    }

    pub(crate) fn before_publish(index: usize, path: &Path) {
        #[cfg(any(test, feature = "test-hooks"))]
        BEFORE_PUBLISH.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(index, path);
            }
        });
        let _ = (index, path);
    }
}

fn invalid_pending() -> AppError {
    AppError::Config("live.invalid_pending".into())
}
fn verification_required() -> AppError {
    AppError::Config("live.verification_required".into())
}

fn valid_digest(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn validate_operation(op: &str, target: &PendingTarget) -> Result<(), AppError> {
    if !matches!(
        op,
        state::op::SWITCH
            | state::op::APPLY
            | state::op::ENTER
            | state::op::EXIT
            | state::op::DETACH
            | state::op::ATTACH
            | state::op::ROUTE
            | state::op::CATALOG
    ) || !target.extra.is_empty()
        || target.stack.is_some()
        || target
            .written
            .as_ref()
            .is_some_and(|written| !written.extra.is_empty())
    {
        return Err(invalid_pending());
    }
    if let Some(mode) = &target.state {
        mode.validate_for_update().map_err(|_| invalid_pending())?;
    }
    Ok(())
}

fn validate_admitted(files: &[LiveFile]) -> Result<(), AppError> {
    let mut unique = HashSet::new();
    for file in files {
        validate_file_path(&file.path)?;
        if !unique.insert(&file.path) {
            return Err(invalid_pending());
        }
    }
    Ok(())
}

/// Validate the complete journal before reading a target or deleting any staging.
/// A path recovered from disk cannot enlarge the trusted caller's affected files.
fn validate_pending(pending: &Pending, admitted: &[LiveFile]) -> Result<(), AppError> {
    validate_operation(&pending.op, &pending.target)?;
    validate_admitted(admitted)?;
    if !pending.extra.is_empty() {
        return Err(invalid_pending());
    }
    let mut paths = HashSet::new();
    let mut staging = HashSet::new();
    for file in &pending.files {
        validate_file_path(&file.path)?;
        if !file.extra.is_empty()
            || !valid_digest(&file.pre)
            || !valid_digest(&file.planned)
            || !paths.insert(&file.path)
            || !admitted
                .iter()
                .any(|allowed| allowed.path == file.path && Some(allowed.private) == file.private)
        {
            return Err(invalid_pending());
        }
        if let Some(staged) = &file.staged {
            validate_file_path(staged)?;
            let prefix = format!(
                "{}.tmp.",
                file.path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(invalid_pending)?
            );
            let suffix = staged
                .file_name()
                .and_then(|v| v.to_str())
                .and_then(|v| v.strip_prefix(&prefix))
                .ok_or_else(invalid_pending)?;
            let parts: Vec<_> = suffix.split('.').collect();
            if let Some(bytes) = read_current(staged)? {
                if digest(Some(&bytes)) != file.planned {
                    return Err(invalid_pending());
                }
            }
            if staged.parent() != file.path.parent()
                || file.planned.is_none()
                || parts.len() != 3
                || parts
                    .iter()
                    .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
                || !staging.insert(staged)
                || admitted.iter().any(|allowed| allowed.path == *staged)
            {
                return Err(invalid_pending());
            }
        } else if file.planned.is_some() && file.pre != file.planned {
            // A published file keeps its original staged name in the journal even
            // after rename. Missing metadata is distinct from an absent stage file.
            return Err(invalid_pending());
        }
    }
    Ok(())
}

fn unverified_files(pending: &Pending) -> Result<Vec<PathBuf>, AppError> {
    pending
        .files
        .iter()
        .filter_map(|file| {
            let result = validate_file_path(&file.path)
                .and_then(|()| Ok(digest(read_current(&file.path)?.as_deref())));
            match result {
                Ok(digest) if digest == file.planned => None,
                Ok(_) => Some(Ok(file.path.clone())),
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;
