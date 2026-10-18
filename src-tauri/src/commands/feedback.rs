//! 问题反馈回传命令层：multipart 上传到签名配置下发的端点。
//!
//! 载荷形状（与服务端 `crowd-metrics/src/feedback.ts` 同一契约）：
//! 文本字段 `sourceId` / `appVersion` / `description` / `meta`（环境事实 JSON）+
//! 可选文本字段 `signals`（分诊摘要 JSON，见 [`crate::diagnostics_export::build_triage_summary`]）+
//! 图片部件 `screenshots`（≤6 张）+ 可选 `bundle`（诊断包 zip）。截图走独立部件
//! 而不是打进 zip —— 服务端要把它们单独存 R2 引进 issue 正文内联渲染。
//!
//! 端点唯源 [`crate::relay::remote_config::feedback_endpoint`]（v2 config 的
//! `feedback_url`，缺省 = 功能整体休眠，前端按钮不显示）。

use crate::feedback_receipt::{
    AcceptedFeedbackReceipt, FeedbackReceipt, FeedbackReceiptIdentity, FeedbackReceiptStore,
    FeedbackSafetySummary,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::diagnostics_export::{
    build_diagnostics_zip, collect_diagnostics, sanitize_attachment_name,
};
use crate::error::AppError;
use crate::store::AppState;

/// 单张截图上限。与服务端体积闸对齐（服务端另有整包闸兜底）。
const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;
/// 截图张数上限。
const MAX_SCREENSHOTS: usize = 6;
/// 整次上传上限（截图合计 + 诊断包）：与服务端 `feedback.ts` 同一数值，客户端先拦一次。
const MAX_TOTAL_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// 描述字符上限（与服务端一致；超长直接拒，不静默截断用户的描述）。
const MAX_DESCRIPTION_CHARS: usize = 8_000;
const UPLOAD_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackScreenshotInput {
    /// 展示名（原文件名或 clipboard-N.png）。
    pub name: String,
    /// 图片字节，data URL（`data:image/...;base64,` 前缀可选）或裸 base64。
    pub base64: String,
}

struct DecodedScreenshot {
    name: String,
    bytes: Vec<u8>,
}

/// 反馈回传端点是否已随签名配置下发。前端按钮显隐的唯一判据。
#[tauri::command]
pub fn feedback_get_endpoint_configured() -> bool {
    crate::relay::remote_config::load_cached()
        .as_ref()
        .and_then(crate::relay::remote_config::feedback_endpoint)
        .is_some()
}

/// Read only the already-verified cached configuration; no network probe.
#[tauri::command]
pub fn feedback_get_form_capabilities() -> Value {
    let supported = crate::relay::remote_config::load_cached()
        .as_ref()
        .is_some_and(crate::relay::remote_config::feedback_extended_fields_supported);
    serde_json::json!({"extendedFields": supported})
}

struct FeedbackDetails {
    related_apps: Vec<String>,
    contact: Option<String>,
}

fn validate_feedback_details(
    related_apps: Vec<String>,
    contact: Option<String>,
    supported: bool,
) -> Result<FeedbackDetails, String> {
    if contact
        .as_ref()
        .is_some_and(|value| value.len() > 4096 || value.encode_utf16().take(1025).count() > 1024)
    {
        return Err("feedback_contact_invalid".to_string());
    }
    if contact
        .as_ref()
        .is_some_and(|value| value.chars().any(char::is_control))
    {
        return Err("feedback_contact_invalid".to_string());
    }
    let contact = contact
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if contact
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err("feedback_contact_invalid".to_string());
    }
    if related_apps.len() > 12
        || related_apps.iter().any(|id| {
            id != "zcode"
                && id != "other"
                && !crate::app_config::AppType::all().any(|app| app.as_str() == id)
        })
    {
        return Err("feedback_related_apps_invalid".to_string());
    }
    if !supported {
        if contact.is_some() {
            return Err("feedback_contact_unsupported".to_string());
        }
        if !related_apps.is_empty() {
            return Err("feedback_extended_fields_unsupported".to_string());
        }
    }
    let ordered: Vec<String> = [
        "claude",
        "claude-desktop",
        "codex",
        "codex-image",
        "pi",
        "grokbuild",
        "gemini",
        "opencode",
        "openclaw",
        "hermes",
        "zcode",
        "other",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let unique = ordered
        .into_iter()
        .filter(|id| related_apps.contains(id))
        .collect();
    Ok(FeedbackDetails {
        related_apps: unique,
        contact,
    })
}

/// 读剪贴板里的截图（arboard，复用既有依赖；与 `copy_text_to_clipboard` 同款
/// spawn_blocking 形状 —— 剪贴板访问在部分平台有线程/事件循环约束）。
///
/// 返回 RGBA 原始字节 + 尺寸，PNG 编码留给前端 canvas（web 标准路径，
/// 不为此引 Rust 图像库）。剪贴板里没有图片 = `Err`，前端按提示处理。
#[tauri::command]
pub async fn read_clipboard_image() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("访问系统剪贴板失败: {e}"))?;
        let image = clipboard
            .get_image()
            .map_err(|_| "剪贴板里没有图片".to_string())?;
        Ok(serde_json::json!({
            "width": image.width,
            "height": image.height,
            "rgbaBase64": base64::engine::general_purpose::STANDARD.encode(image.bytes.as_ref()),
        }))
    })
    .await
    .map_err(|e| format!("读取剪贴板任务失败: {e}"))?
}

