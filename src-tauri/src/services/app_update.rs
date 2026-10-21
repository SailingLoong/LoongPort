use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::watch;

use crate::diagnostics::DiagnosticEvent;
use crate::error::AppError;

use super::app_update_policy::{self as policy, CheckFailure, DownloadFailure};
use super::app_update_stage::{DownloadTicket, PrestageAction, ReadyUpdate};

pub type AppUpdateStage = super::app_update_stage::UpdateStage<Update>;

// Diagnostic labels are static, and counts come from the existing owner. Never
// attach an Update, error, version, URL, installer path or frontend request ID.
fn update_event(stage: &'static str, outcome: &'static str) -> DiagnosticEvent {
    DiagnosticEvent::new("app.update", outcome).field("stage", stage)
}

// Instance-local logging only: dropping a future must close its started event,
// without cancelling work, changing the stage owner or inventing a failure.
struct UpdateDiagnostic<F: Fn(&'static str) -> DiagnosticEvent> {
    event: F,
    terminal: bool,
}

impl<F: Fn(&'static str) -> DiagnosticEvent> UpdateDiagnostic<F> {
    fn new(event: F) -> Self {
        log::debug!("{}", event("started"));
        Self {
            event,
            terminal: false,
        }
    }

    fn finish(
        &mut self,
        outcome: &'static str,
        error_category: Option<&'static str>,
        byte_count: Option<u64>,
    ) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        let mut event = (self.event)(outcome);
        if let Some(category) = error_category {
            event = event.field("error_category", category);
        }
        if let Some(count) = byte_count {
            event = event.field("byte_count", count);
        }
        if outcome == "failed" {
            log::warn!("{event}");
        } else {
            log::debug!("{event}");
        }
    }
}

impl<F: Fn(&'static str) -> DiagnosticEvent> Drop for UpdateDiagnostic<F> {
    fn drop(&mut self) {
        self.finish("dropped", None, None);
    }
}

// Keep typed transport facts until after the race. Raw plugin messages can
// contain URLs or proxy credentials and must not become UI text or log fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateCheckFailure {
    Timeout,
    Connect,
    Http,
    Manifest,
    Platform,
    Unknown,
    Rejected(&'static str),
}

impl UpdateCheckFailure {
    fn category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Http => "http",
            Self::Manifest | Self::Rejected(_) => "manifest",
            Self::Platform => "platform",
            Self::Unknown => "unknown",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Timeout => "更新检查超时，请重试或从官网下载。",
            Self::Connect => "无法连接更新服务，请检查网络后重试。",
            Self::Http => "更新服务返回异常响应，请稍后重试。",
            Self::Manifest => "更新清单无效，请稍后重试。",
            Self::Platform => "更新清单不支持当前平台，请从官网下载。",
            Self::Unknown => "更新检查未完成，请重试；如持续失败，请导出诊断日志。",
            Self::Rejected(message) => message,
        }
    }
}

fn classify_check_error(error: &tauri_plugin_updater::Error) -> UpdateCheckFailure {
    use tauri_plugin_updater::Error;
    match error {
        Error::Reqwest(error) if error.is_timeout() => UpdateCheckFailure::Timeout,
        Error::Reqwest(error) if error.is_connect() => UpdateCheckFailure::Connect,
        Error::Reqwest(error) if error.status().is_some() => UpdateCheckFailure::Http,
        Error::Reqwest(error) if error.is_decode() => UpdateCheckFailure::Manifest,
        Error::Serialization(_) | Error::Semver(_) => UpdateCheckFailure::Manifest,
        Error::UnsupportedArch
        | Error::UnsupportedOs
        | Error::TargetNotFound(_)
        | Error::TargetsNotFound(_) => UpdateCheckFailure::Platform,
        // ReleaseNotFound does not retain the response status in the pinned
        // updater. String variants do not prove HTTP, DNS or TLS failure either.
        _ => UpdateCheckFailure::Unknown,
    }
}

fn check_failure_summary(failure: &CheckFailure<UpdateCheckFailure>) -> &'static str {
    match failure {
        CheckFailure::TimedOut => "更新检查超时或结果不完整，请重试或从官网下载",
        CheckFailure::NoCandidates => "没有可用的更新端点",
        CheckFailure::Failed(errors) => match errors.first() {
            Some(first) if errors.iter().all(|error| error == first) => first.summary(),
            _ => UpdateCheckFailure::Unknown.summary(),
        },
    }
}

fn check_route_event(direct: bool, started: Instant, outcome: &'static str) -> DiagnosticEvent {
    update_event("check_route", outcome)
        .field("route", if direct { "direct" } else { "default" })
        .field("elapsed_ms", started.elapsed().as_millis() as u64)
        .field("cancelled", outcome == "dropped")
}

/// Keep channel selection pinned while the bounded race is in flight. Each
/// updater gets one endpoint, avoiding the plugin's serial endpoint loop.
fn channel_endpoints(app: &AppHandle, beta: bool) -> Result<Vec<tauri::Url>, String> {
    if beta {
        return Ok(vec![tauri::Url::parse(
            crate::config::UPDATE_BETA_MANIFEST_URL,
        )
        .map_err(|e| e.to_string())?]);
    }
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|value| value.get("endpoints"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "更新端点配置缺失".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "更新端点配置无效".to_string())
                .and_then(|url| tauri::Url::parse(url).map_err(|e| e.to_string()))
        })
        .collect()
}

fn channel_updater(
    app: &AppHandle,
    endpoint: tauri::Url,
    timeout: Duration,
    direct: bool,
) -> Result<tauri_plugin_updater::Updater, tauri_plugin_updater::Error> {
    let mut builder = app
        .updater_builder()
        .endpoints(vec![endpoint])?
        .timeout(timeout);
    if direct {
        builder = builder.no_proxy();
    }
    // Windows 上 updater 以 `msiexec /passive /promptrestart` 安装 MSI。passive 没有模态框、
    // 未指定重启选项时，安装目录里有文件被占用，Windows Installer 会不经确认直接重启系统
    // （上游 #8534）。`/promptrestart` 只对应 REBOOTPROMPT，挡不住这一点，因此显式追加
    // REBOOT=ReallySuppress（即 `/norestart`）。旧版本的 updater 不带这个参数，
    // 由 MSI 模板自身的 REBOOT 属性兜底。
    #[cfg(target_os = "windows")]
    {
        builder = builder.installer_arg("REBOOT=ReallySuppress");
    }
    builder.build()
}

