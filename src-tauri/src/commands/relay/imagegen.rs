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

/// `relay_imagegen_generate` 的返回：这次用了哪个档位/模型、图片落在哪、几张没成。
///
/// 模型名给前端展示用（「已生成（gpt-image-2）」），不是让前端替用户做任何决定；
/// `failed` 是部分失败的张数（批量并发下成功的那部分已落盘，用户拿得到）。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImagegenGenerateResult {
    pub tier_name: String,
    pub model: String,
    pub images: Vec<ImagegenImageRef>,
    pub failed: usize,
}

/// App 内直接生图（生图页「生成」视图）：不经过任何 CLI 会话。
///
/// 与 MCP 工具（`--mcp-image-gen`）共用 [`imagegen`] 这一个核心 —— 档位选择、
/// 请求形状、落盘与修剪完全一致，差别只在结果不进宿主对话。
///
/// `n` = 张数（批量，1-50 自由输入），范围判据的唯一源在核心层
/// （`imagegen::validate_count`）；`parallel` = 并发提交开关（生成视图的勾选框，
/// 缺省开）——两种模式的形状与取舍见 `imagegen::split_batch` 的表，本层只做缺省补全。
#[tauri::command]
pub async fn relay_imagegen_generate(
    app_handle: tauri::AppHandle,
    prompt: String,
    size: Option<String>,
    n: Option<u32>,
    parallel: Option<bool>,
) -> Result<ImagegenGenerateResult, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt 不能为空".into());
    }
    let count = imagegen::validate_count(n.unwrap_or(1))?;
    let tier = imagegen::load_current_tier()?;
    let (images, failed) = imagegen::generate_batch(
        &tier,
        prompt,
        size.as_deref(),
        count,
        parallel.unwrap_or(true),
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
        failed,
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

/// 当前生图存储目录（展示用，`PathBuf::to_string_lossy` 已是平台原生分隔符）。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImagegenOutputDir {
    pub path: String,
}

/// `relay_imagegen_set_output_dir` 的返回：切换到了哪、搬迁模式下搬过去几张。
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImagegenOutputDirSwitchResult {
    pub path: String,
    pub moved: usize,
}

/// 读当前生图存储目录。
#[tauri::command]
pub fn relay_imagegen_get_output_dir() -> Result<ImagegenOutputDir, String> {
    Ok(ImagegenOutputDir {
        path: imagegen::output_dir().to_string_lossy().to_string(),
    })
}

/// 更改生图存储目录（绝对路径；`migrate` = 把旧目录里我们生成的图搬过去）。
///
/// 顺序是**先搬迁、验证都过了才写设置** —— 迁移失败时设置不变，目录还在原地，
/// 用户重试幂等（同名跳过）。写入即生效：两条入口每次都现读 `imagegen::output_dir()`，
/// codex 里的 MCP 不必重启。磁盘根目录与用户主目录拒绝（画廊会退化成无意义扫描）。
#[tauri::command]
pub fn relay_imagegen_set_output_dir(
    app_handle: tauri::AppHandle,
    path: String,
    migrate: bool,
) -> Result<ImagegenOutputDirSwitchResult, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("存储位置不能为空".into());
    }
    let target = std::path::PathBuf::from(trimmed);
    if !target.is_absolute() {
        return Err("存储位置必须是绝对路径".into());
    }
    // canonicalize：折叠 `..` 与软链成真实路径，顺带确认目录存在
    // （目录选择对话框给的都存在，这里兜住别的入口）。
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("目录不存在：{e}"))?;
    if canonical.parent().is_none() {
        return Err("不能把磁盘根目录作为存储位置".into());
    }
    if canonical == crate::config::get_home_dir() {
        return Err("不能把整个用户主目录作为存储位置".into());
    }
    let display = canonical.to_string_lossy().to_string();
    let old = imagegen::output_dir();
    if canonical == old {
        return Ok(ImagegenOutputDirSwitchResult {
            path: display,
            moved: 0,
        });
    }
    std::fs::create_dir_all(&canonical).map_err(|e| format!("创建目录失败: {e}"))?;
    let moved = if migrate {
        imagegen::migrate_images(&old, &canonical)?
    } else {
        0
    };
    crate::settings::set_imagegen_output_dir(display.clone())
        .map_err(|e| format!("保存设置失败: {e}"))?;
    imagegen::ensure_asset_scope(&app_handle);
    Ok(ImagegenOutputDirSwitchResult {
        path: display,
        moved,
    })
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
