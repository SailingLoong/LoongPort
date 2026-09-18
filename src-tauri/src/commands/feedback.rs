//! 问题反馈回传命令层：multipart 上传到签名配置下发的端点。
//!
//! 载荷形状（与服务端 `crowd-metrics/src/feedback.ts` 同一契约）：
//! 文本字段 `sourceId` / `appVersion` / `description` / `meta`（环境事实 JSON）+
//! 图片部件 `screenshots`（≤6 张）+ 可选 `bundle`（诊断包 zip）。截图走独立部件
//! 而不是打进 zip —— 服务端要把它们单独存 R2 引进 issue 正文内联渲染。
//!
//! 端点唯源 [`crate::relay::remote_config::feedback_endpoint`]（v2 config 的
//! `feedback_url`，缺省 = 功能整体休眠，前端按钮不显示）。

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use tauri::State;

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

/// 提交问题反馈：描述 + 截图（独立部件）+ 可选诊断包，multipart 上传。
#[tauri::command]
pub async fn submit_feedback(
    description: String,
    include_diagnostics: bool,
    include_sites: bool,
    screenshots: Vec<FeedbackScreenshotInput>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let description = description.trim().to_string();
    if description.is_empty() {
        return Err("描述不能为空".to_string());
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!("描述超过 {MAX_DESCRIPTION_CHARS} 字上限"));
    }

    let endpoint = crate::relay::remote_config::load_cached()
        .as_ref()
        .and_then(crate::relay::remote_config::feedback_endpoint)
        .ok_or_else(|| "反馈通道未开放".to_string())?;

    let attachments = decode_screenshots(&screenshots)?;

    // sourceId 复用 crowd 的每日轮换身份：同日多份反馈可对号，隔日自然换新。
    let db = state.db.clone();
    let now = chrono::Utc::now().timestamp();
    let db_for_id = std::sync::Arc::clone(&db);
    let source_id = tauri::async_runtime::spawn_blocking(move || {
        crate::crowd::uploader::ensure_daily_source_id(&db_for_id, now)
    })
    .await
    .map_err(|e| format!("反馈身份读取任务失败: {e}"))?
    .map_err(|e| e.to_string())?;

    // 环境事实永远随行（issue 正文要展示）；诊断包（日志等）按勾选附带。
    let facts = super::diagnostics::gather_environment(&db).await;
    let meta_text =
        serde_json::to_string(&facts.manifest).map_err(|e| format!("环境摘要序列化失败: {e}"))?;

    let bundle = if include_diagnostics {
        let manifest = facts.manifest.clone();
        let origins = include_sites.then(|| facts.site_origins.clone());
        Some(
            tauri::async_runtime::spawn_blocking(move || {
                collect_diagnostics(manifest, origins).and_then(build_diagnostics_zip)
            })
            .await
            .map_err(|e| AppError::Message(format!("诊断包构建任务失败: {e}")))?
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    let total_bytes = attachments.iter().map(|a| a.bytes.len()).sum::<usize>()
        + bundle.as_ref().map_or(0, |z| z.len());
    if total_bytes > MAX_TOTAL_UPLOAD_BYTES {
        return Err(format!(
            "反馈内容超过 {}MB 上限，请减少截图或去掉诊断信息",
            MAX_TOTAL_UPLOAD_BYTES / 1024 / 1024
        ));
    }

    let mut form = reqwest::multipart::Form::new()
        .text("sourceId", source_id.clone())
        .text("appVersion", env!("CARGO_PKG_VERSION").to_string())
        .text("description", description.clone())
        .text("meta", meta_text);
    for attachment in &attachments {
        let part = reqwest::multipart::Part::bytes(attachment.bytes.clone())
            .file_name(attachment.name.clone())
            .mime_str(image_mime_for_name(&attachment.name))
            .map_err(|e| format!("截图 {} 的 mime 构造失败: {e}", attachment.name))?;
        form = form.part("screenshots", part);
    }
    if let Some(zip_bytes) = bundle {
        let part = reqwest::multipart::Part::bytes(zip_bytes)
            .file_name("diagnostics.zip")
            .mime_str("application/zip")
            .map_err(|e| format!("诊断包 mime 构造失败: {e}"))?;
        form = form.part("bundle", part);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(UPLOAD_TIMEOUT_SECS))
        // 与 crowd 上传同一 UA 惯例（LoongPort/ 前缀）。
        .user_agent(concat!("LoongPort/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("构造反馈上传客户端失败: {e}"))?;

    let response = client
        .post(&endpoint)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("反馈提交失败: {e}"))?;

    match response.status() {
        status if status.is_success() => {
            log::info!("问题反馈已提交（sourceId={source_id}）");
            Ok(serde_json::json!({ "success": true }))
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => Err("提交太频繁，请明天再试".to_string()),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => {
            Err("反馈内容过大，请减少截图或去掉诊断信息".to_string())
        }
        status => Err(format!("反馈提交失败: {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