fn version_at_least(version: &str, minimum: Option<&str>) -> bool {
    let Ok(version) = semver::Version::parse(version) else {
        return false;
    };
    minimum.is_none_or(|minimum| {
        semver::Version::parse(minimum).is_ok_and(|minimum| version >= minimum)
    })
}

async fn check_update_with_proxy_fallback(
    app: &AppHandle,
    budget: Duration,
    minimum: Option<&str>,
) -> Result<Option<Update>, String> {
    log::debug!("{}", update_event("check", "started"));
    let beta = crate::settings::get_settings().receive_beta_updates;
    let endpoints = channel_endpoints(app, beta).inspect_err(|_| {
        log::warn!(
            "{}",
            update_event("check", "failed").field("error_category", "configuration")
        );
    })?;
    let mut checks = Vec::new();
    for (index, endpoint) in endpoints.into_iter().enumerate() {
        for direct in [false, true] {
            let endpoint = endpoint.clone();
            let delay = policy::CHECK_HEDGE_DELAY * (index as u32 * 2 + u32::from(direct));
            checks.push((delay, async move {
                // Created after the existing hedge delay: an unstarted backup
                // is not reported as a failed or cancelled network request.
                let started = Instant::now();
                let mut diagnostic =
                    UpdateDiagnostic::new(|outcome| check_route_event(direct, started, outcome));
                let result = async {
                    let update = channel_updater(app, endpoint, budget, direct)
                        .map_err(|error| classify_check_error(&error))?
                        .check()
                        .await
                        .map_err(|error| classify_check_error(&error))?;
                    match &update {
                        Some(update) if policy::is_withdrawn_version(&update.version) => {
                            return Err(UpdateCheckFailure::Rejected(
                                "该更新已撤回，请检查更新版本",
                            ))
                        }
                        Some(update) if !version_at_least(&update.version, minimum) => {
                            return Err(UpdateCheckFailure::Rejected(
                                "清单版本低于已发现的更新，保留已有更新并重试",
                            ))
                        }
                        Some(update)
                            if !beta
                                && semver::Version::parse(&update.version)
                                    .is_ok_and(|v| !v.pre.is_empty()) =>
                        {
                            return Err(UpdateCheckFailure::Rejected("稳定通道返回了测试版更新"))
                        }
                        None if minimum.is_some() => {
                            return Err(UpdateCheckFailure::Rejected(
                                "清单未确认已有更新，保留预下载产物并重试",
                            ))
                        }
                        _ => {}
                    }
                    Ok(update)
                }
                .await;
                match &result {
                    Ok(_) => diagnostic.finish("completed", None, None),
                    Err(error) => diagnostic.finish("failed", Some(error.category()), None),
                }
                result
            }));
        }
    }
    let result = policy::race_update_checks(checks, budget, |a, b| {
        semver::Version::parse(&a.version).ok() > semver::Version::parse(&b.version).ok()
    })
    .await
    .map_err(|error| {
        let category = match &error {
            CheckFailure::TimedOut => "timeout",
            CheckFailure::NoCandidates => "no_candidates",
            CheckFailure::Failed(_) => "endpoint_failure",
        };
        log::warn!(
            "{}",
            update_event("check", "failed").field("error_category", category)
        );
        check_failure_summary(&error).to_string()
    })?;
    if beta != crate::settings::get_settings().receive_beta_updates {
        log::warn!(
            "{}",
            update_event("check", "failed").field("error_category", "channel_changed")
        );
        return Err("更新通道已改变，请重新检查".to_string());
    }
    log::debug!(
        "{}",
        update_event("check", "completed").field("update_available", result.is_some())
    );
    Ok(result)
}

/// 应用更新下载进度（通过 `update-download-progress` 事件发给前端）。
/// 前端只在安装弹窗打开时监听；后台预下载发出的同名事件无人监听，无副作用。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDownloadProgress {
    attempt_id: u64,
    request_id: Option<String>,
    phase: &'static str,
    source: &'static str,
    downloaded: u64,
    total: Option<u64>,
}

/// 预下载安装包落盘目录（应用缓存目录下的自有子目录）。产物跨会话保留，
/// 由启动闸门（`apply_pending_staged_update_on_startup`）在下一次启动时
/// 重验后消费；周期检查的 reconcile（`DropStaged`）负责清理失效产物。
fn staged_update_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|dir| dir.join("app-updates"))
}

/// 启动闸门的纯决策：给定落盘产物版本、本次清单版本与用户跳过的版本，
/// 对这份产物该做什么。与 I/O 分离以便单测（`Update` 无法在测试中构造）。
#[derive(Debug, PartialEq, Eq)]
enum PendingUpdateAction {
    /// 没有落盘产物（逐文件循环里不会出现，留给矩阵测试表达完整状态空间）。
    NothingStaged,
    /// 产物版本被用户「跳过本版本」：不自动装，保留供手动升级。
    KeepForManual,
    /// 更新已撤回或已有更新版本：产物失效，删除。
    Drop,
    /// 产物与本次清单同版本且未被跳过：立即安装。
    Install,
}

fn pending_update_action(
    staged: Option<&str>,
    offered: Option<&str>,
    dismissed: Option<&str>,
) -> PendingUpdateAction {
    let Some(staged) = staged else {
        return PendingUpdateAction::NothingStaged;
    };
    // 跳过优先于一切：用户明确不装这个版本，哪怕它仍在线上
    //（若已被撤回，周期检查的 reconcile 会随后清掉产物）。
    if dismissed == Some(staged) {
        return PendingUpdateAction::KeepForManual;
    }
    match offered {
        Some(offered) if offered == staged => PendingUpdateAction::Install,
        // 清单没有更新（撤回）或版本对不上（产物过时）：失效。
        _ => PendingUpdateAction::Drop,
    }
}

/// 列出预下载目录里的 `(版本, 安装包路径)`。文件名即版本
/// （`prestage_download` 的落盘约定）。
fn list_staged_installers(dir: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let version = path
                .file_name()?
                .to_str()?
                .strip_suffix(".installer")?
                .to_string();
            Some((version, path))
        })
        .collect()
}