/// 解码并校验截图输入（纯函数，便于单测）。
fn decode_screenshots(
    inputs: &[FeedbackScreenshotInput],
) -> Result<Vec<DecodedScreenshot>, String> {
    if inputs.len() > MAX_SCREENSHOTS {
        return Err(format!("截图最多 {MAX_SCREENSHOTS} 张"));
    }
    let mut attachments = Vec::with_capacity(inputs.len());
    for input in inputs {
        let payload = match input.base64.find(",") {
            Some(index) if input.base64.starts_with("data:") => &input.base64[index + 1..],
            _ => input.base64.as_str(),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|e| format!("截图 {} 不是合法的 base64: {e}", input.name))?;
        if bytes.len() > MAX_SCREENSHOT_BYTES {
            return Err(format!("截图 {} 超过单张 5MB 上限", input.name));
        }
        attachments.push(DecodedScreenshot {
            name: sanitize_attachment_name(&input.name),
            bytes,
        });
    }
    Ok(attachments)
}

/// 截图部件的 MIME：按文件名后缀映射（前端保证扩展名；非图片后缀按 octet-stream
/// 直存，只是不进 issue 正文内联）。
fn image_mime_for_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "application/octet-stream"
    }
}

// A single in-flight submission prevents a late successful response replacing
// a newer receipt. This is admission only; unknown outcomes are never replayed.
static UPLOAD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
struct UploadAdmission;
impl UploadAdmission {
    fn acquire() -> Result<Self, String> {
        UPLOAD_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "已有反馈正在提交".to_string())
    }
}
impl Drop for UploadAdmission {
    fn drop(&mut self) {
        UPLOAD_IN_FLIGHT.store(false, Ordering::Release);
    }
}

#[derive(Debug, PartialEq)]
enum ServerAcceptance {
    Accepted { uploaded_at: String },
    LegacyAccepted { number: u64 },
    Unknown,
}
fn parse_server_acceptance(body: &[u8], expected_id: &str) -> ServerAcceptance {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ServerAcceptance::Unknown;
    };
    if value.get("accepted").and_then(Value::as_bool) != Some(true) {
        return ServerAcceptance::Unknown;
    }
    // Existing servers return an issue number but no new-protocol receipt.
    // Do not invent a receipt or replace the last locally verified success.
    if value.get("reportId").is_none() {
        if let Some(number) = value
            .get("issueNumber")
            .and_then(Value::as_u64)
            .filter(|number| (1..=9_007_199_254_740_991).contains(number))
        {
            return ServerAcceptance::LegacyAccepted { number };
        }
    }
    if value.get("reportId").and_then(Value::as_str) != Some(expected_id) {
        return ServerAcceptance::Unknown;
    }
    let Some(uploaded_at) = value.get("uploadedAt").and_then(Value::as_str) else {
        return ServerAcceptance::Unknown;
    };
    if chrono::DateTime::parse_from_rfc3339(uploaded_at).is_err() {
        return ServerAcceptance::Unknown;
    }
    ServerAcceptance::Accepted {
        uploaded_at: uploaded_at.to_owned(),
    }
}
fn legacy_feedback_source_id(install_id: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(install_id)
        .map(|id| id.simple().to_string())
        .map_err(|_| "本机反馈安装身份无效，未提交反馈".to_string())
}

fn receipt_store() -> FeedbackReceiptStore {
    FeedbackReceiptStore::new(crate::config::get_app_config_dir())
}
#[tauri::command]
pub async fn feedback_get_last_report() -> Result<Option<FeedbackReceipt>, String> {
    tauri::async_runtime::spawn_blocking(|| receipt_store().read_last())
        .await
        .map_err(|_| "读取本机反馈记录失败".to_string())?
        .map_err(|_| "读取本机反馈记录失败".to_string())
}
#[tauri::command]
pub async fn feedback_delete_local_report(expected_report_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || receipt_store().delete_local(&expected_report_id))
        .await
        .map_err(|_| "删除本机反馈记录失败".to_string())?
        .map_err(|_| "本机反馈记录已变化或无法删除，请刷新后重试".to_string())
}
#[tauri::command]
pub async fn feedback_export_last_diagnostics(
    expected_report_id: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let Some(target) = app
        .dialog()
        .file()
        .add_filter("ZIP", &["zip"])
        .set_file_name("diagnostics.zip")
        .blocking_save_file()
        .map(|p| p.to_string())
    else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || {
        // Recheck after the dialog: a concurrently replaced/deleted receipt
        // must not export a different report under the user's old selection.
        let bytes = receipt_store()
            .export_zip(&expected_report_id)
            .map_err(|_| "本机诊断包已变化或不可用，请刷新后重试".to_string())?;
        crate::config_file_io::stage_write(
            std::path::Path::new(&target),
            &bytes,
            Some(0o600),
            true,
        )
        .and_then(|staged| staged.commit())
        .map_err(|_| "导出本机诊断包失败".to_string())?;
        Ok(true)
    })
    .await
    .map_err(|_| "导出本机诊断包失败".to_string())?
}

