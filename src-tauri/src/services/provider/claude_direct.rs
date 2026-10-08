//! 写 Claude Code 的 `settings.json`：只替换关键字段和独有字段，其余字节不碰。
//!
//! 写 Claude live 的入口（切换、新增第一个供应商、编辑当前供应商、同步、统一供应商、
//! 进入 / 退出代理）都走这里：先拿应用写锁，再经 `mode::operation` 记下 pending、发布。
//! 不回填、不合并通用配置片段、不注入上下文默认值：用户的设置本来就留在 live 里。
//! LoongPort currently dispatches only schema20 ProviderService switches here;
//! other write entrypoints remain on their existing paths until integrated.

use crate::app_config::AppType;
use crate::config::get_claude_settings_path;
use crate::error::AppError;
use crate::live::engine::LiveFile;
use crate::live::patch::json::JsonPatch;
use crate::live::project::claude::{direct_patch, ClaudeProjection};
use crate::mode::operation::{AppWrite, FileChange, OperationReport};
use crate::mode::state::{op, PendingTarget};
use crate::provider::Provider;
use crate::store::AppState;

/// `settings.json`（旧安装可能是 `claude.json`）。里面有 Key，按 0600 写。
pub(crate) fn settings_file() -> LiveFile {
    LiveFile::private(get_claude_settings_path())
}

/// 从 `prev` 切到 `target`：同一个操作里写 live、再把当前供应商改成 `target`。
///
/// `prev` 是 live 现在对应的供应商（直连指针指向的那家），用来删它带进来的独有字段。
pub(crate) fn switch_to(
    state: &AppState,
    prev: Option<&Provider>,
    target: &Provider,
) -> Result<OperationReport, AppError> {
    write(state, prev, target, Some(&target.id))
}

/// 把当前供应商 `target` 重新投影到 live，不改指针。`prev` 是 live 现在对应的那一版
/// 行（编辑前的行；没改过就是它自己）。
#[allow(dead_code)] // Retained upstream API; current-editor integration follows.
pub(crate) fn reapply(
    state: &AppState,
    prev: Option<&Provider>,
    target: &Provider,
) -> Result<OperationReport, AppError> {
    write(state, prev, target, None)
}

fn write(
    state: &AppState,
    prev: Option<&Provider>,
    target: &Provider,
    pointer: Option<&str>,
) -> Result<OperationReport, AppError> {
    let prev = prev.map(|provider| ClaudeProjection::of(&provider.settings_config));
    let patch = direct_patch(
        prev.as_ref(),
        &ClaudeProjection::of(&target.settings_config),
    );
    run(
        state,
        if pointer.is_some() {
            op::SWITCH
        } else {
            op::APPLY
        },
        Some(&patch),
        PendingTarget::pointer(pointer.map(str::to_string)),
    )
}

/// 用 `patch` 改写 `settings.json`，和 `target` 在同一个操作里提交；`patch` 为空时只
/// 落定状态、不读也不写文件。
pub(crate) fn run(
    state: &AppState,
    op: &str,
    patch: Option<&JsonPatch>,
    target: PendingTarget,
) -> Result<OperationReport, AppError> {
    // 补丁是按调用方读到的指针算的（要删上一家的独有字段）：未完成操作须先走显式恢复。
    let write = AppWrite::begin(state, &AppType::Claude)?;
    let changes: Vec<FileChange<'_>> = patch
        .into_iter()
        .map(|patch| FileChange {
            file: settings_file(),
            patch,
        })
        .collect();
    write.run(op, &changes, target)
}