fn seed_staged_version_floor(app: &AppHandle) {
    let beta = crate::settings::get_settings().receive_beta_updates;
    let stage = app.state::<AppUpdateStage>();
    stage.align_channel(beta);
    let Some(dir) = staged_update_dir(app) else {
        return;
    };
    let artifacts = list_staged_installers(&dir);
    if let Some(version) = policy::staged_version_floor(
        artifacts.iter().map(|(version, _)| version.as_str()),
        &app.package_info().version.to_string(),
        beta,
    ) {
        let _ = stage.remember_version(&version);
    }
}

/// 从 tauri.conf.json 的插件配置读 updater 公钥（与插件同一份配置，不复制）。
fn updater_pubkey(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")?
        .get("pubkey")?
        .as_str()
        .map(str::to_owned)
}

/// minisign 验签，逐字对齐 updater 插件 `download()` 内联的 verify_signature
/// （base64 解码 → key/signature decode → verify quiet）。
fn verify_installer_signature(
    bytes: &[u8],
    release_signature: &str,
    pub_key: &str,
    version: &str,
    require_signed_version: bool,
) -> Result<(), String> {
    let event = |outcome| update_event("verify", outcome).field("byte_count", bytes.len() as u64);
    log::debug!("{}", event("started"));
    let failure = |category| log::warn!("{}", event("failed").field("error_category", category));
    let decode = |value: &str, what: &str, category: &'static str| {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .ok()
            .and_then(|decoded| String::from_utf8(decoded).ok())
            .ok_or_else(|| {
                failure(category);
                format!("{what} 不是有效的 base64/UTF-8")
            })
    };
    let public_key =
        PublicKey::decode(&decode(pub_key, "更新公钥", "key_encoding")?).map_err(|e| {
            failure("key_decode");
            format!("更新公钥解析失败: {e}")
        })?;
    let signature = Signature::decode(&decode(
        release_signature,
        "更新签名",
        "signature_encoding",
    )?)
    .map_err(|e| {
        failure("signature_decode");
        format!("更新签名解析失败: {e}")
    })?;
    public_key.verify(bytes, &signature, true).map_err(|e| {
        failure("signature_verification");
        format!("更新签名校验失败: {e}")
    })?;
    policy::verify_signed_version(signature.trusted_comment(), version, require_signed_version)
        .inspect_err(|_| failure("signed_version"))?;
    log::debug!("{}", event("completed"));
    Ok(())
}

/// 启动闸门：应用上个会话预下载好的更新。
///
/// 在 setup 里同步调用（托盘/状态就绪之后、窗口显示/代理恢复/维护任务之前），
/// 「重开即自动更新」在这里闭环：安装成功路径不返回（Windows 是 spawn
/// 安装器后退出、安装器装完自动拉起新版；macOS/Linux 是 install 后
/// restart_process），能返回到调用方就说明本次启动不装，继续正常启动。
///
/// 决策顺序本地优先：没有预下载产物、或产物版本被用户「跳过本版本」→ 直接
/// 返回（跳过时保留产物，手动升级仍可瞬装）；只有存在候选产物时才拉一次
/// 清单（5s 超时，失败保留产物下次再试）。安装前对产物字节做 minisign 重验
/// ——updater 插件的验签在 `download()` 内联、不跨进程，`install(bytes)` 不
/// 重复校验，跨会话复用落盘文件必须自己重新建立信任。
pub async fn apply_pending_staged_update_on_startup(app: &AppHandle) {
    // A retained upgrade checkpoint still owns recovery and explicit restart.
    // Keep staged installers for the existing owner after review completes.
    if crate::secrets::upgrade::checkpoint::ensure_no_pending_checkpoint(
        &crate::live::engine::DeviceStore::for_device(),
    )
    .is_err()
    {
        return;
    }

    // 便携版不能原地升级：不自动装，也不动预下载产物（本来也不会产生）。
    if crate::commands::portable_mode_enabled() {
        return;
    }
    let Some(dir) = staged_update_dir(app) else {
        return;
    };
    let staged_files = list_staged_installers(&dir);
    if staged_files.is_empty() {
        return;
    }
    seed_staged_version_floor(app);
    let minimum = app.state::<AppUpdateStage>().minimum_version();
    let dismissed = crate::settings::get_settings().dismissed_update_version;

    let offered = match check_update_with_proxy_fallback(
        app,
        policy::STARTUP_CHECK_BUDGET,
        minimum.as_deref(),
    )
    .await
    {
        Ok(update) => update,
        Err(_) => {
            log::info!(
                "{}",
                update_event("startup", "deferred").field("reason", "manifest_unavailable")
            );
            return;
        }
    };
    let offered_version = offered.as_ref().map(|update| update.version.as_str());

    let mut install_candidate: Option<std::path::PathBuf> = None;
    for (version, installer) in staged_files {
        match pending_update_action(Some(&version), offered_version, dismissed.as_deref()) {
            PendingUpdateAction::NothingStaged => {}
            PendingUpdateAction::KeepForManual => {
                log::debug!(
                    "{}",
                    update_event("startup", "deferred").field("reason", "dismissed")
                );
            }
            PendingUpdateAction::Drop => {
                log::info!(
                    "{}",
                    update_event("startup", "discarding").field("reason", "obsolete")
                );
                let _ = std::fs::remove_file(&installer);
            }
            PendingUpdateAction::Install => install_candidate = Some(installer),
        }
    }
    // 决策矩阵保证 Install 仅在清单版本等于产物版本时出现。
    let (Some(installer), Some(update)) = (install_candidate, offered) else {
        return;
    };

    let bytes = match tokio::fs::read(&installer).await {
        Ok(bytes) => bytes,
        Err(_) => {
            log::warn!(
                "{}",
                update_event("startup", "failed").field("error_category", "cache_read")
            );
            let _ = std::fs::remove_file(&installer);
            return;
        }
    };
    let Some(pubkey) = updater_pubkey(app) else {
        log::error!(
            "{}",
            update_event("startup", "failed").field("error_category", "missing_public_key")
        );
        return;
    };
    if verify_installer_signature(
        &bytes,
        &update.signature,
        &pubkey,
        &update.version,
        require_signed_version(app),
    )
    .is_err()
    {
        let _ = std::fs::remove_file(&installer);
        return;
    }
    // 字节已读入内存且验签通过，产物文件使命完成；先删再装，成功路径
    //（不返回）不留残留，下次启动也不用再靠 reconcile 清。
    let _ = std::fs::remove_file(&installer);
    if install_bytes_and_restart(app, update, bytes).await.is_err() {
        // 安装失败不算致命：正常启动，用户仍可手动升级（届时现场重下）。
        log::error!(
            "{}",
            update_event("startup", "failed").field("error_category", "install")
        );
    }
}