/// 提交问题反馈：描述 + 截图（独立部件）+ 可选诊断包，multipart 上传。
#[tauri::command]
pub async fn prepare_feedback(
    description: String,
    include_diagnostics: bool,
    include_sites: bool,
    screenshots: Vec<FeedbackScreenshotInput>,
    related_apps: Option<Vec<String>>,
    contact: Option<String>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let admission = UploadAdmission::acquire()?;
    let db = state.db.clone();
    run_owned_preparation(admission, async move {
        let generation = preview_slot().begin().map_err(str::to_owned)?;
        let description = description.trim().to_string();
        if description.is_empty() {
            return Err("描述不能为空".to_string());
        }
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(format!("描述超过 {MAX_DESCRIPTION_CHARS} 字上限"));
        }

        let remote_config = crate::relay::remote_config::load_cached();
        let endpoint = remote_config
            .as_ref()
            .and_then(crate::relay::remote_config::feedback_endpoint)
            .ok_or_else(|| "反馈通道未开放".to_string())?;
        let details = validate_feedback_details(
            related_apps.unwrap_or_default(),
            contact,
            remote_config
                .as_ref()
                .is_some_and(crate::relay::remote_config::feedback_extended_fields_supported),
        )?;

        let attachments = decode_screenshots(&screenshots)?;
        let report_id = uuid::Uuid::new_v4().to_string();
        let install_id =
            tauri::async_runtime::spawn_blocking(crate::settings::ensure_stats_install_id)
                .await
                .map_err(|_| {
                    "无法读取反馈安装身份，请确认本机保险库已解锁；仍可导出本机诊断包".to_string()
                })?
                .map_err(|_| {
                    "无法读取反馈安装身份，请确认本机保险库已解锁；仍可导出本机诊断包".to_string()
                })?;

        // Keep the legacy 32-hex wire shape, derived from the same installation
        // UUID. Feedback no longer takes the database lock to obtain a daily ID.
        let source_id = legacy_feedback_source_id(&install_id)?;

        // Detailed disk/CLI diagnostics share the same explicit consent as the ZIP.
        let facts = super::diagnostics::gather_feedback_environment(
            &db,
            include_diagnostics,
            details.related_apps.clone(),
        )
        .await;

        // 分诊摘要与诊断包同一开关：内容派生自日志，走同一同意边界。空摘要（Null）
        // 不发送该字段——服务端把缺省视为老客户端。
        let (bundle, signals) = if include_diagnostics {
            let manifest = facts.manifest.clone();
            let origins = include_sites.then(|| facts.site_origins.clone());
            let (zip, signals) = tauri::async_runtime::spawn_blocking(move || {
                let signals = crate::diagnostics_export::build_triage_summary();
                collect_diagnostics(manifest, origins)
                    .and_then(build_diagnostics_zip)
                    .map(|zip| (zip, signals))
            })
            .await
            .map_err(|e| AppError::Message(format!("诊断包构建任务失败: {e}")))?
            .map_err(|e| e.to_string())?;
            (Some(zip), signals)
        } else {
            (None, Value::Null)
        };

        let prepared = assemble_feedback_snapshot(FeedbackUploadInputs {
            endpoint,
            install_id,
            source_id,
            report_id,
            description,
            details,
            attachments,
            manifest: facts.manifest,
            bundle,
            signals,
        })?;
        preview_slot()
            .publish(generation, prepared, Instant::now())
            .map_err(str::to_owned)
    })
    .await
}

// Keep admission with the complete collection task, including blocking children.
// Dropping the IPC waiter must not admit overlapping expensive collections.
async fn run_owned_preparation<F>(admission: UploadAdmission, prepare: F) -> Result<Value, String>
where
    F: std::future::Future<Output = Result<Value, String>> + Send + 'static,
{
    tokio::spawn(async move {
        let _admission = admission;
        prepare.await
    })
    .await
    .map_err(|_| "feedback_preview_unavailable".to_string())?
}

struct FeedbackUploadInputs {
    endpoint: String,
    install_id: String,
    source_id: String,
    report_id: String,
    description: String,
    details: FeedbackDetails,
    attachments: Vec<DecodedScreenshot>,
    manifest: Value,
    bundle: Option<Vec<u8>>,
    signals: Value,
}

