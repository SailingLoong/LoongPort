use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::oneshot;

use crate::error::AppError;

const APP_UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

fn app_update_check_timeout() -> Duration {
    APP_UPDATE_CHECK_TIMEOUT
}

/// 应用更新下载进度（通过 `update-download-progress` 事件发给前端）。
/// 前端只在安装弹窗打开时监听；后台预下载发出的同名事件无人监听，无副作用。
#[derive(Clone, serde::Serialize)]
struct UpdateDownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// 预下载状态机的纯决策：给定当前暂存版本与最新检查结果，该做什么。
/// 与 I/O 分离以便单测（`Update` 无法在测试中构造）。
#[derive(Debug, PartialEq, Eq)]
enum PrestageAction {
    /// 无可用更新：丢弃暂存（含其文件）
    DropStaged,
    /// 同版本已在下载/已就绪：保留
    Keep,
    /// 需要开始（或换版本重下）：`Start`
    Start,
    /// 本来就没有暂存也没有更新
    Nothing,
}

fn prestage_action(staged: Option<&str>, available: Option<&str>) -> PrestageAction {
    match (staged, available) {
        (None, None) => PrestageAction::Nothing,
        (Some(_), None) => PrestageAction::DropStaged,
        (None, Some(_)) => PrestageAction::Start,
        (Some(staged), Some(available)) => {
            if staged == available {
                PrestageAction::Keep
            } else {
                PrestageAction::Start
            }
        }
    }
}

/// `set_ready` 的发布守卫：只有当前 Downloading 槽位仍属于同版本时才允许发布。
/// 后到的旧版本任务完成（已被更新版本取代）不得覆盖新状态。
fn should_publish_ready(current: &StageInner, version: &str) -> bool {
    matches!(current, StageInner::Downloading { version: v, .. } if v == version)
}

/// 已就绪的预下载产物：`Update` 对象 + 安装包落盘路径。
struct ReadyUpdate {
    update: Update,
    installer: std::path::PathBuf,
}

enum StageInner {
    Idle,
    Downloading {
        version: String,
        /// 预下载任务完成信号的接收端；安装命令取走以续接在途下载
        /// （oneshot 完成态被锁存，先创建后等待不存在注册竞态）。
        done: Option<oneshot::Receiver<()>>,
    },
    Ready {
        version: String,
        ready: Box<ReadyUpdate>,
    },
}

impl StageInner {
    fn staged_version(&self) -> Option<&str> {
        match self {
            StageInner::Idle => None,
            StageInner::Downloading { version, .. } | StageInner::Ready { version, .. } => {
                Some(version)
            }
        }
    }
}

/// 跨命令持有的应用更新预下载状态。
/// `check` 发现新版本时后台预下载到磁盘；`install` 优先消费已就绪的产物，
/// 让「点升级」无需现场下载（离线也能装）。
pub struct AppUpdateStage {
    inner: std::sync::Mutex<StageInner>,
}

impl AppUpdateStage {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(StageInner::Idle),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StageInner> {
        // 状态机临界区无 panic 路径；中毒也按原值继续，避免预下载状态卡死安装。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 决定是否需要（重新）预下载，并在需要时同步占用 Downloading 槽位，
    /// 把完成信号交给调用方随预下载任务持有。决策与占位在同一临界区内，
    /// 并发检查不会双起任务。
    fn begin_if_needed(
        &self,
        available: Option<&str>,
    ) -> (PrestageAction, Option<oneshot::Sender<()>>) {
        let mut inner = self.lock();
        let action = prestage_action(inner.staged_version(), available);
        match action {
            PrestageAction::DropStaged => {
                if let StageInner::Ready { ready, .. } = &*inner {
                    let _ = std::fs::remove_file(&ready.installer);
                }
                *inner = StageInner::Idle;
                (action, None)
            }
            PrestageAction::Start => {
                if let StageInner::Ready { ready, .. } = &*inner {
                    let _ = std::fs::remove_file(&ready.installer);
                }
                let (done_tx, done_rx) = oneshot::channel();
                *inner = StageInner::Downloading {
                    version: available
                        .expect("Start implies an available version")
                        .to_string(),
                    done: Some(done_rx),
                };
                (action, Some(done_tx))
            }
            PrestageAction::Keep | PrestageAction::Nothing => (action, None),
        }
    }