fn require_signed_version(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|value| value.get("requireSignedVersion"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The check identity and staged transition are committed by one state lock.
fn schedule_prestage(app: &AppHandle, check: u64, available: Option<Update>) -> Result<(), String> {
    let stage = app.state::<AppUpdateStage>();
    if crate::commands::portable_mode_enabled() {
        return stage.remember_for_check(
            check,
            available.as_ref().map(|update| update.version.as_str()),
        );
    }
    let (action, ticket) = stage.begin_for_check(
        check,
        available.as_ref().map(|update| update.version.as_str()),
    )?;
    if action == PrestageAction::Start {
        let update = available.expect("Start has an Update");
        let ticket = ticket.expect("Start has an attempt");
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            prestage_download(app, update, ticket).await;
        });
    }
    Ok(())
}

async fn prestage_download(app: AppHandle, update: Update, mut ticket: DownloadTicket) {
    let id = ticket.id;
    let event = |outcome| update_event("prestage", outcome).field("attempt_id", id);
    let mut diagnostic = UpdateDiagnostic::new(event);
    let version = update.version.clone();
    let result = async {
        let dir = staged_update_dir(&app).ok_or_else(|| "无法读取更新缓存目录".to_string())?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("创建更新缓存目录失败: {e}"))?;
        // Old and new same-version tasks never write the same partial path.
        let temporary = dir.join(format!("{version}.{id}.partial"));
        let bytes = tokio::select! {
            biased;
            _ = &mut ticket.cancel => {
                diagnostic.finish("cancelled", None, None);
                return Err("预下载已被较新的更新取代".to_string());
            },
            result = download_update_bytes(&app, &update, id, None) => result?,
        };
        if let Err(error) = tokio::fs::write(&temporary, bytes).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(format!("保存预下载更新失败: {error}"));
        }
        let ready = ReadyUpdate {
            update,
            installer: temporary.clone(),
        };
        let published = app.state::<AppUpdateStage>().set_ready(
            id,
            ready,
            dir.join(format!("{version}.installer")),
        );
        match published {
            Ok(true) => Ok(()),
            Ok(false) => {
                diagnostic.finish("superseded", None, None);
                Err("预下载已被较新的更新取代".to_string())
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(temporary).await;
                Err(error)
            }
        }
    }
    .await;
    if result.is_err() {
        diagnostic.finish("failed", Some("download_or_storage"), None);
        app.state::<AppUpdateStage>().set_failed(id);
    } else {
        diagnostic.finish("completed", None, None);
    }
    if let Some(done) = ticket.done.take() {
        let _ = done.send(result);
    }
}

fn emit_download_progress(
    app: &AppHandle,
    id: u64,
    phase: &'static str,
    source: &'static str,
    downloaded: u64,
    total: Option<u64>,
) {
    if let Some(request_id) = app.state::<AppUpdateStage>().progress_request(id) {
        let _ = app.emit(
            "update-download-progress",
            UpdateDownloadProgress {
                attempt_id: id,
                request_id,
                phase,
                source,
                downloaded,
                total,
            },
        );
    }
}

async fn probe_download_route(
    update: &Update,
    route: &policy::DownloadRoute,
) -> (policy::ProbeResult, Duration) {
    let started = tokio::time::Instant::now();
    let probe = async {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
        if route.direct {
            builder = builder.no_proxy();
        } else if let Some(proxy) = &update.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy.as_str()).ok()?);
        }
        let response = builder
            .build()
            .ok()?
            .get(&route.url)
            .headers(update.headers.clone())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .ok()?;
        let status = response.status().as_u16();
        // Never read the body, including when a server ignores Range with 200.
        drop(response);
        Some(status)
    };
    let status = tokio::time::timeout(Duration::from_secs(5), probe)
        .await
        .ok()
        .flatten();
    (
        policy::probe_result(route.source == "official", status),
        started.elapsed(),
    )
}

fn download_routes(
    update: &Update,
    preferred: Option<&str>,
) -> Result<Vec<policy::DownloadRoute>, String> {
    let urls = if let Some((official, github)) =
        policy::trusted_asset_pair(update.download_url.as_str(), &update.version)
    {
        vec![(official, "official"), (github, "github")]
    } else {
        if preferred.is_some() {
            return Err("这个安装包没有已验证的替代下载源".into());
        }
        vec![(update.download_url.to_string(), "manifest")]
    };
    Ok(urls
        .into_iter()
        .flat_map(|(url, source)| {
            [update.no_proxy, !update.no_proxy]
                .into_iter()
                .map(move |direct| policy::DownloadRoute {
                    url: url.clone(),
                    source,
                    direct,
                })
        })
        .collect())
}