/// The one projection of collected bytes into both the preview and HTTP body.
fn assemble_feedback_snapshot(
    input: FeedbackUploadInputs,
) -> Result<crate::feedback_preview::PreparedFeedback, String> {
    let FeedbackUploadInputs {
        endpoint,
        install_id,
        source_id,
        report_id,
        description,
        details,
        attachments,
        manifest,
        bundle,
        signals,
    } = input;
    let meta_text =
        serde_json::to_string(&manifest).map_err(|_| "环境摘要序列化失败".to_string())?;
    let signals_text = if signals.is_null() {
        None
    } else {
        Some(serde_json::to_string(&signals).map_err(|_| "分诊摘要序列化失败".to_string())?)
    };
    let apps_text = serde_json::to_string(&details.related_apps)
        .map_err(|_| "feedback_related_apps_invalid".to_string())?;
    let total_bytes = attachments.iter().map(|a| a.bytes.len()).sum::<usize>()
        + bundle.as_ref().map_or(0, |z| z.len())
        + meta_text.len()
        + install_id.len()
        + source_id.len()
        + report_id.len()
        + env!("CARGO_PKG_VERSION").len()
        + attachments.iter().map(|a| a.name.len()).sum::<usize>()
        + description.len()
        + details.contact.as_ref().map_or(0, String::len)
        + apps_text.len()
        + signals_text.as_ref().map_or(0, String::len);
    if total_bytes > MAX_TOTAL_UPLOAD_BYTES {
        return Err(format!(
            "反馈内容超过 {}MB 上限，请减少截图或去掉诊断信息",
            MAX_TOTAL_UPLOAD_BYTES / 1024 / 1024
        ));
    }

    let summary = FeedbackSafetySummary {
        description_included: true,
        contact_included: details.contact.is_some(),
        screenshot_count: attachments.len() as u32,
        screenshot_bytes: attachments.iter().map(|a| a.bytes.len() as u64).sum(),
        diagnostics_included: bundle.is_some(),
        diagnostics_bytes: bundle.as_ref().map_or(0, |b| b.len() as u64),
    };
    let mut files: Vec<Value> = attachments.iter().map(|a| serde_json::json!({
        "name":a.name, "kind":"screenshot", "bytes":a.bytes.len(), "sha256":hex::encode(Sha256::digest(&a.bytes)),
    })).collect();
    if let Some(bytes) = &bundle {
        files.push(serde_json::json!({
        "name":"diagnostics.zip", "kind":"diagnostics", "bytes":bytes.len(), "sha256":hex::encode(Sha256::digest(bytes)),
    }));
    }
    let requires_extended_fields = details.contact.is_some() || !details.related_apps.is_empty();
    let preview = serde_json::json!({
        "files":files, "summary":summary, "totalBytes":total_bytes,
        "applicationConfigs":manifest.pointer("/diagnostics/applications").cloned().unwrap_or(Value::Null),
    });
    let mut form = reqwest::multipart::Form::new()
        .text("installId", install_id)
        .text("reportId", report_id.clone())
        .text("sourceId", source_id.clone())
        .text("appVersion", env!("CARGO_PKG_VERSION").to_string())
        .text("description", description.clone())
        .text("meta", meta_text);
    if !details.related_apps.is_empty() {
        form = form.text("relatedApps", apps_text);
    }
    if let Some(contact) = details.contact {
        form = form.text("contact", contact);
    }
    if let Some(signals_text) = signals_text {
        form = form.text("signals", signals_text);
    }
    for attachment in &attachments {
        let part = reqwest::multipart::Part::bytes(attachment.bytes.clone())
            .file_name(attachment.name.clone())
            .mime_str(image_mime_for_name(&attachment.name))
            .map_err(|e| format!("截图 {} 的 mime 构造失败: {e}", attachment.name))?;
        form = form.part("screenshots", part);
    }
    if let Some(zip_bytes) = bundle.as_ref() {
        let part = reqwest::multipart::Part::bytes(zip_bytes.clone())
            .file_name("diagnostics.zip")
            .mime_str("application/zip")
            .map_err(|e| format!("诊断包 mime 构造失败: {e}"))?;
        form = form.part("bundle", part);
    }

    Ok(crate::feedback_preview::PreparedFeedback {
        endpoint,
        form,
        report_id,
        requires_extended_fields,
        summary,
        bundle,
        preview,
    })
}

fn preview_slot() -> &'static crate::feedback_preview::FeedbackPreviewSlot {
    static SLOT: OnceLock<crate::feedback_preview::FeedbackPreviewSlot> = OnceLock::new();
    SLOT.get_or_init(Default::default)
}

#[tauri::command]
pub fn feedback_cancel_preview(preview_id: String) -> Result<(), String> {
    preview_slot().cancel(&preview_id).map_err(str::to_owned)
}

/// Only the displayed immutable snapshot can be sent; never recollect here.
#[tauri::command]
pub async fn submit_feedback(preview_id: String) -> Result<Value, String> {
    let current = crate::relay::remote_config::load_cached();
    submit_preview_snapshot(
        preview_slot(),
        &preview_id,
        current
            .as_ref()
            .and_then(crate::relay::remote_config::feedback_endpoint),
        current
            .as_ref()
            .is_some_and(crate::relay::remote_config::feedback_extended_fields_supported),
        receipt_store(),
    )
    .await
}

async fn submit_preview_snapshot(
    slot: &crate::feedback_preview::FeedbackPreviewSlot,
    preview_id: &str,
    current_endpoint: Option<String>,
    extended_fields: bool,
    store: FeedbackReceiptStore,
) -> Result<Value, String> {
    let admission = UploadAdmission::acquire()?;
    let prepared = slot
        .take(preview_id, Instant::now())
        .map_err(str::to_owned)?;
    if current_endpoint.as_deref() != Some(prepared.endpoint.as_str())
        || (prepared.requires_extended_fields && !extended_fields)
    {
        return Err("feedback_preview_changed".to_string());
    }
    upload_prepared_feedback(
        &prepared.endpoint,
        prepared.form,
        prepared.report_id,
        prepared.summary,
        prepared.bundle,
        store,
        admission,
    )
    .await
}

// Admission belongs to the real blocking publisher until it can no longer
// replace the receipt, even when the invoking future is cancelled.
async fn publish_feedback_receipt(
    admission: UploadAdmission,
    publish: impl FnOnce() -> Result<FeedbackReceipt, crate::feedback_receipt::FeedbackReceiptError>
        + Send
        + 'static,
) -> Result<
    Result<FeedbackReceipt, crate::feedback_receipt::FeedbackReceiptError>,
    tokio::task::JoinError,
