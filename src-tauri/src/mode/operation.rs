//! Multi-file intent, publication and restart recovery from cc-switch v4.0.2.
//! LoongPort adds encrypted journal I/O, trusted path/privacy admission and strict
//! file readback. External changes or missing staging retain the original pending
//! operation as verification-required; target state is not silently committed.
//! The caller holds the app lock and existing SecretSession read guard throughout.
//! Target callbacks are idempotent and must reuse that guard (no nested session read).

use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::secrets::session::SecretSession;
use crate::store::AppState;
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

/// Existing credential owner may admit one exact current digest before replay.
pub(crate) type VerifyReplay<'a> =
    &'a dyn Fn(&Pending) -> Result<Option<(usize, String)>, AppError>;

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
    validate_operation(guard.app(), op, &target)?;
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
    recover_checked(store, vault, guard, admitted_files, commit_target, &|_| {
        Ok(None)
    })
}

pub(crate) fn recover_checked(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    guard: &AppWriteGuard,
    admitted_files: &[LiveFile],
    commit_target: CommitTarget<'_>,
    verify_replay: VerifyReplay<'_>,
) -> Result<Option<RecoveryOutcome>, AppError> {
    let Some(mut pending) = state::pending(store, vault, guard.app())? else {
        return Ok(None);
    };

    validate_pending(guard.app(), &pending, admitted_files)?;

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
    let elsewhere = |pending: &Pending, positions: &[At]| -> Vec<PathBuf> {
        pending
            .files
            .iter()
            .zip(positions)
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
        let paths = elsewhere(&pending, &positions);
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

    // A client-specific owner may prove a newer credential generation while
    // retaining the exact original target. Accept only that observed digest as
    // a no-op, and only when every other original file is already planned.
    if let Some((index, observed)) = verify_replay(&pending)? {
        if !valid_digest(&Some(observed.clone()))
            || index >= pending.files.len()
            || pending.files[index].private != Some(true)
        {
            return Err(invalid_pending());
        }
        for (i, file) in pending.files.iter().enumerate() {
            let actual = digest(read_current(&file.path)?.as_deref());
            let expected = if i == index {
                Some(observed.clone())
            } else {
                file.planned.clone()
            };
            if actual != expected {
                return Err(verification_required());
            }
        }
        // Delete only validated old staging. If persistence fails, the old
        // journal stays blocked and the same current-generation proof can retry.
        discard_staged(&pending.files[index])?;
        pending.files[index].pre = Some(observed.clone());
        pending.files[index].planned = Some(observed);
        pending.files[index].staged = None;
        state::set_pending(store, vault, guard.app(), Some(pending.clone()))?;
        failpoint::hit("recover:adopted")?;
        positions[index] = At::Planned;
    }
    let mut skipped = elsewhere(&pending, &positions);
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

    pub(crate) fn current_crash() -> Option<String> {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            CRASH_AT.with(|slot| slot.borrow().clone())
        }
        #[cfg(not(any(test, feature = "test-hooks")))]
        {
            None
        }
    }

    /// Keep thread-local test injection scoped when the real controller uses a
    /// blocking worker. Ordinary builds never expose a configurable failpoint.
    pub(crate) fn in_worker<R>(point: Option<String>, work: impl FnOnce() -> R) -> R {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            struct Reset(Option<String>);
            impl Drop for Reset {
                fn drop(&mut self) {
                    CRASH_AT.with(|slot| *slot.borrow_mut() = self.0.take());
                }
            }
            let previous = CRASH_AT.with(|slot| slot.replace(point));
            let _reset = Reset(previous);
            work()
        }
        #[cfg(not(any(test, feature = "test-hooks")))]
        {
            let _ = point;
            work()
        }
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

fn validate_operation(app: &str, op: &str, target: &PendingTarget) -> Result<(), AppError> {
    if target.model_preference.is_some() || target.routing_order.is_some() {
        if op != state::op::APPLY || !matches!(app, "claude" | "codex" | "gemini" | "grokbuild") {
            return Err(invalid_pending());
        }
        if let Some(action) = &target.model_preference {
            if target.pointer.is_none()
                && target
                    .state
                    .as_ref()
                    .and_then(|mode| mode.proxy_route.as_ref())
                    .is_none()
            {
                return Err(invalid_pending());
            }
            if matches!(action, state::ModelPreferenceAction::Set { model } if model.trim().is_empty())
            {
                return Err(invalid_pending());
            }
        }
        if let Some(order) = &target.routing_order {
            if order.profile_name.trim().is_empty()
                || order.planned.profiles.is_none()
                || order.planned.current.is_none()
                || order.planned.priority.is_none()
            {
                return Err(invalid_pending());
            }
            if let Some(id) = target.pointer.as_ref().or_else(|| {
                target
                    .state
                    .as_ref()
                    .and_then(|mode| mode.proxy_route.as_ref())
            }) {
                if !order.provider_ids.contains(id) {
                    return Err(invalid_pending());
                }
            }
        }
        if let Some(row) = &target.saved_row {
            if let Some(id) = target.pointer.as_ref().or_else(|| {
                target
                    .state
                    .as_ref()
                    .and_then(|mode| mode.proxy_route.as_ref())
            }) {
                if saved_provider(row)?.id != *id {
                    return Err(invalid_pending());
                }
            }
        }
    }
    if let Some(row) = &target.saved_row {
        if op != state::op::APPLY {
            return Err(invalid_pending());
        }
        saved_provider(row)?;
    }
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
        || target.written.as_ref().is_some_and(|written| {
            written.validate().is_err() || (written.codex.is_some() && app != "codex")
        })
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
fn validate_pending(app: &str, pending: &Pending, admitted: &[LiveFile]) -> Result<(), AppError> {
    validate_operation(app, &pending.op, &pending.target)?;
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
            let staged_target = staged
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::config_file_io::staging_target_name);
            if staged.parent() != file.path.parent()
                || file.planned.is_none()
                || staged_target.is_none()
                || staged_target != file.path.file_name().and_then(|name| name.to_str())
                || !staging.insert(staged)
                || admitted.iter().any(|allowed| allowed.path == *staged)
            {
                return Err(invalid_pending());
            }
            if let Some(bytes) = read_current(staged)? {
                if digest(Some(&bytes)) != file.planned {
                    return Err(invalid_pending());
                }
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

// Application bindings of upstream AppWrite/commit_target. These remain in the
// original operation owner, sharing the generic transaction above.

/// The persisted schema is the existing migration fact, not a second feature flag.
pub(crate) fn uses_upstream4_schema(db: &Database) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);
    let version = Database::get_user_version(&conn)?;
    let modern = uses_upstream4_version(version, crate::database::SCHEMA_VERSION)?;
    if modern
        && crate::database::loongport_schema::read_stored_version(&conn)?
            != crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
    {
        return Err(AppError::Config("upgrade.future_version".into()));
    }
    Ok(modern)
}

// Pure comparison keeps the active startup ceiling distinct from the target
// migration version so the transition can be tested before ordinary activation.
pub(crate) fn uses_upstream4_version(version: i32, supported: i32) -> Result<bool, AppError> {
    if version == crate::database::UPSTREAM4_SCHEMA_VERSION {
        return Ok(true);
    }
    if version <= supported && version < crate::database::UPSTREAM4_SCHEMA_VERSION {
        return Ok(false);
    }
    Err(AppError::Config("upgrade.future_version".into()))
}

pub(crate) fn saved_provider(row: &state::SavedRow) -> Result<crate::provider::Provider, AppError> {
    if !valid_digest(&Some(row.before.clone())) {
        return Err(invalid_pending());
    }
    let provider: crate::provider::Provider =
        serde_json::from_value(row.provider.clone()).map_err(|_| invalid_pending())?;
    if provider.id.is_empty() || Database::provider_update_value(&provider)? != row.provider {
        return Err(invalid_pending());
    }
    Ok(provider)
}

pub(crate) fn verify_saved_row(
    db: &Database,
    session: &SecretSession,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &AppType,
    target: &PendingTarget,
) -> Result<(), AppError> {
    if let Some(order) = &target.routing_order {
        crate::database::order_profiles::verify_prepared(
            db,
            app.as_str(),
            &order.profile_name,
            &order.provider_ids,
            &order.before,
            &order.planned,
        )?;
    }
    if let Some(row) = &target.saved_row {
        let planned = saved_provider(row)?;
        let current = db
            .get_provider_by_id_with_vault(&planned.id, app.as_str(), session, vault)?
            .ok_or_else(|| AppError::Config("mode.provider_changed".into()))?;
        let digest = Database::provider_update_digest(&current)?;
        if digest != row.before && digest != Database::provider_update_digest(&planned)? {
            return Err(AppError::Config("mode.provider_changed".into()));
        }
    }
    Ok(())
}

/// Idempotent upstream target owner, retaining LoongPort's existing settings/DB
/// pointer and model preference owners. Mode transitions/Stack are not enabled.
pub(crate) fn commit_target(
    db: &Database,
    session: &SecretSession,
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &AppType,
    target: &PendingTarget,
) -> Result<(), AppError> {
    if target.stack.is_some()
        || !target.extra.is_empty()
        || (target.written.is_some() && !matches!(app, AppType::GrokBuild | AppType::Codex))
        || target.written.as_ref().is_some_and(|w| {
            w.validate().is_err()
                || (w.codex.is_some() && *app != AppType::Codex)
                || (*app == AppType::Codex && (w.codex.is_none() || !w.tables.is_empty()))
        })
    {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    super::current::validate_known_mode(store, vault, app)?;
    verify_saved_row(db, session, vault, app, target)?;
    if let Some(row) = &target.saved_row {
        let provider = saved_provider(row)?;
        db.save_provider_with_vault(app.as_str(), &provider, Some(&row.before), session, vault)?;
        let current = db
            .get_provider_by_id_with_vault(&provider.id, app.as_str(), session, vault)?
            .ok_or_else(invalid_pending)?;
        if Database::provider_update_digest(&current)?
            != Database::provider_update_digest(&provider)?
        {
            return Err(verification_required());
        }
    }
    if let Some(id) = target.pointer.as_deref() {
        if !super::current::provider_exists(db, app, id)? {
            return Err(AppError::Config("mode.verification_required".into()));
        }
        crate::settings::set_current_provider_with_vault(app, Some(id), session, vault)?;
        db.set_current_provider(app.as_str(), id)?;
        super::current::verify_direct_pointer(db, app, id)?;
    }
    let fallback_clear = target.pointer.is_some()
        || target
            .saved_row
            .as_ref()
            .is_some_and(|row| row.clear_model_preference);
    let action = target
        .model_preference
        .as_ref()
        .cloned()
        .or_else(|| fallback_clear.then_some(state::ModelPreferenceAction::Clear {}));
    if let Some(action) = action {
        let model = match &action {
            state::ModelPreferenceAction::Clear {} => None,
            state::ModelPreferenceAction::Set { model } => Some(model.as_str()),
        };
        crate::proxy::auto_strategy::set_model_pref(db, app.as_str(), model)?;
        if crate::proxy::auto_strategy::get_model_pref_checked(db, app.as_str())?.as_deref()
            != Some(model.unwrap_or(""))
        {
            return Err(verification_required());
        }
    }
    if let Some(order) = &target.routing_order {
        crate::database::order_profiles::apply_prepared(
            db,
            app.as_str(),
            &order.profile_name,
            &order.provider_ids,
            &order.before,
            &order.planned,
        )?;
    }
    if let Some(written) = &target.written {
        state::update_app(store, vault, app.as_str(), |entry| {
            entry.written = Some(written.clone());
            Ok(())
        })?;
        if state::written(store, vault, app.as_str())?.as_ref() != Some(written) {
            return Err(AppError::Config("mode.verification_required".into()));
        }
    }
    if let Some(mode) = &target.state {
        mode.validate_for_update().map_err(|_| invalid_pending())?;
        if mode.mode.is_none() {
            return Err(invalid_pending());
        }
        if let Some(id) = &mode.proxy_route {
            if !super::current::provider_exists(db, app, id)? {
                return Err(verification_required());
            }
        }
        state::update_app(store, vault, app.as_str(), |entry| {
            entry
                .set_mode_state(mode.clone())
                .map_err(|_| invalid_pending())
        })?;
        let (_, failover) = db.get_proxy_flags_checked(app.as_str())?;
        db.set_proxy_flags_sync(app.as_str(), mode.is_proxy(), failover)?;
        if state::mode_state(store, vault, app.as_str())? != *mode
            || db.get_proxy_flags_checked(app.as_str())? != (mode.is_proxy(), failover)
        {
            return Err(verification_required());
        }
    }
    super::current::validate_known_mode(store, vault, app).map(|_| ())
}

/// Upstream per-app transaction context, borrowing the existing session guard.
/// Caller owns the existing application switch lock. Recovery is explicit, so
/// begin refuses any old intent instead of silently finishing a different action.
pub(crate) struct AppWrite<'a> {
    db: &'a Database,
    session: &'a SecretSession,
    app: AppType,
    pub(crate) store: DeviceStore,
    pub(crate) guard: AppWriteGuard,
    pub(crate) vault: RwLockReadGuard<'a, VaultContext>,
}
impl<'a> AppWrite<'a> {
    pub(crate) fn open(state: &'a AppState, app: &AppType) -> Result<Self, AppError> {
        let write = Self::open_mode(&state.proxy_service, app)?;
        super::current::validate_direct_mode(&write.store, &write.vault, app)?;
        Ok(write)
    }
    pub(crate) fn open_mode(
        service: &'a crate::services::ProxyService,
        app: &AppType,
    ) -> Result<Self, AppError> {
        let db = service.database();
        if !uses_upstream4_schema(db)? {
            return Err(AppError::Config("upgrade.migration_required".into()));
        }
        crate::settings::get_current_provider_ready(app)?;
        if futures::executor::block_on(db.get_live_backup(app.as_str()))?.is_some() {
            return Err(verification_required());
        }
        let placeholder = service.detect_takeover_in_live_config_for_app(app);
        let store = DeviceStore::for_device();
        let guard = crate::live::engine::lock_app(app.as_str());
        let session = db.secret_session();
        let vault = session.read()?;
        crate::secrets::upgrade::checkpoint::ensure_sync_admitted(&store)?;
        let mode = super::current::validate_known_mode(&store, &vault, app)?;
        if placeholder
            && !(mode.is_proxy() && mode.attached)
            && state::pending(&store, &vault, app.as_str())?.is_none()
        {
            return Err(verification_required());
        }
        Ok(Self {
            db,
            session,
            app: app.clone(),
            store,
            guard,
            vault,
        })
    }
    pub(crate) fn begin_mode(
        service: &'a crate::services::ProxyService,
        app: &AppType,
    ) -> Result<Self, AppError> {
        let write = Self::open_mode(service, app)?;
        if state::pending(&write.store, &write.vault, app.as_str())?.is_some() {
            return Err(verification_required());
        }
        Ok(write)
    }
    pub(crate) fn begin(state: &'a AppState, app: &AppType) -> Result<Self, AppError> {
        let write = Self::open(state, app)?;
        if state::pending(&write.store, &write.vault, app.as_str())?.is_some() {
            return Err(AppError::Config("mode.verification_required".into()));
        }
        Ok(write)
    }
    pub(crate) fn commit(&self, target: &PendingTarget) -> Result<(), AppError> {
        commit_target(
            self.db,
            self.session,
            &self.store,
            &self.vault,
            &self.app,
            target,
        )
    }
    pub(crate) fn run(
        &self,
        op: &str,
        changes: &[FileChange<'_>],
        target: PendingTarget,
    ) -> Result<OperationReport, AppError> {
        verify_saved_row(self.db, self.session, &self.vault, &self.app, &target)?;
        run(
            &self.store,
            &self.vault,
            &self.guard,
            op,
            changes,
            target,
            &|target| self.commit(target),
        )
    }
}

/// Explicit recovery only. No read/startup/GUI caller is registered here.
pub(crate) fn recover_pending(
    state: &AppState,
    app: &AppType,
    files: &[LiveFile],
) -> Result<Option<RecoveryOutcome>, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let write = AppWrite::open(state, app)?;
    recover(&write.store, &write.vault, &write.guard, files, &|target| {
        write.commit(target)
    })
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;

/// Metadata writers share the app's unresolved-operation barrier. The caller
/// owns the original service app lock; release the vault guard before DAO calls
/// that pin their own credential generation. Legacy/independent apps are unchanged.
pub(crate) fn admit_metadata_write(
    service: &crate::services::ProxyService,
    app: &AppType,
) -> Result<(), AppError> {
    if app.supports_local_proxy() && uses_upstream4_schema(service.database())? {
        AppWrite::begin_mode(service, app)?;
        if !super::current::read_view(service, app).is_some_and(|view| view.can_write) {
            return Err(AppError::Config("mode.verification_required".into()));
        }
    }
    Ok(())
}