/// Download only one full package at a time. All retries retain the Update's
/// original target, version, signature, public key and plugin verification.
async fn download_update_bytes(
    app: &AppHandle,
    update: &Update,
    id: u64,
    preferred: Option<&str>,
) -> Result<Vec<u8>, String> {
    let event = |outcome| update_event("download", outcome).field("attempt_id", id);
    let mut diagnostic = UpdateDiagnostic::new(event);
    if policy::is_withdrawn_version(&update.version) {
        diagnostic.finish("failed", Some("withdrawn"), None);
        return Err("该版本已撤回，请检查更新版本".into());
    }
    let deadline = tokio::time::Instant::now() + policy::DOWNLOAD_TOTAL_BUDGET;
    let stage = app.state::<AppUpdateStage>();
    let _download = tokio::time::timeout_at(deadline, stage.download.lock())
        .await
        .map_err(|_| {
            diagnostic.finish("failed", Some("queue_timeout"), None);
            "等待现有更新下载超时，请重试".to_string()
        })?;
    emit_download_progress(app, id, "probing", "official", 0, None);
    let routes = download_routes(update, preferred).inspect_err(|_| {
        diagnostic.finish("failed", Some("untrusted_alternative"), None);
    })?;
    let probes = tokio::time::timeout_at(
        deadline,
        futures::future::join_all(routes.into_iter().map(|route| async move {
            let (result, latency) = probe_download_route(update, &route).await;
            (route, result, latency)
        })),
    )
    .await
    .map_err(|_| {
        diagnostic.finish("failed", Some("probe_timeout"), None);
        "更新下载已达总时限，请换源重试或从官网下载".to_string()
    })?;
    let routes = policy::rank_download_routes(probes, preferred)
        .inspect_err(|_| diagnostic.finish("failed", Some("no_routes"), None))
        .map_err(str::to_string)?;
    let mut failures = Vec::new();
    for (index, route) in routes.into_iter().enumerate() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            diagnostic.finish("failed", Some("total_timeout"), None);
            return Err("更新下载已达总时限，请换源重试或从官网下载".into());
        }
        emit_download_progress(
            app,
            id,
            if index == 0 {
                "downloading"
            } else {
                "switchingSource"
            },
            route.source,
            0,
            None,
        );
        let route_event = |outcome| {
            update_event("download_route", outcome)
                .field("attempt_id", id)
                .field("route_index", index as u64)
                .field(
                    "source",
                    match route.source {
                        "official" => "official",
                        "github" => "github",
                        "manifest" => "manifest",
                        _ => "other",
                    },
                )
                .field("direct", route.direct)
        };
        let mut route_diagnostic = UpdateDiagnostic::new(route_event);
        let mut candidate = update.clone();
        candidate.download_url = tauri::Url::parse(&route.url).map_err(|e| {
            route_diagnostic.finish("failed", Some("invalid_url"), None);
            diagnostic.finish("failed", Some("invalid_url"), None);
            e.to_string()
        })?;
        candidate.no_proxy = route.direct;
        candidate.timeout = Some(remaining);
        let (progress, receiver) = watch::channel(0_u64);
        let mut downloaded = 0_u64;
        let download = candidate.download(
            |length, total| {
                downloaded = downloaded.saturating_add(length as u64);
                progress.send_replace(downloaded);
                emit_download_progress(app, id, "downloading", route.source, downloaded, total);
            },
            || {},
        );
        match policy::bounded_download(download, receiver, policy::DOWNLOAD_IDLE_BUDGET, remaining)
            .await
        {
            Ok(bytes) => {
                route_diagnostic.finish("completed", None, Some(bytes.len() as u64));
                diagnostic.finish("completed", None, Some(bytes.len() as u64));
                return Ok(bytes);
            }
            Err(DownloadFailure::TimedOut) => {
                route_diagnostic.finish("failed", Some("total_timeout"), Some(downloaded));
                diagnostic.finish("failed", Some("total_timeout"), None);
                return Err("更新下载已达总时限，请换源重试或从官网下载".into());
            }
            Err(DownloadFailure::Stalled) => {
                route_diagnostic.finish("failed", Some("stalled"), Some(downloaded));
                failures.push(format!("{}：60秒无下载进展", route.source))
            }
            Err(DownloadFailure::Operation(error)) => {
                // Updater 2.13.2 exposes HTTP status only in this typed Network
                // variant. Match its exact generated 410 message, never an
                // arbitrary error substring, and never bypass a withdrawal.
                let withdrawn = matches!(&error, tauri_plugin_updater::Error::Network(message)
                    if message == &format!("Download request failed with status: {}", reqwest::StatusCode::GONE));
                let category = if withdrawn {
                    "withdrawn"
                } else {
                    match &error {
                        tauri_plugin_updater::Error::Reqwest(_)
                        | tauri_plugin_updater::Error::Network(_) => "network",
                        _ => "verification_or_download",
                    }
                };
                route_diagnostic.finish("failed", Some(category), Some(downloaded));
                if withdrawn {
                    diagnostic.finish("failed", Some("withdrawn"), None);
                    return Err("该版本已撤回，请检查更新版本".into());
                }
                if !matches!(
                    &error,
                    tauri_plugin_updater::Error::Reqwest(_)
                        | tauri_plugin_updater::Error::Network(_)
                ) {
                    diagnostic.finish("failed", Some("verification_or_download"), None);
                    return Err(format!("更新校验或下载失败，未安装：{error}"));
                }
                failures.push(format!("{}：{error}", route.source));
            }
        }
        // The previous full download future was dropped before the next starts.
    }
    diagnostic.finish("failed", Some("routes_exhausted"), None);
    Err(format!(
        "所有下载路径均失败，请手动换源重试或从官网下载：{}",
        failures.join("；")
    ))
}