> {
    tokio::task::spawn_blocking(move || {
        let _admission = admission;
        publish()
    })
    .await
}

/// The real send/acceptance/local publication path. Its inputs are prepared by
/// the original command, and its store root never comes from an IPC argument.
async fn upload_prepared_feedback(
    endpoint: &str,
    form: reqwest::multipart::Form,
    report_id: String,
    summary: FeedbackSafetySummary,
    bundle: Option<Vec<u8>>,
    store: FeedbackReceiptStore,
    admission: UploadAdmission,
) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(UPLOAD_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        // 与 crowd 上传同一 UA 惯例（LoongPort/ 前缀）。
        .user_agent(concat!("LoongPort/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("构造反馈上传客户端失败: {e}"))?;

    let Ok(mut response) = client.post(endpoint).multipart(form).send().await else {
        return Ok(serde_json::json!({"status":"unknown"}));
    };
    match response.status() {
        status if status.is_success() => {}
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            return Err("服务暂时无法接收反馈，请稍后再试".to_string())
        }
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => {
            return Err("反馈内容过大，请减少附件".to_string())
        }
        status if status.is_client_error() => return Err("服务未接受此次反馈".to_string()),
        _ => return Ok(serde_json::json!({"status":"unknown"})),
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= 64 * 1024 => {
                body.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return Ok(serde_json::json!({"status":"unknown"})),
        }
    }
    let (identity, uploaded_at) = match parse_server_acceptance(&body, &report_id) {
        ServerAcceptance::Unknown => return Ok(serde_json::json!({"status":"unknown"})),
        ServerAcceptance::LegacyAccepted { number } => (
            FeedbackReceiptIdentity::GithubIssue { number },
            // The old protocol has no server timestamp. This is the local
            // observation of its explicit acceptance, not invented server data.
            chrono::Utc::now().to_rfc3339(),
        ),
        ServerAcceptance::Accepted { uploaded_at } => (
            FeedbackReceiptIdentity::Report { id: report_id },
            uploaded_at,
        ),
    };
    let accepted = AcceptedFeedbackReceipt {
        identity,
        uploaded_at,
        summary,
    };
    let unsaved = crate::feedback_receipt::project_unsaved(accepted.clone());
    let saved = publish_feedback_receipt(admission, move || {
        store.save_accepted(accepted, bundle.as_deref())
    })
    .await;
    match saved {
        Ok(Ok(receipt)) => {
            Ok(serde_json::json!({"status":"accepted","receipt":receipt,"localSave":"stored"}))
        }
        _ => Ok(serde_json::json!({"status":"accepted","receipt":unsaved,"localSave":"failed"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_identity_uses_same_uuid_without_replacing_invalid_legacy_values() {
        let id = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            legacy_feedback_source_id(id).unwrap(),
            "11111111222243338444555555555555"
        );
        for invalid in [
            "",
            "legacy-non-uuid",
            "private/path",
            "1111111122224333844455555555555",
        ] {
            assert!(legacy_feedback_source_id(invalid).is_err());
        }
    }

    #[test]
    fn acceptance_requires_actual_server_identity_and_matching_new_request() {
        assert_eq!(
            parse_server_acceptance(br#"{"accepted":true,"issueNumber":7}"#, "request"),
            ServerAcceptance::LegacyAccepted { number: 7 }
        );
        for body in [
            r#"{"success":true}"#,
            r#"{"accepted":true}"#,
            r#"{"accepted":true,"issueNumber":0}"#,
            r#"{"accepted":true,"issueNumber":9007199254740992}"#,
            r#"{"accepted":true,"issueNumber":-1}"#,
            r#"{"accepted":true,"reportId":"other","issueNumber":7,"uploadedAt":"2026-10-18T10:00:00Z"}"#,
            r#"{"accepted":true,"reportId":"request","uploadedAt":"private invalid text"}"#,
            r#"{"accepted":false,"issueNumber":7}"#,
        ] {
            assert_eq!(
                parse_server_acceptance(body.as_bytes(), "request"),
                ServerAcceptance::Unknown
            );
        }
        assert_eq!(
            parse_server_acceptance(
                br#"{"accepted":true,"reportId":"request","uploadedAt":"2026-10-18T10:00:00Z"}"#,
                "request"
            ),
            ServerAcceptance::Accepted {
                uploaded_at: "2026-10-18T10:00:00Z".into()
            }
        );
    }

    fn input(name: &str, base64: &str) -> FeedbackScreenshotInput {
        FeedbackScreenshotInput {
            name: name.to_string(),
            base64: base64.to_string(),
        }
    }

    #[test]
    fn decode_accepts_data_urls_and_raw_base64() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");
        let attachments = decode_screenshots(&[
            input(
                "clipboard-1.png",
                &format!("data:image/png;base64,{encoded}"),
            ),
            input("shot.png", &encoded),
        ])
        .expect("两种形态都该解出");
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].bytes, b"png-bytes".to_vec());
        assert_eq!(attachments[0].name, "clipboard-1.png");
    }

    #[test]
    fn decode_rejects_oversize_count_and_bad_base64() {
        assert!(decode_screenshots(&[]).unwrap().is_empty());

        let too_many: Vec<_> = (0..=MAX_SCREENSHOTS)
            .map(|i| input(&format!("s{i}.png"), "AAAA"))
            .collect();
        assert!(decode_screenshots(&too_many).is_err(), "超张数必须拒");

        let big = vec![0u8; MAX_SCREENSHOT_BYTES + 1];
        let encoded = base64::engine::general_purpose::STANDARD.encode(big);
        assert!(
            decode_screenshots(&[input("big.png", &encoded)]).is_err(),
            "超单张体积必须拒"
        );

        assert!(decode_screenshots(&[input("bad.png", "!!!not-base64!!!")]).is_err());
    }

    #[test]
    fn image_mime_follows_extension_with_octet_stream_fallback() {
        assert_eq!(image_mime_for_name("a.PNG"), "image/png");
        assert_eq!(image_mime_for_name("b.jpeg"), "image/jpeg");
        assert_eq!(image_mime_for_name("noext"), "application/octet-stream");
    }
    fn synthetic_zip() -> Vec<u8> {
        crate::diagnostics_export::build_diagnostics_zip(
            crate::diagnostics_export::CollectedDiagnostics {
                manifest: serde_json::json!({"appVersion":"synthetic-test"}),
                entries: vec![(
                    "logs/synthetic.log".into(),
                    b"synthetic diagnostic".to_vec(),
                )],
            },
        )
        .unwrap()
    }
    fn synthetic_summary(zip: &[u8]) -> FeedbackSafetySummary {
        FeedbackSafetySummary {
            description_included: true,
            contact_included: false,
            screenshot_count: 0,
            screenshot_bytes: 0,
            diagnostics_included: true,
            diagnostics_bytes: zip.len() as u64,
        }
    }
    fn single_http_reply(status: u16, body: &str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/feedback", listener.local_addr().unwrap());
        let body = body.to_owned();
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let header_end;
            loop {
                let mut bytes = [0_u8; 4096];
                let n = socket.read(&mut bytes).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&bytes[..n]);
                if let Some(pos) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    header_end = pos + 4;
                    break;
                }
                assert!(request.len() < 64 * 1024);
            }
            let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|s| s.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            assert!(length < MAX_TOTAL_UPLOAD_BYTES);
            while request.len() < header_end + length {
                let mut bytes = [0_u8; 4096];
                let n = socket.read(&mut bytes).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&bytes[..n]);
            }
            write!(socket, "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            request
        });
        (endpoint, task)
    }
    fn synthetic_form(zip: &[u8]) -> reqwest::multipart::Form {
        reqwest::multipart::Form::new()
            .text("description", "synthetic only")
            .part(
                "bundle",
                reqwest::multipart::Part::bytes(zip.to_vec()).file_name("diagnostics.zip"),
            )
    }
    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn real_http_acceptance_saves_the_exact_uploaded_zip_for_both_receipt_protocols() {
        for (reply, cas) in [
            (
                r#"{"accepted":true,"reportId":"synthetic-report","uploadedAt":"2026-10-18T10:00:00Z"}"#,
                "synthetic-report",
            ),
            (r#"{"accepted":true,"issueNumber":42}"#, "github_issue:42"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let zip = synthetic_zip();
            let (endpoint, task) = single_http_reply(200, reply);
            let result = upload_prepared_feedback(
                &endpoint,
                synthetic_form(&zip),
                "synthetic-report".into(),
                synthetic_summary(&zip),
                Some(zip.clone()),
                FeedbackReceiptStore::new(temp.path()),
                UploadAdmission::acquire().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(result["status"], "accepted");
            assert_eq!(result["localSave"], "stored");
            assert_eq!(result["receipt"]["reportId"], cas);
            assert_eq!(
                FeedbackReceiptStore::new(temp.path())
                    .export_zip(cas)
                    .unwrap(),
                zip
            );
            let wire = task.join().unwrap();
            assert!(wire.windows(zip.len()).any(|part| part == zip));
        }
    }
    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn unverified_http_outcome_never_replaces_last_success_or_replays() {
        for (status, reply) in [
            (200, r#"{"success":true}"#),
            (
                200,
                r#"{"accepted":true,"reportId":"other","uploadedAt":"2026-10-18T10:00:00Z"}"#,
            ),
            (500, "unknown"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let zip = synthetic_zip();
            let store = FeedbackReceiptStore::new(temp.path());
            store
                .save_accepted(
                    AcceptedFeedbackReceipt {
                        identity: FeedbackReceiptIdentity::GithubIssue { number: 7 },
                        uploaded_at: "2026-10-18T09:00:00Z".into(),
                        summary: synthetic_summary(&zip),
                    },
                    Some(&zip),
                )
                .unwrap();
            let record = temp.path().join("feedback/last-success.json");
            let before = std::fs::read(&record).unwrap();
            let (endpoint, task) = single_http_reply(status, reply);
            let result = upload_prepared_feedback(
                &endpoint,
                synthetic_form(&zip),
                "synthetic-report".into(),
                synthetic_summary(&zip),
                Some(zip),
                store,
                UploadAdmission::acquire().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(result["status"], "unknown");
            assert_eq!(std::fs::read(record).unwrap(), before);
            task.join().unwrap();
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn accepted_http_with_local_publication_failure_is_not_an_upload_failure() {
        let temp = tempfile::tempdir().unwrap();
        let zip = synthetic_zip();
        let store = FeedbackReceiptStore::new(temp.path());
        store
            .save_accepted(
                AcceptedFeedbackReceipt {
                    identity: FeedbackReceiptIdentity::GithubIssue { number: 7 },
                    uploaded_at: "2026-10-18T09:00:00Z".into(),
                    summary: synthetic_summary(&zip),
                },
                Some(&zip),
            )
            .unwrap();
        let record = temp.path().join("feedback/last-success.json");
        let before = std::fs::read(&record).unwrap();
        // An actual unsafe leaf makes the original owner refuse publication.
        std::fs::hard_link(&record, temp.path().join("synthetic-link")).unwrap();
        let (endpoint, task) = single_http_reply(200, r#"{"accepted":true,"issueNumber":8}"#);
        let result = upload_prepared_feedback(
            &endpoint,
            synthetic_form(&zip),
            "synthetic-report".into(),
            synthetic_summary(&zip),
            Some(zip),
            store,
            UploadAdmission::acquire().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "accepted");
        assert_eq!(result["localSave"], "failed");
        assert_eq!(
            result["receipt"]["identity"],
            serde_json::json!({"kind":"github_issue","number":8})
        );
        assert_eq!(result["receipt"]["localCopy"]["status"], "missing");
        assert_eq!(std::fs::read(record).unwrap(), before);
        task.join().unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial(feedback_upload)]
    async fn cancelled_invoke_keeps_admission_until_real_blocking_publication_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(temp.path());
        let zip = synthetic_zip();
        let accepted = AcceptedFeedbackReceipt {
            identity: FeedbackReceiptIdentity::GithubIssue { number: 42 },
            uploaded_at: "2026-10-18T10:00:00Z".into(),
            summary: synthetic_summary(&zip),
        };
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let admission = UploadAdmission::acquire().unwrap();
        let invoke = tokio::spawn(publish_feedback_receipt(admission, move || {
            let _ = entered_tx.send(());
            release_rx.recv().unwrap();
            let result = store.save_accepted(accepted, Some(&zip));
            let _ = done_tx.send(());
            result
        }));
        entered_rx.await.unwrap();
        invoke.abort();
        assert!(invoke.await.unwrap_err().is_cancelled());
        let still_denied = UploadAdmission::acquire().is_err();
        // Always release the real publisher before asserting, including RED.
        release_tx.send(()).unwrap();
        done_rx.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while UPLOAD_IN_FLIGHT.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            still_denied,
            "cancelled caller released admission before publication finished"
        );
        assert!(UploadAdmission::acquire().is_ok());
        assert_eq!(
            FeedbackReceiptStore::new(temp.path())
                .read_last()
                .unwrap()
                .unwrap()
                .report_id,
            "github_issue:42"
        );
    }
    #[test]
    fn extended_fields_are_fail_closed_and_base_feedback_remains_available() {
        assert!(validate_feedback_details(vec![], None, false).is_ok());
        assert_eq!(
            validate_feedback_details(vec![], Some("synthetic@example.invalid".into()), false)
                .err()
                .unwrap(),
            "feedback_contact_unsupported"
        );
        assert_eq!(
            validate_feedback_details(vec!["codex".into()], None, false)
                .err()
                .unwrap(),
            "feedback_extended_fields_unsupported"
        );
    }
    #[test]
    fn optional_contact_is_bounded_without_silent_control_stripping() {
        assert!(validate_feedback_details(vec![], Some(" ".into()), false)
            .unwrap()
            .contact
            .is_none());
        for text in ["\nsynthetic", "synthetic\t", "synthetic\0"] {
            assert_eq!(
                validate_feedback_details(vec![], Some(text.into()), true)
                    .err()
                    .unwrap(),
                "feedback_contact_invalid"
            );
        }
        assert!(validate_feedback_details(vec![], Some("字".repeat(200)), true).is_ok());
        assert!(validate_feedback_details(vec![], Some("字".repeat(201)), true).is_err());
        assert_eq!(
            validate_feedback_details(vec![], Some("  synthetic@example.invalid  ".into()), true)
                .unwrap()
                .contact
                .as_deref(),
            Some("synthetic@example.invalid")
        );
    }
    #[test]
    fn related_apps_use_registered_ids_and_stable_unique_order() {
        let details = validate_feedback_details(
            vec!["other".into(), "pi".into(), "codex".into(), "pi".into()],
            None,
            true,
        )
        .unwrap();
        assert_eq!(details.related_apps, ["codex", "pi", "other"]);
        for id in ["CODEX", "../secret", "custom-token"] {
            assert!(validate_feedback_details(vec![id.into()], None, true).is_err());
        }
        for app in crate::app_config::AppType::all() {
            assert!(validate_feedback_details(vec![app.as_str().into()], None, true).is_ok());
        }
        assert!(validate_feedback_details(vec!["zcode".into()], None, true).is_ok());
    }

    #[test]
    fn raw_contact_size_is_bounded_before_trimming_or_unicode_expansion() {
        assert!(validate_feedback_details(vec![], Some(" ".repeat(1025)), true).is_err());
        assert!(
            validate_feedback_details(vec![], Some(" ".repeat(1024)), true)
                .unwrap()
                .contact
                .is_none()
        );
        assert!(
            validate_feedback_details(vec![], Some(format!("{}a", " ".repeat(1023))), true).is_ok()
        );
        assert!(validate_feedback_details(vec![], Some("😀".repeat(513)), true).is_err());
    }
    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn cancelled_prepare_waiter_keeps_admission_until_blocking_collection_finishes() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let admission = UploadAdmission::acquire().unwrap();
        let caller = tokio::spawn(run_owned_preparation(admission, async move {
            tokio::task::spawn_blocking(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .await
            .unwrap();
            Ok(Value::Null)
        }));
        entered_rx.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(UploadAdmission::acquire().is_err());
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = UploadAdmission::acquire() {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn frozen_preview_send_and_local_copy_use_same_bytes_and_token_is_one_shot() {
        use crate::feedback_preview::FeedbackPreviewSlot;
        let slot = FeedbackPreviewSlot::default();
        let applications = serde_json::json!({"codex":{"diskConfig":{"status":"ok","modelReasoningEffort":"high"}}});
        let manifest = serde_json::json!({"appVersion":"synthetic-test","diagnostics":{"applications":applications}});
        let zip = crate::diagnostics_export::build_diagnostics_zip(
            crate::diagnostics_export::CollectedDiagnostics {
                manifest: manifest.clone(),
                entries: vec![],
            },
        )
        .unwrap();
        let id = "preview-report";
        let (endpoint, server) = single_http_reply(
            201,
            &format!(
                r#"{{"accepted":true,"reportId":"{id}","uploadedAt":"2026-10-18T11:00:00Z"}}"#
            ),
        );
        let prepared = assemble_feedback_snapshot(FeedbackUploadInputs {
            endpoint: endpoint.clone(),
            install_id: "00000000-0000-4000-8000-000000000001".into(),
            source_id: "00000000000040008000000000000001".into(),
            report_id: id.into(),
            description: "synthetic issue".into(),
            details: validate_feedback_details(
                vec!["codex".into()],
                Some("synthetic@example.invalid".into()),
                true,
            )
            .unwrap(),
            attachments: vec![],
            manifest,
            bundle: Some(zip.clone()),
            signals: Value::Null,
        })
        .unwrap();
        let generation = slot.begin().unwrap();
        let shown = slot.publish(generation, prepared, Instant::now()).unwrap();
        assert_eq!(shown["applicationConfigs"], applications);
        let token = shown["previewId"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let result = submit_preview_snapshot(
            &slot,
            token,
            Some(endpoint.clone()),
            true,
            FeedbackReceiptStore::new(dir.path()),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "accepted");
        let wire = server.join().unwrap();
        assert!(wire.windows(zip.len()).any(|part| part == zip));
        let saved = store.export_zip(id).unwrap();
        assert_eq!(saved, zip);
        assert_eq!(
            shown["files"][0]["sha256"],
            hex::encode(Sha256::digest(&saved))
        );
        assert_eq!(
            submit_preview_snapshot(&slot, token, Some(endpoint), true, store)
                .await
                .unwrap_err(),
            "feedback_preview_changed"
        );
    }
    #[tokio::test]
    #[serial_test::serial(feedback_upload)]
    async fn changed_endpoint_or_capability_consumes_preview_without_http_or_retry() {
        use crate::feedback_preview::{FeedbackPreviewSlot, PreparedFeedback};
        for change_endpoint in [true, false] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}/feedback", listener.local_addr().unwrap());
            let slot = FeedbackPreviewSlot::default();
            let zip = synthetic_zip();
            let generation = slot.begin().unwrap();
            let preview = slot
                .publish(
                    generation,
                    PreparedFeedback {
                        endpoint: endpoint.clone(),
                        form: reqwest::multipart::Form::new(),
                        report_id: "not-sent".into(),
                        requires_extended_fields: !change_endpoint,
                        summary: synthetic_summary(&zip),
                        bundle: Some(zip),
                        preview: serde_json::json!({}),
                    },
                    Instant::now(),
                )
                .unwrap();
            let token = preview["previewId"].as_str().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let store = FeedbackReceiptStore::new(dir.path());
            assert_eq!(
                submit_preview_snapshot(
                    &slot,
                    token,
                    if change_endpoint {
                        None
                    } else {
                        Some(endpoint.clone())
                    },
                    false,
                    FeedbackReceiptStore::new(dir.path())
                )
                .await
                .unwrap_err(),
                "feedback_preview_changed"
            );
            assert!(listener.accept().is_err());
            assert!(store.read_last().unwrap().is_none());
            assert_eq!(
                submit_preview_snapshot(&slot, token, Some(endpoint), true, store)
                    .await
                    .unwrap_err(),
                "feedback_preview_changed"
            );
            assert!(listener.accept().is_err());
        }
    }
    #[test]
    fn prepared_payload_counts_metadata_and_user_text_in_total_limit() {
        let result = assemble_feedback_snapshot(FeedbackUploadInputs {
            endpoint: "https://example.invalid/feedback".into(),
            install_id: "synthetic-install".into(),
            source_id: "synthetic-source".into(),
            report_id: "synthetic-report".into(),
            description: "synthetic".into(),
            details: validate_feedback_details(vec![], None, false).unwrap(),
            attachments: vec![],
            manifest: serde_json::json!({"synthetic": "x".repeat(MAX_TOTAL_UPLOAD_BYTES)}),
            bundle: None,
            signals: Value::Null,
        });
        assert!(result.is_err());
    }
}