    fn set_ready(&self, version: String, ready: Box<ReadyUpdate>) {
        let mut inner = self.lock();
        if should_publish_ready(&inner, &version) {
            *inner = StageInner::Ready { version, ready };
        } else {
            // 一个已被更新版本取代的预下载任务后到完成：不发布（否则会把
            // 更新的 Downloading/Ready 顶掉，点升级时装到被取代的旧版），
            // 并清掉刚写盘的过期产物。
            let _ = std::fs::remove_file(&ready.installer);
        }
    }

    /// 预下载失败：只在仍处于同版本 Downloading 时退回 Idle（不覆盖新版本就绪态）。
    fn set_failed(&self, version: &str) {
        let mut inner = self.lock();
        if matches!(&*inner, StageInner::Downloading { version: v, .. } if v == version) {
            *inner = StageInner::Idle;
        }
    }

    fn take_ready(&self) -> Option<Box<ReadyUpdate>> {
        let mut inner = self.lock();
        match std::mem::replace(&mut *inner, StageInner::Idle) {
            StageInner::Ready { ready, .. } => Some(ready),
            other => {
                *inner = other;
                None
            }
        }
    }

    fn take_downloading_done(&self) -> Option<oneshot::Receiver<()>> {
        let mut inner = self.lock();
        match &mut *inner {
            StageInner::Downloading { done, .. } => done.take(),
            _ => None,
        }
    }
}

/// 预下载安装包落盘目录（应用缓存目录下的自有子目录，启动时整体清扫）。
fn staged_update_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|dir| dir.join("app-updates"))
}

/// 启动时清扫上个会话残留的预下载文件：`Update` 对象不跨进程，
/// 旧文件不可能再被安装，留着只占缓存。
pub fn sweep_stale_staged_updates(app: &AppHandle) {
    if let Some(dir) = staged_update_dir(app) {
        if dir.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                log::warn!("清扫预下载更新目录失败 {}: {e}", dir.display());
            }
        }
    }
}

/// `check` 之后调用：按最新检查结果（重新）安排后台预下载。
/// 便携版不能原地升级，跳过（否则纯浪费后台流量）。
fn schedule_prestage(app: &AppHandle, available: Option<Update>) {
    if crate::commands::portable_mode_enabled() {
        return;
    }
    let stage = app.state::<AppUpdateStage>();
    let (action, done_tx) =
        stage.begin_if_needed(available.as_ref().map(|update| update.version.as_str()));
    if action == PrestageAction::Start {
        let update = available.expect("Start implies an owned Update");
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            prestage_download(app, update, done_tx).await;
        });
    }
}

async fn prestage_download(app: AppHandle, update: Update, done_tx: Option<oneshot::Sender<()>>) {
    let version = update.version.clone();
    let Some(dir) = staged_update_dir(&app) else {
        app.state::<AppUpdateStage>().set_failed(&version);
        return;
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log::warn!("创建预下载目录失败 {}: {e}", dir.display());
        app.state::<AppUpdateStage>().set_failed(&version);
        return;
    }
    let installer = dir.join(format!("{version}.installer"));
    log::info!("后台预下载应用更新: {version}");
    match download_update_bytes(&app, &update).await {
        Ok(bytes) => match tokio::fs::write(&installer, &bytes).await {
            Ok(()) => {
                log::info!("应用更新预下载完成: {version}（{} 字节）", bytes.len());
                app.state::<AppUpdateStage>()
                    .set_ready(version, Box::new(ReadyUpdate { update, installer }));
            }
            Err(e) => {
                log::warn!("预下载安装包写盘失败 {}: {e}", installer.display());
                app.state::<AppUpdateStage>().set_failed(&version);
            }
        },
        Err(e) => {
            log::warn!("后台预下载失败（点击升级时将现场重下）: {e}");
            app.state::<AppUpdateStage>().set_failed(&version);
        }
    }
    // 无论成败都释放等待中的安装命令（drop 等价于发送）。
    drop(done_tx);
}

