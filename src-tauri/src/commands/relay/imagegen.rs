//! 生图（codex-image）命令：MCP 注册同步与 App 内生成视图入口。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::imagegen;

// ============================================================================
// 生图（MCP 注册 + App 内直接生图）
// ============================================================================

// MCP 注册同步的触发点全部在数据层（2026-09-07 收口）：启动（lib.rs 无条件对齐
// 一次）、provision 收尾（mark_pricing_after_success）、设置写入
// （relay_set_imagegen_mcp_enabled）与删站点/账号。曾经还有「进入生图页时同步」
// 的前端入口 —— 那是视图读路径驱动数据层行为，已删（命令与 App.tsx effect 一并）。

/// 一张生成图片（App 内直接生图的结果条目）。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImagegenImageRef {
    pub path: String,
    pub mime: String,
}

/// `relay_imagegen_generate` 的返回：这次用了哪个档位/模型、图片落在哪。
///
/// 模型名给前端展示用（「已生成（gpt-image-2）」），不是让前端替用户做任何决定。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImagegenGenerateResult {
    pub tier_name: String,
    pub model: String,
    pub images: Vec<ImagegenImageRef>,
}

/// App 内直接生图（生图页「生成」视图）：不经过任何 CLI 会话。
///
/// 与 MCP 工具（`--mcp-image-gen`）共用 [`imagegen`] 这一个核心 —— 档位选择、
/// 请求形状、落盘与修剪完全一致，差别只在结果不进宿主对话。
///
/// `n` = 张数（批量）。范围判据的唯一源在核心层（`imagegen::validate_count`，
/// 上限与计费后果的说明见那里），本层只做缺省补 1。
#[tauri::command]
pub async fn relay_imagegen_generate(
    app_handle: tauri::AppHandle,
    prompt: String,
    size: Option<String>,
    n: Option<u32>,
) -> Result<ImagegenGenerateResult, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt 不能为空".into());
    }
    let count = imagegen::validate_count(n.unwrap_or(1))?;
    let tier = imagegen::load_current_tier()?;
    let images = imagegen::generate_image(
        &tier,
        prompt,
        size.as_deref(),
        count,
        imagegen::request_timeout(count),
    )
    .await?;
    imagegen::ensure_asset_scope(&app_handle);
    Ok(ImagegenGenerateResult {
        tier_name: tier.display_name,
        model: tier.model,
        images: images
            .into_iter()
            .map(|img| ImagegenImageRef {
                path: img.path.to_string_lossy().to_string(),
                mime: img.mime.to_string(),
            })
            .collect(),
    })
}

/// 出图目录的画廊清单（MCP 与直接生图的产物汇在同一目录），mtime 从新到旧。
#[tauri::command]
pub fn relay_imagegen_list_images(
    app_handle: tauri::AppHandle,
) -> Result<Vec<imagegen::GalleryImage>, String> {
    imagegen::ensure_asset_scope(&app_handle);
    Ok(imagegen::gallery_images())
}

/// 在文件管理器里显示一张生成的图。
///
/// 只接受出图目录内的路径 —— 这是个「打开本地文件」的命令，不该被拿去探测任意路径。
#[tauri::command]
pub async fn relay_imagegen_reveal_image(
    app_handle: tauri::AppHandle,
    path: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;

    let target = std::path::PathBuf::from(&path);
    // canonicalize 把 `..`、软链等折叠成真实路径，再做出图目录的包含校验。
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("找不到这张图：{e}"))?;
    let dir = imagegen::output_dir()
        .canonicalize()
        .map_err(|e| format!("出图目录不存在：{e}"))?;
    if !canonical.starts_with(&dir) {
        return Err("只能查看生成图片目录里的文件".into());
    }
    app_handle
        .opener()
        .reveal_item_in_dir(&canonical)
        .map_err(|e| format!("打开文件夹失败：{e}"))
}

/// 切换「在 CLI 对话中提供生图工具（MCP）」并立刻对齐注册。
#[tauri::command]
pub fn relay_set_imagegen_mcp_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), AppError> {
    crate::settings::set_imagegen_mcp_enabled(enabled)?;
    imagegen_mcp::sync_registration(&state)
}