/// 执行安装并按平台收尾。Windows 的安装器会替换并退出当前进程；
/// macOS/Linux 先安装再走统一的重启路径。
async fn install_bytes_and_restart(
    app: &AppHandle,
    update: Update,
    bytes: Vec<u8>,
) -> Result<bool, String> {
    let event = |outcome| update_event("install", outcome).field("byte_count", bytes.len() as u64);
    log::info!("{}", event("started"));
    if policy::is_withdrawn_version(&update.version) {
        log::warn!("{}", event("failed").field("error_category", "withdrawn"));
        return Err("该版本已撤回，请检查更新版本".into());
    }
    if !crate::settings::get_settings().receive_beta_updates
        && semver::Version::parse(&update.version).is_ok_and(|version| !version.pre.is_empty())
    {
        log::warn!(
            "{}",
            event("failed").field("error_category", "channel_changed")
        );
        return Err("已关闭测试版更新，未安装这份测试版，请重新检查".into());
    }
    if !version_at_least(
        &update.version,
        app.state::<AppUpdateStage>().minimum_version().as_deref(),
    ) {
        log::warn!(
            "{}",
            event("failed").field("error_category", "version_floor")
        );
        return Err("已发现更高版本，未安装较旧的更新，请重试".into());
    }
    let byte_count = bytes.len() as u64;

    #[cfg(target_os = "windows")]
    {
        // Windows updater 会在 install() 内启动安装器并直接退出当前进程
        // （插件内部 std::process::exit(0)，绕过 TrayIcon::drop、不发
        // NIM_DELETE，会残留死图标——与托盘"退出"路径相同的问题）。
        // 因此清理只能放在 install 前执行，且必须显式移除托盘图标。
        // 应用退出与安装器写文件的竞速由 NSIS 钩子（installer-hooks.nsh 的
        // WaitForMainBinaryWritable）在安装器侧等待兜底，应用侧无需再睡。
        crate::save_window_state_before_exit(app);
        crate::cleanup_before_exit(app).await;
        crate::remove_tray_icon_before_exit(app);
        crate::destroy_single_instance_lock(app);
        update.install(bytes).map_err(|e| {
            log::error!("{}", update_event("install", "failed").field("byte_count", byte_count).field("error_category", "installer"));
            format!(
                "Windows 更新安装失败: {e}。已执行退出前清理，代理或 Live 接管可能已暂停，套餐领取已暂停至重启；请重启应用后再试。"
            )
        })?;
        log::info!(
            "{}",
            update_event("install", "completed").field("byte_count", byte_count)
        );
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS/Linux install() 会返回；先安装，避免安装失败时误停代理/撤回接管。
        update.install(bytes).map_err(|e| {
            log::error!(
                "{}",
                update_event("install", "failed")
                    .field("byte_count", byte_count)
                    .field("error_category", "installer")
            );
            format!("安装更新失败: {e}")
        })?;

        crate::save_window_state_before_exit(app);
        crate::cleanup_before_exit(app).await;

        log::info!(
            "{}",
            update_event("install", "completed").field("byte_count", byte_count)
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        crate::restart_process(app);
    }
}

/// 「点升级」的统一入口：已预下载则瞬装（无需网络），在途则续接进度，
/// 都没有则回退为现场「检查+下载+安装」。
pub async fn install_staged_or_download_and_restart(
    app: &AppHandle,
    preferred: Option<&str>,
    request_id: Option<String>,
) -> Result<bool, String> {
    if crate::commands::portable_mode_enabled() {
        return Err("便携版请从官网下载新版本后手动更新".into());
    }
    if preferred.is_some_and(|source| source != "official" && source != "github") {
        return Err("未知的更新下载源".into());
    }
    let stage = app.state::<AppUpdateStage>();
    let _install = stage
        .install
        .try_lock()
        .map_err(|_| "已有更新安装任务，请等待当前任务结束".to_string())?;
    seed_staged_version_floor(app);
    let minimum = stage.minimum_version();
    if let Some(ready) = stage.take_ready() {
        if let Some(outcome) = install_ready_update(app, ready).await {
            return outcome;
        }
    }
    if let Some(waiting) = stage.take_downloading_done() {
        let _foreground = stage.foreground(waiting.id);
        stage.set_progress_request(request_id.clone());
        if preferred.is_some() {
            // An explicit source retry replaces, rather than races, the old full
            // transfer. The matching cancellation token stops its owned future.
            stage.set_failed(waiting.id);
        }
        let wait_budget = if preferred.is_some() {
            Duration::from_secs(5)
        } else {
            policy::DOWNLOAD_TOTAL_BUDGET + Duration::from_secs(5)
        };
        let completed = tokio::time::timeout(wait_budget, waiting.done).await;
        match completed {
            Err(_) => {
                stage.set_failed(waiting.id);
                return Err("等待预下载超时，任务已取消，请重试".into());
            }
            Ok(Err(_)) if preferred.is_none() => {
                stage.set_failed(waiting.id);
                return Err("预下载已中止，请重试".into());
            }
            Ok(Ok(Err(error))) if preferred.is_none() => return Err(error),
            _ => {}
        }
        if let Some(ready) = stage.take_ready() {
            if let Some(outcome) = install_ready_update(app, ready).await {
                return outcome;
            }
        }
    }
    let Some(update) =
        check_update_with_proxy_fallback(app, policy::CHECK_BUDGET, minimum.as_deref()).await?
    else {
        return Ok(false);
    };
    stage.remember_version(&update.version)?;
    let id = stage.next_id();
    let _foreground = stage.foreground(id);
    stage.set_progress_request(request_id);
    let bytes = download_update_bytes(app, &update, id, preferred).await?;
    install_bytes_and_restart(app, update, bytes).await
}

/// 消费就绪的预下载产物：先读后删；读失败返回 None 让调用方退回全量流程。
async fn install_ready_update(
    app: &AppHandle,
    ready: Box<ReadyUpdate<Update>>,
) -> Option<Result<bool, String>> {
    match tokio::fs::read(&ready.installer).await {
        Ok(bytes) => {
            if policy::is_withdrawn_version(&ready.update.version) {
                log::warn!(
                    "{}",
                    update_event("cached_install", "failed").field("error_category", "withdrawn")
                );
                let _ = std::fs::remove_file(&ready.installer);
                return Some(Err("该版本已撤回，请检查更新版本".into()));
            }
            let Some(pubkey) = updater_pubkey(app) else {
                log::warn!(
                    "{}",
                    update_event("cached_install", "failed")
                        .field("error_category", "missing_public_key")
                );
                return Some(Err("更新公钥缺失，无法验证缓存安装包".into()));
            };
            if let Err(error) = verify_installer_signature(
                &bytes,
                &ready.update.signature,
                &pubkey,
                &ready.update.version,
                require_signed_version(app),
            ) {
                let _ = std::fs::remove_file(&ready.installer);
                return Some(Err(error));
            }
            let _ = std::fs::remove_file(&ready.installer);
            Some(install_bytes_and_restart(app, ready.update, bytes).await)
        }
        Err(_) => {
            log::warn!(
                "{}",
                update_event("cached_install", "failed").field("error_category", "cache_read")
            );
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
    let stage = app.state::<AppUpdateStage>();
    seed_staged_version_floor(app);
    let check = stage.begin_check();
    let minimum = stage.minimum_version();
    let update = check_update_with_proxy_fallback(app, policy::CHECK_BUDGET, minimum.as_deref())
        .await
        .map_err(|error| AppError::Message(format!("检查更新失败: {error}")))?;

    let Some(update) = update else {
        // 检查不到更新也走一次 reconcile：应用跨版本运行期间，暂存的旧版
        // 产物应当被丢弃（例如 latest.json 撤回后）。
        schedule_prestage(app, check, None).map_err(AppError::Message)?;
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
    schedule_prestage(app, check, Some(update)).map_err(AppError::Message)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{pending_update_action, AppUpdateCheckResult, PendingUpdateAction};

    #[test]
    fn pending_update_action_matrix() {
        use PendingUpdateAction::*;
        // 没有落盘产物：闸门无事可做。
        assert_eq!(
            pending_update_action(None, Some("6.18.1"), None),
            NothingStaged
        );
        assert_eq!(pending_update_action(None, None, None), NothingStaged);
        // 产物与清单同版本、未跳过：安装。
        assert_eq!(
            pending_update_action(Some("6.18.1"), Some("6.18.1"), None),
            Install
        );
        // 用户明确跳过产物版本：保留给手动升级，即使清单仍在线上。
        assert_eq!(
            pending_update_action(Some("6.18.1"), Some("6.18.1"), Some("6.18.1")),
            KeepForManual
        );
        // 跳过的是旧版本，不影响新产物自动安装。
        assert_eq!(
            pending_update_action(Some("6.18.1"), Some("6.18.1"), Some("6.18.0")),
            Install
        );
        // 清单已无更新（撤回）：产物失效。
        assert_eq!(pending_update_action(Some("6.18.1"), None, None), Drop);
        // 清单版本更新（产物过时）：失效，周期检查会预下载新版本。
        assert_eq!(
            pending_update_action(Some("6.18.1"), Some("6.18.2"), None),
            Drop
        );
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

#[cfg(test)]
mod update_diagnostic_tests {
    use super::{update_event, verify_installer_signature, UpdateDiagnostic};
    use base64::Engine as _;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    // A deterministic test-only signer keeps these fixtures independent of
    // release keys, update servers and real installer bytes.
    pub(super) fn signed_fixture(bytes: &[u8], version: &str) -> (String, String) {
        let key = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let prefix = [b"Ed".as_slice(), &[0; 8]].concat();
        let signature = key.sign(bytes);
        let comment =
            format!("version:{version}\tfile:PRIVATE_PATH_CANARY\ttoken:PRIVATE_TOKEN_CANARY");
        let global = key.sign(&[signature.as_ref(), comment.as_bytes()].concat());
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let public = format!(
            "untrusted comment: PRIVATE_KEY_COMMENT_CANARY\n{}",
            encode(&[prefix.as_slice(), key.public_key().as_ref()].concat())
        );
        let signed = format!(
            "untrusted comment: PRIVATE_SIGNATURE_COMMENT_CANARY\n{}\ntrusted comment: {comment}\n{}",
            encode(&[prefix.as_slice(), signature.as_ref()].concat()),
            encode(global.as_ref())
        );
        (encode(public.as_bytes()), encode(signed.as_bytes()))
    }

    #[test]
    fn signed_installer_validation_keeps_its_success_and_failure_contract() {
        let bytes = b"PRIVATE_BODY_CANARY";
        let (key, signature) = signed_fixture(bytes, "6.18.1");
        assert!(verify_installer_signature(bytes, &signature, &key, "6.18.1", true).is_ok());
        let wrong_version = verify_installer_signature(bytes, &signature, &key, "6.18.2", true);
        assert_eq!(wrong_version, Err("更新签名版本与清单不符".into()));
        assert!(verify_installer_signature(b"tampered", &signature, &key, "6.18.1", true).is_err());
        assert_eq!(
            verify_installer_signature(bytes, &signature, "invalid", "6.18.1", true),
            Err("更新公钥 不是有效的 base64/UTF-8".into())
        );
        assert_eq!(
            verify_installer_signature(bytes, "invalid", &key, "6.18.1", true),
            Err("更新签名 不是有效的 base64/UTF-8".into())
        );
    }

    #[test]
    fn update_event_uses_the_existing_structured_envelope() {
        let event = update_event("verify", "failed")
            .field("error_category", "signed_version")
            .field("byte_count", 42_u64);
        let value: serde_json::Value = serde_json::from_str(&event.to_string()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "event": "app.update", "outcome": "failed", "stage": "verify",
                "error_category": "signed_version", "byte_count": 42
            })
        );
    }

    #[test]
    fn lifecycle_guard_closes_once_and_does_not_reclassify_terminal_outcomes() {
        // Enable lazy debug event construction even when this test runs alone.
        // Reuse the existing global logger rather than installing a competing one.
        log::set_max_level(log::LevelFilter::Trace);
        for terminal in [
            None,
            Some("cancelled"),
            Some("superseded"),
            Some("completed"),
            Some("failed"),
        ] {
            let outcomes = std::cell::RefCell::new(Vec::new());
            {
                let mut diagnostic = UpdateDiagnostic::new(|outcome| {
                    outcomes.borrow_mut().push(outcome);
                    update_event("prestage", outcome)
                });
                if let Some(terminal) = terminal {
                    diagnostic.finish(terminal, None, None);
                    diagnostic.finish("failed", Some("download_or_storage"), None);
                }
            }
            assert_eq!(
                *outcomes.borrow(),
                ["started", terminal.unwrap_or("dropped")]
            );
        }
    }
}

#[cfg(test)]
mod check_diagnostic_tests {
    use super::{
        check_failure_summary, check_route_event, classify_check_error, policy, CheckFailure,
        UpdateCheckFailure, UpdateDiagnostic,
    };
    use futures::future::{pending, BoxFuture};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri_plugin_updater::Error;

    #[test]
    fn typed_check_errors_are_classified_without_reading_error_text() {
        assert_eq!(
            classify_check_error(&Error::Serialization(
                serde_json::from_str::<serde_json::Value>("invalid").unwrap_err()
            )),
            UpdateCheckFailure::Manifest
        );
        assert_eq!(
            classify_check_error(&Error::Semver(
                semver::Version::parse("invalid").unwrap_err()
            )),
            UpdateCheckFailure::Manifest
        );
        for error in [
            Error::UnsupportedArch,
            Error::UnsupportedOs,
            Error::TargetNotFound("PRIVATE_TARGET_CANARY".into()),
            Error::TargetsNotFound(vec!["PRIVATE_TARGET_CANARY".into()]),
        ] {
            assert_eq!(classify_check_error(&error), UpdateCheckFailure::Platform);
        }
        // The plugin discards non-success HTTP statuses before this variant.
        // Neither it nor a string containing DNS/TLS/HTTP terms proves a cause.
        for error in [
            Error::ReleaseNotFound,
            Error::Network("DNS TLS HTTP 503 timeout https://user:password@example.invalid/?token=PRIVATE_TOKEN_CANARY Authorization: Bearer PRIVATE_HEADER_CANARY".into()),
            Error::Io(std::io::Error::other("PRIVATE_PATH_CANARY")),
        ] {
            let classified = classify_check_error(&error);
            assert_eq!(classified, UpdateCheckFailure::Unknown);
            let summary = check_failure_summary(&CheckFailure::Failed(vec![classified, classified]));
            assert_eq!(summary, "更新检查未完成，请重试；如持续失败，请导出诊断日志。");
            assert!(!summary.contains("PRIVATE_"));
            assert!(!summary.contains("https://"));
        }
    }

    #[test]
    fn duplicate_failures_have_one_fixed_summary_and_mixed_failures_stay_unspecified() {
        use UpdateCheckFailure::*;
        for (error, category, message) in [
            (Timeout, "timeout", "更新检查超时，请重试或从官网下载。"),
            (Connect, "connect", "无法连接更新服务，请检查网络后重试。"),
            (Http, "http", "更新服务返回异常响应，请稍后重试。"),
            (Manifest, "manifest", "更新清单无效，请稍后重试。"),
            (
                Platform,
                "platform",
                "更新清单不支持当前平台，请从官网下载。",
            ),
            (
                Unknown,
                "unknown",
                "更新检查未完成，请重试；如持续失败，请导出诊断日志。",
            ),
        ] {
            assert_eq!(error.category(), category);
            assert_eq!(
                check_failure_summary(&CheckFailure::Failed(vec![error, error])),
                message
            );
        }
        assert_eq!(
            check_failure_summary(&CheckFailure::Failed(vec![Connect, Manifest])),
            Unknown.summary()
        );
        assert_eq!(
            check_failure_summary(&CheckFailure::Failed(vec![])),
            Unknown.summary()
        );
        assert_eq!(
            check_failure_summary(&CheckFailure::TimedOut),
            "更新检查超时或结果不完整，请重试或从官网下载"
        );
        assert_eq!(
            check_failure_summary(&CheckFailure::NoCandidates),
            "没有可用的更新端点"
        );
        let rejection = Rejected("清单未确认已有更新，保留预下载产物并重试");
        assert_eq!(
            check_failure_summary(&CheckFailure::Failed(vec![rejection, rejection])),
            "清单未确认已有更新，保留预下载产物并重试"
        );
    }

    #[test]
    fn route_event_has_only_fixed_labels_elapsed_time_and_cancellation() {
        for direct in [false, true] {
            let event = check_route_event(direct, Instant::now(), "dropped");
            let value: serde_json::Value = serde_json::from_str(&event.to_string()).unwrap();
            assert_eq!(value["event"], "app.update");
            assert_eq!(value["stage"], "check_route");
            assert_eq!(value["route"], if direct { "direct" } else { "default" });
            assert_eq!(value["outcome"], "dropped");
            assert_eq!(value["cancelled"], true);
            assert!(value["elapsed_ms"].is_u64());
            assert_eq!(value.as_object().unwrap().len(), 6);
        }
    }

    type Events = Arc<Mutex<Vec<serde_json::Value>>>;
    type Check = BoxFuture<'static, Result<Option<u8>, UpdateCheckFailure>>;

    // Exercise the existing diagnostic guard and race together using the real
    // route envelope. No HTTP requests, updater state or global logger replaced.
    fn observed_check(direct: bool, events: Events, check: Check) -> Check {
        Box::pin(async move {
            let started = Instant::now();
            let mut diagnostic = UpdateDiagnostic::new(|outcome| {
                let event = check_route_event(direct, started, outcome);
                // finish constructs its event even when logging is disabled.
                // Ignore debug-only starts; never mutate the global log level.
                if outcome != "started" {
                    events
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str(&event.to_string()).unwrap());
                }
                event
            });
            let result = check.await;
            match &result {
                Ok(_) => diagnostic.finish("completed", None, None),
                Err(error) => diagnostic.finish("failed", Some(error.category()), None),
            }
            result
        })
    }

    #[tokio::test]
    async fn both_failed_routes_close_once_and_produce_one_summary() {
        let events = Events::default();
        let checks = [false, true]
            .into_iter()
            .map(|direct| {
                (
                    Duration::ZERO,
                    observed_check(
                        direct,
                        events.clone(),
                        Box::pin(async { Err(UpdateCheckFailure::Connect) }),
                    ),
                )
            })
            .collect();
        let failure = policy::race_update_checks(checks, Duration::from_secs(1), |a, b| a > b)
            .await
            .unwrap_err();
        assert_eq!(
            check_failure_summary(&failure),
            UpdateCheckFailure::Connect.summary()
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event["outcome"] == "failed")
                .count(),
            2
        );
        assert!(events.iter().all(|event| event["cancelled"] == false));
    }

    #[tokio::test]
    async fn winning_backup_cancels_the_started_primary_without_a_false_failure() {
        let events = Events::default();
        let checks = vec![
            (
                Duration::ZERO,
                observed_check(false, events.clone(), Box::pin(pending())),
            ),
            (
                Duration::from_millis(1),
                observed_check(true, events.clone(), Box::pin(async { Ok(Some(2)) })),
            ),
        ];
        assert_eq!(
            policy::race_update_checks(checks, Duration::from_secs(1), |a, b| a > b)
                .await
                .unwrap(),
            Some(2)
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| event["route"] == "default"
            && event["outcome"] == "dropped"
            && event["cancelled"] == true));
        assert!(events.iter().any(|event| event["route"] == "direct"
            && event["outcome"] == "completed"
            && event["cancelled"] == false));
        assert!(!events.iter().any(|event| event["outcome"] == "failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn total_deadline_cancels_all_started_routes_and_remains_an_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        assert_eq!(policy::CHECK_BUDGET, Duration::from_secs(12));
        assert_eq!(policy::STARTUP_CHECK_BUDGET, Duration::from_secs(5));
        assert_eq!(policy::CHECK_HEDGE_DELAY, Duration::from_millis(750));
        let events = Events::default();
        let started = Arc::new(AtomicUsize::new(0));
        let checks = [false, true]
            .into_iter()
            .map(|direct| {
                let started = started.clone();
                (
                    Duration::ZERO,
                    observed_check(
                        direct,
                        events.clone(),
                        Box::pin(async move {
                            started.fetch_add(1, Ordering::SeqCst);
                            pending().await
                        }),
                    ),
                )
            })
            .collect();
        let race = policy::race_update_checks(checks, Duration::from_millis(5), |a, b| a > b);
        tokio::pin!(race);
        assert!(futures::poll!(&mut race).is_pending());
        // Let both zero-delay routes start before testing cancellation at the deadline.
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(futures::poll!(&mut race).is_pending());
        assert_eq!(started.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_millis(5)).await;
        assert!(matches!(race.await, Err(CheckFailure::TimedOut)));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event["outcome"] == "dropped" && event["cancelled"] == true)
                .count(),
            2
        );
        assert!(!events.iter().any(|event| event["outcome"] == "failed"));
    }
}