/// 下载更新字节并向前端发 `update-download-progress` 事件（弹窗打开时才有监听者）。
async fn download_update_bytes(app: &AppHandle, update: &Update) -> Result<Vec<u8>, String> {
    let progress_handle = app.clone();
    let mut downloaded: u64 = 0;
    update
        .download(
            move |chunk_len, content_len| {
                downloaded = downloaded.saturating_add(chunk_len as u64);
                let _ = progress_handle.emit(
                    "update-download-progress",
                    UpdateDownloadProgress {
                        downloaded,
                        total: content_len,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载更新失败: {e}"))
}

/// 执行安装并按平台收尾。Windows 的安装器会替换并退出当前进程；
/// macOS/Linux 先安装再走统一的重启路径。
async fn install_bytes_and_restart(
    app: &AppHandle,
    update: Update,
    bytes: Vec<u8>,
) -> Result<bool, String> {
    log::info!("开始安装应用更新: {}", update.version);

    #[cfg(target_os = "windows")]
    {
        // Windows updater 会在 install() 内启动安装器并直接退出当前进程
        // （插件内部 std::process::exit(0)，绕过 TrayIcon::drop、不发
        // NIM_DELETE，会残留死图标——与托盘"退出"路径相同的问题）。
        // 因此清理只能放在 install 前执行，且必须显式移除托盘图标。
        crate::save_window_state_before_exit(app);
        crate::cleanup_before_exit(app).await;
        crate::remove_tray_icon_before_exit(app);
        crate::destroy_single_instance_lock(app);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        update.install(bytes).map_err(|e| {
            format!(
                "Windows 更新安装失败: {e}。已执行退出前清理，代理或 Live 接管可能已暂停；请重启应用或重新开启代理后再试。"
            )
        })?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS/Linux install() 会返回；先安装，避免安装失败时误停代理/撤回接管。
        update
            .install(bytes)
            .map_err(|e| format!("安装更新失败: {e}"))?;

        crate::save_window_state_before_exit(app);
        crate::cleanup_before_exit(app).await;

        log::info!("应用更新安装完成，正在重启应用");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        crate::restart_process(app);
    }
}

/// 「点升级」的统一入口：已预下载则瞬装（无需网络），在途则续接进度，
/// 都没有则回退为现场「检查+下载+安装」。
pub async fn install_staged_or_download_and_restart(app: &AppHandle) -> Result<bool, String> {
    let stage = app.state::<AppUpdateStage>();

    if let Some(ready) = stage.take_ready() {
        if let Some(outcome) = install_ready_update(app, ready).await {
            return outcome;
        }
    }

    if let Some(done) = stage.take_downloading_done() {
        // 续接后台下载；预下载任务无论成败都会关闭通道。
        let _ = done.await;
        if let Some(ready) = stage.take_ready() {
            if let Some(outcome) = install_ready_update(app, ready).await {
                return outcome;
            }
        }
        // 预下载失败/产物不可读 → 继续走全量流程。
    }

    let updater = app
        .updater_builder()
        .build()
        .map_err(|e| format!("初始化更新器失败: {e}"))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?
    else {
        return Ok(false);
    };
    log::info!("开始下载应用更新: {}", update.version);
    let bytes = download_update_bytes(app, &update).await?;
    install_bytes_and_restart(app, update, bytes).await
}

/// 消费就绪的预下载产物：先读后删；读失败返回 None 让调用方退回全量流程。
async fn install_ready_update(
    app: &AppHandle,
    ready: Box<ReadyUpdate>,
) -> Option<Result<bool, String>> {
    match tokio::fs::read(&ready.installer).await {
        Ok(bytes) => {
            let _ = std::fs::remove_file(&ready.installer);
            Some(install_bytes_and_restart(app, ready.update, bytes).await)
        }
        Err(e) => {
            log::warn!("预下载安装包读取失败，转为现场下载: {e}");
            None
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub available_version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AppUpdateCheckResult {
    UpToDate,
    Available { info: AppUpdateInfo },
}

impl AppUpdateCheckResult {
    pub fn available(
        current_version: String,
        available_version: String,
        notes: Option<String>,
        pub_date: Option<String>,
    ) -> Self {
        Self::Available {
            info: AppUpdateInfo {
                current_version,
                available_version,
                notes,
                pub_date,
            },
        }
    }
}

pub async fn check(app: &tauri::AppHandle) -> Result<AppUpdateCheckResult, AppError> {
    let updater = app
        .updater_builder()
        .timeout(app_update_check_timeout())
        .build()
        .map_err(|error| AppError::Message(format!("初始化更新器失败: {error}")))?;
    let update = updater
        .check()
        .await
        .map_err(|error| AppError::Message(format!("检查更新失败: {error}")))?;

    let Some(update) = update else {
        // 检查不到更新也走一次 reconcile：应用跨版本运行期间，暂存的旧版
        // 产物应当被丢弃（例如 latest.json 撤回后）。
        schedule_prestage(app, None);
        return Ok(AppUpdateCheckResult::UpToDate);
    };

    // The updater validates `pub_date` as RFC 3339 before constructing `Update`.
    // Preserve that original manifest string instead of using OffsetDateTime's
    // human-readable Display representation, which is not RFC 3339.
    let pub_date = update
        .date
        .and_then(|_| update.raw_json.get("pub_date")?.as_str().map(str::to_owned));

    let result = AppUpdateCheckResult::available(
        update.current_version.clone(),
        update.version.clone(),
        update.body.clone(),
        pub_date,
    );
    // 检查到新版本：克隆信息字段后，把 `Update` 对象交给预下载流水线
    //（点升级时直接消费，无需现场重新检查+下载）。
    schedule_prestage(app, Some(update));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{
        app_update_check_timeout, prestage_action, should_publish_ready, AppUpdateCheckResult,
        PrestageAction, StageInner,
    };
    use std::time::Duration;

    #[test]
    fn prestage_action_matrix() {
        use PrestageAction::*;
        // 无暂存、无更新：什么都不做。
        assert_eq!(prestage_action(None, None), Nothing);
        // 有暂存、无更新（latest.json 撤回/误报）：丢弃暂存。
        assert_eq!(prestage_action(Some("6.8.3"), None), DropStaged);
        // 无暂存、有更新：开始预下载。
        assert_eq!(prestage_action(None, Some("6.8.3")), Start);
        // 同版本在下载/已就绪：保留，不重复下载。
        assert_eq!(prestage_action(Some("6.8.3"), Some("6.8.3")), Keep);
        // 应用跨版本运行，来了更新的版本：换目标重新预下载。
        assert_eq!(prestage_action(Some("6.8.3"), Some("6.8.4")), Start);
    }

    #[test]
    fn stale_prestage_completion_never_supersedes_newer_state() {
        use tokio::sync::oneshot;

        let downloading = |version: &str| StageInner::Downloading {
            version: version.to_string(),
            done: Some(oneshot::channel().1),
        };
        // 只有仍处于同版本 Downloading 时才发布就绪。
        assert!(should_publish_ready(&downloading("6.9.0"), "6.9.0"));
        // 后到的旧版本完成不得覆盖更新版本的下载，也不得在 Idle（更新消失）时发布。
        assert!(!should_publish_ready(&downloading("6.9.1"), "6.9.0"));
        assert!(!should_publish_ready(&StageInner::Idle, "6.9.0"));
    }

    #[test]
    fn update_checks_use_the_backend_request_timeout_policy() {
        assert_eq!(app_update_check_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn available_update_serializes_for_the_frontend() {
        let result = AppUpdateCheckResult::available(
            "3.24.0".into(),
            "3.25.0".into(),
            Some("notes".into()),
            Some("2026-08-14T00:00:00Z".into()),
        );

        let value = serde_json::to_value(result).expect("serialize available update");

        assert_eq!(value["status"], "available");
        assert_eq!(value["info"]["currentVersion"], "3.24.0");
        assert_eq!(value["info"]["availableVersion"], "3.25.0");
        assert_eq!(value["info"]["notes"], "notes");
        assert_eq!(value["info"]["pubDate"], "2026-08-14T00:00:00Z");
    }

    #[test]
    fn absent_update_metadata_serializes_as_null() {
        let result = AppUpdateCheckResult::available("3.24.0".into(), "3.25.0".into(), None, None);

        let value = serde_json::to_value(result).expect("serialize absent update metadata");

        assert_eq!(value["info"]["notes"], serde_json::Value::Null);
        assert_eq!(value["info"]["pubDate"], serde_json::Value::Null);
    }

    #[test]
    fn up_to_date_serializes_as_a_tagged_result() {
        let value = serde_json::to_value(AppUpdateCheckResult::UpToDate)
            .expect("serialize up-to-date result");

        assert_eq!(value, serde_json::json!({ "status": "upToDate" }));
    }
}
