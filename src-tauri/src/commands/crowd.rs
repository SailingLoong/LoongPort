//! 站点实测共建的前端命令。

use crate::crowd::snapshot::{self, Snapshot};
use crate::error::AppError;

/// 读公共快照（对等门禁在这一层）。
///
/// 共建关闭 → `None`：前端拿不到数据、渲染锁定态，**拉取也不会发生**。
/// 有缓存且不陈旧 → 直接回；陈旧 → 回旧值同时后台刷新（先出画面，再追新）；
/// 完全没有缓存 → 现拉（首次打开最多等一个超时）。
#[tauri::command]
pub async fn crowd_get_snapshot() -> Result<Option<Snapshot>, AppError> {
    if !crate::settings::get_settings().crowd_metrics_enabled {
        return Ok(None);
    }

    match snapshot::read_cached() {
        Some(cached) => {
            if snapshot::is_stale(&cached) {
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = snapshot::refresh_and_cache().await {
                        log::debug!("crowd 快照后台刷新失败（用旧值）: {e}");
                    }
                });
            }
            Ok(Some(snapshot::with_bin_edges(cached)))
        }
        None => snapshot::refresh_and_cache()
            .await
            .map(|s| Some(snapshot::with_bin_edges(s))),
    }
}
