//! 异步执行桥：GUI 模式转发 tauri 全局运行时，无 GUI 模式自建 tokio。
//!
//! `services::provider` 等桌面/CLI 共享模块在同步上下文里等待异步结果时用它。
//! GUI 模式下是 [`tauri::async_runtime::block_on`] 的纯 re-export，行为与
//! 引入本模块之前完全一致；只有无 GUI 构建（`loongport-cli` 静态包）才走
//! 自建 runtime 的兜底。

#[cfg(feature = "gui")]
pub(crate) use tauri::async_runtime::{block_on, spawn};

#[cfg(not(feature = "gui"))]
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    // 每次调用建短命 runtime：CLI 是一次性进程，这些调用点都是低频操作
    // （读写配置、备份），不值得为它持有一个全局 runtime。调用方都在同步
    // 上下文里（CLI 的异步阶段在外层 block_on 之外先完成）。
    tokio::runtime::Runtime::new()
        .expect("创建 tokio 运行时失败")
        .block_on(future)
}

/// 后台放一把火就走的任务。GUI 模式用 tauri 全局运行时；无 GUI 构建
/// 兜底开线程跑——CLI 路径实际不会触发这些调用（不构造 AppState），
/// 这里只为让共享代码（model_verification 协调器等）在两种构建下都编译。
#[cfg(not(feature = "gui"))]
pub(crate) fn spawn<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    std::thread::spawn(|| {
        if let Ok(runtime) = tokio::runtime::Runtime::new() {
            runtime.block_on(future);
        }
    });
}
