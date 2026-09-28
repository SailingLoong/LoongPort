//! 诊断包构建器：环境事实 + 脱敏后的日志/crash（+ 可选站点域名清单）打成一个 zip。
//!
//! 两个消费者共用同一段组装逻辑：
//! - 本地导出（`commands::diagnostics::export_diagnostics`，用户在群里协作调试用）；
//! - 反馈回传（`commands::feedback::submit_feedback`，包内附带同款诊断段）。
//!
//! 脱敏唯源 [`crate::diagnostics::redact_file_for_export`]；日志/崩溃件的枚举唯源
//! [`crate::panic_hook::diagnostic_entries`]（它与「哪些路径算诊断数据」的其它判定
//! 共用同一张清单，避免这里私自多收/漏收文件）。

use std::io::Write;
use std::path::Path;

use serde_json::Value;

use crate::error::AppError;

/// 单个日志/崩溃文件的防御性体积闸。log 轮转上限 20 MiB，正常件不会触碰这条；
/// 超过的多半不是日志（被换过的文件），跳过并在 manifest 记名。
const MAX_EXPORT_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// 收集结果：manifest 正文 + 已脱敏的 zip 条目（zip 内路径 → 字节）。
pub(crate) struct CollectedDiagnostics {
    pub manifest: Value,
    pub entries: Vec<(String, Vec<u8>)>,
}

/// 当前活跃日志的文件名。owner 是 lib.rs 日志初始化的 `file_name: Some("loongport")`
/// （插件补 `.log` 后缀）；轮转出的历史件带时间戳，不在此列。
const CURRENT_LOG_FILE: &str = "loongport.log";
/// 分诊摘要读取的日志尾部字节：20 MiB 轮转闸下取尾段足够覆盖最近数周，避免整读。
const TRIAGE_LOG_TAIL_BYTES: u64 = 256 * 1024;
/// 进摘要的最近 ERROR 行数上限与单行字符上限（issue 正文要留得住，见 worker 端闸）。
const TRIAGE_MAX_RECENT_ERRORS: usize = 8;
const TRIAGE_LINE_CHAR_CAP: usize = 240;

/// 反馈分诊摘要：让维护者**读 issue 本身**就能分诊，不必先下载诊断包解压翻日志。
///
/// 从与诊断包同源的两类件里提取信号（哪些件算诊断数据唯源
/// [`crate::panic_hook::diagnostic_entries`]）：
/// - `crash.log` 最后一次崩溃的时间/当时版本/消息——crash.log 只按体积轮转、
///   不按时间清理，旧版本的历史崩溃会一直躺在包里（曾误导排查），摘要里带上
///   时间与版本让维护者一眼判断新旧；
/// - 当前日志尾部的 ERROR 行，按归一化消息去重（时间戳/数字不计入区分），
///   最多 [`TRIAGE_MAX_RECENT_ERRORS`] 条，按发生顺序排列。
///
/// 输出经 [`crate::diagnostics::redact_file_for_export`] 同一条脱敏链，随反馈
/// multipart 的 `signals` 字段进 issue 正文；**仅在用户勾选附带诊断信息时随行**
/// ——内容派生自日志，与诊断包同一同意边界。没有信号时返回 `Value::Null`
/// （调用方不发送该字段）。
pub(crate) fn build_triage_summary() -> Value {
    let root = crate::panic_hook::diagnostic_root_path();

    let mut summary = serde_json::Map::new();
    let crash_text = read_text_lossy(&root.join(crate::panic_hook::CRASH_LOG_FILE));
    if let Some(crash) = summarize_last_crash(&crash_text) {
        summary.insert("lastCrash".to_string(), crash);
    }
    let log_tail = read_text_tail_lossy(
        &root
            .join(crate::panic_hook::LOG_DIRECTORY)
            .join(CURRENT_LOG_FILE),
        TRIAGE_LOG_TAIL_BYTES,
    );
    if let Some(errors) = summarize_recent_errors(&log_tail) {
        summary.insert("recentErrors".to_string(), errors);
    }

    if summary.is_empty() {
        Value::Null
    } else {
        Value::Object(summary)
    }
}

fn read_text_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default()
}

/// 只读文件尾部（日志可能有 20 MiB，分诊不需要全量）。从截断点起的第一行
/// 多半被切半，丢弃。
fn read_text_tail_lossy(path: &Path, tail_bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(tail_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut raw = Vec::new();
    if file.read_to_end(&mut raw).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    if start == 0 {
        text
    } else {
        match text.find('\n') {
            // 截断点后第一行切半，从下一行起才是完整的
            Some(next_line) => text[next_line + 1..].to_string(),
            None => String::new(),
        }
    }
}

/// 解析 crash.log 的最后一次崩溃（纯函数，供单测钉死）。
/// 块格式见 panic_hook：`[CRASH REPORT] <时间>` 头 + `App Version:` / `Message:` 行。
fn summarize_last_crash(text: &str) -> Option<Value> {
    let (_, last_block) = text.rsplit_once("[CRASH REPORT] ")?;
    let at = last_block.lines().next()?.trim();
    if at.is_empty() {
        return None;
    }
    let mut app_version = None;
    let mut message = None;
    for line in last_block.lines() {
        if let Some(v) = line.trim().strip_prefix("App Version: ") {
            app_version = Some(v.trim().to_string());
        } else if let Some(m) = line.trim().strip_prefix("Message: ") {
            message = Some(m.trim().to_string());
        }
    }
    let cap = |s: &str| s.chars().take(TRIAGE_LINE_CHAR_CAP).collect::<String>();
    Some(serde_json::json!({
        "at": at,
        "appVersion": app_version.unwrap_or_default(),
        "message": cap(message.as_deref().unwrap_or("")),
    }))
}

/// 从日志尾部提取去重后的最近 ERROR 行（纯函数，供单测钉死）。
/// 去重键 = 掐掉行首时间戳两组 `[..]` 后、数字归一为 `#` 的整行；
/// 同键多次出现保留**最后一次**（早期已修复的偶发不挤占额度），输出按发生顺序。
fn summarize_recent_errors(tail: &str) -> Option<Value> {
    let mut kept: Vec<(String, String)> = Vec::new(); // (key, redacted line)
    for line in tail.lines() {
        let Some(rest) = strip_log_timestamp(line) else {
            continue;
        };
        // rest 以等级词开头（`ERROR][target] …`）
        if !rest.starts_with("ERROR]") {
            continue;
        }
        let key: String = rest
            .chars()
            .map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect();
        kept.retain(|(k, _)| *k != key);
        let redacted = crate::diagnostics::redact_file_for_export(&cap_line(line));
        kept.push((key, redacted));
        if kept.len() > TRIAGE_MAX_RECENT_ERRORS {
            kept.remove(0);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(Value::Array(
        kept.into_iter()
            .map(|(_, line)| Value::String(line))
            .collect(),
    ))
}

/// 掐掉行首 `[date][time]` 两组；不是这个形状的行返回 `None`。
/// 掐掉行首 `[date][time]` 两组，返回余下部分。组间分隔符 `][` 的 `[` 属于下一组，
/// 跳过整段分隔符后返回值以**等级词**开头（如 `ERROR][target] 消息`）。
fn strip_log_timestamp(line: &str) -> Option<&str> {
    let after_date = line.strip_prefix("[20")?;
    let sep = after_date.find("][")?;
    let after_time = after_date.get(sep + 2..)?;
    let sep = after_time.find("][")?;
    let rest = after_time.get(sep + 2..)?;
    // 余下部分须以合法等级词（ERROR/WARN/INFO…）开头，防止误吞正文
    let level_end = rest.find(']')?;
    let level = rest.get(..level_end)?;
    if !level.is_empty() && level.chars().all(|c| c.is_ascii_uppercase()) {
        Some(rest)
    } else {
        None
    }
}

fn cap_line(line: &str) -> String {
    line.chars().take(TRIAGE_LINE_CHAR_CAP).collect()
}

/// 收集诊断段。
///
/// `env_manifest` 是命令层拼好的环境事实（版本/OS/工具版本/代理/设置摘要/计数），
/// 构建器只补 `generatedAt` / `includeSites` / `skippedFiles` 三个包内事实 ——
/// 「环境里有什么」归命令层，「包里长什么样」归这里。
///
/// `site_origins`：`Some` = 用户勾选了附带站点域名清单（仅 origin，绝不含密钥）。
pub(crate) fn collect_diagnostics(
    env_manifest: Value,
    site_origins: Option<Vec<String>>,
) -> Result<CollectedDiagnostics, AppError> {
    let root = crate::panic_hook::diagnostic_root_path();
    let mut entries = Vec::new();
    let mut skipped_files = Vec::new();

    for top_level in crate::panic_hook::diagnostic_entries() {
        let path = root.join(&top_level);
        let base_name = top_level.to_string_lossy().replace('\\', "/");
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                collect_directory(&path, &base_name, &mut entries, &mut skipped_files)?;
            }
            Ok(meta) if meta.is_file() => {
                push_redacted_file(&path, &base_name, &mut entries, &mut skipped_files);
            }
            // 不存在 = 这台机器没产生过这类件（比如没崩过），正常。
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::io(&path, error));
            }
        }
    }

    let mut manifest = env_manifest;
    if let Some(origins) = site_origins {
        let mut sorted = origins;
        sorted.sort();
        sorted.dedup();
        entries.push((
            "sites.txt".to_string(),
            format!("{}\n", sorted.join("\n")).into_bytes(),
        ));
        manifest["includeSites"] = Value::Bool(true);
    } else {
        manifest["includeSites"] = Value::Bool(false);
    }
    manifest["generatedAt"] = Value::String(
        chrono::Local::now()
            .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
            .to_string(),
    );
    if !skipped_files.is_empty() {
        manifest["skippedFiles"] = serde_json::to_value(&skipped_files)
            .map_err(|e| AppError::JsonSerialize { source: e })?;
    }

    Ok(CollectedDiagnostics { manifest, entries })
}

fn collect_directory(
    dir: &Path,
    zip_prefix: &str,
    entries: &mut Vec<(String, Vec<u8>)>,
    skipped_files: &mut Vec<String>,
) -> Result<(), AppError> {
    let mut children: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| AppError::io(dir, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::io(dir, e))?;
    children.sort_by_key(|child| child.file_name());

    for child in children {
        let path = child.path();
        let name = child.file_name().to_string_lossy().replace('\\', "/");
        // 日志目录里不该有子目录；出现也只是跳过，不让整包失败。
        if !path.is_file() {
            continue;
        }
        let zip_path = format!("{zip_prefix}/{name}");
        push_redacted_file(&path, &zip_path, entries, skipped_files);
    }
    Ok(())
}

fn push_redacted_file(
    path: &Path,
    zip_path: &str,
    entries: &mut Vec<(String, Vec<u8>)>,
    skipped_files: &mut Vec<String>,
) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() > MAX_EXPORT_FILE_BYTES {
        log::warn!(
            "诊断包跳过异常大文件 {}: {} 字节",
            path.display(),
            meta.len()
        );
        skipped_files.push(zip_path.to_string());
        return;
    }
    match std::fs::read(path) {
        Ok(raw) => {
            let redacted =
                crate::diagnostics::redact_file_for_export(&String::from_utf8_lossy(&raw));
            entries.push((zip_path.to_string(), redacted.into_bytes()));
        }
        Err(error) => {
            // 单个文件读不了不阻塞整包（日志可能正被本进程轮转），记名即可。
            log::warn!("诊断包读取 {} 失败（跳过）: {error}", path.display());
            skipped_files.push(zip_path.to_string());
        }
    }
}

/// 纯诊断包（本地导出与反馈附带共用）：manifest + 收集件。
pub(crate) fn build_diagnostics_zip(
    diagnostics: CollectedDiagnostics,
) -> Result<Vec<u8>, AppError> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let manifest_json = serde_json::to_vec_pretty(&diagnostics.manifest)
        .map_err(|e| AppError::JsonSerialize { source: e })?;
    entries.push(("manifest.json".to_string(), manifest_json));
    entries.extend(diagnostics.entries);
    build_zip(entries)
}

fn build_zip(entries: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, AppError> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());

    for (path, bytes) in entries {
        writer
            .start_file(path.as_str(), options)
            .map_err(|e| AppError::Config(format!("诊断包写入 zip 条目 {path} 失败: {e}")))?;
        writer
            .write_all(&bytes)
            .map_err(|e| AppError::Config(format!("诊断包写入 zip 内容 {path} 失败: {e}")))?;
    }

    writer
        .finish()
        .map_err(|e| AppError::Config(format!("诊断包 zip 收尾失败: {e}")))
        .map(|cursor| cursor.into_inner())
}

/// 截图文件名收窄到 zip 安全字符集（跨平台 path separator 也不进来）。
pub(crate) fn sanitize_attachment_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "screenshot".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read_zip_entry(bytes: &[u8], name: &str) -> String {
        let cursor = std::io::Cursor::new(bytes.to_vec());
        let mut archive = zip::ZipArchive::new(cursor).expect("zip 应可解析");
        let mut entry = archive.by_name(name).expect("zip 条目应存在");
        let mut out = String::new();
        std::io::Read::read_to_string(&mut entry, &mut out).expect("zip 条目应可读");
        out
    }

    /// 把诊断根目录指到一个装好假日志/假 crash 件的临时目录，返回时自动复原。
    fn with_diagnostic_root() -> crate::panic_hook::test_root_override::Guard {
        let root = std::env::temp_dir().join(format!(
            "lp-diag-export-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("logs")).expect("logs dir");
        std::fs::write(
            root.join("logs").join("loongport.log"),
            "url=https://user:pass@example.com/v1\nplain line\nAuthorization: Bearer sk-topsecret000000\n",
        )
        .expect("log file");
        std::fs::write(
            root.join("crash.log"),
            "-----BEGIN PRIVATE KEY-----\nkey-material-line\n-----END PRIVATE KEY-----\n",
        )
        .expect("crash file");
        crate::panic_hook::test_root_override::install(root)
    }

    #[test]
    fn collected_files_are_redacted_and_sites_are_opt_in() {
        let _root = with_diagnostic_root();

        let without_sites = collect_diagnostics(json!({"appVersion": "6.25.0"}), None).unwrap();
        assert!(
            without_sites
                .entries
                .iter()
                .all(|(path, _)| path != "sites.txt"),
            "未勾选时不得出现站点清单"
        );
        assert_eq!(
            without_sites.manifest["includeSites"],
            json!(false),
            "manifest 必须如实记录未含站点清单"
        );

        let log_entry = without_sites
            .entries
            .iter()
            .find(|(path, _)| path == "logs/loongport.log")
            .expect("日志应入包");
        let text = String::from_utf8_lossy(&log_entry.1).to_string();
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("sk-topsecret000000"));
        assert!(text.contains("plain line"), "非敏感行必须保留");

        let crash_entry = without_sites
            .entries
            .iter()
            .find(|(path, _)| path == "crash.log")
            .expect("crash.log 应入包");
        assert!(!String::from_utf8_lossy(&crash_entry.1).contains("key-material-line"));

        let with_sites = collect_diagnostics(
            json!({}),
            Some(vec!["z.example".into(), "a.example".into()]),
        )
        .unwrap();
        let sites = with_sites
            .entries
            .iter()
            .find(|(path, _)| path == "sites.txt")
            .expect("勾选后站点清单应入包");
        assert_eq!(
            std::str::from_utf8(&sites.1).unwrap(),
            "a.example\nz.example\n"
        );
        assert_eq!(with_sites.manifest["includeSites"], json!(true));
    }

    #[test]
    fn diagnostics_zip_roundtrips_through_zip_archive() {
        let _root = with_diagnostic_root();
        let collected = collect_diagnostics(json!({"appVersion": "6.25.0"}), None).unwrap();
        let zip_bytes = build_diagnostics_zip(collected).unwrap();

        let manifest = read_zip_entry(&zip_bytes, "manifest.json");
        let parsed: Value = serde_json::from_str(&manifest).expect("manifest 应是合法 JSON");
        assert_eq!(parsed["appVersion"], json!("6.25.0"));
        assert!(read_zip_entry(&zip_bytes, "logs/loongport.log").contains("plain line"));
    }

    #[test]
    fn attachment_names_never_carry_path_separators() {
        for input in ["../../etc/passwd", "a/b\\c.png", "。。。", ""] {
            let name = sanitize_attachment_name(input);
            assert!(!name.contains('/'), "{input} → {name} 不该带路径分隔符");
            assert!(!name.contains('\\'), "{input} → {name} 不该带反斜杠");
            assert!(!name.is_empty());
        }
    }

    /// 装好自定义诊断根（返回路径供用例继续写件），返回时自动复原。
    fn with_custom_diagnostic_root() -> (
        crate::panic_hook::test_root_override::Guard,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "lp-triage-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("logs")).expect("logs dir");
        let guard = crate::panic_hook::test_root_override::install(root.clone());
        (guard, root)
    }

    #[test]
    fn triage_summary_reports_last_crash_and_deduped_recent_errors() {
        let (_guard, root) = with_custom_diagnostic_root();
        std::fs::write(
            root.join("crash.log"),
            "================================================================================\n[CRASH REPORT] 2026-09-06 09:57:55.231\n\
             Message: old crash in 6.15.0\n\
             ================================================================================================\n[CRASH REPORT] 2026-09-15 20:00:17.229\n\
             ----------------------------------------\nSystem Information\n----------------------------------------\n\
             App Version: 6.24.0\n\
             Message: cannot execute `LocalPool` executor from within another executor: EnterError\n",
        )
        .expect("crash file");
        std::fs::write(
            root.join("logs").join("loongport.log"),
            "[2026-09-19][16:16:34][ERROR][cc_switch_lib::proxy::response_processor] [Codex] 流错误: error decoding response body\n\
             [2026-09-20][21:59:08][ERROR][cc_switch_lib::proxy::response_processor] [Codex] 流式响应静默期超时 (120秒)\n\
             [2026-09-21][14:49:20][ERROR][cc_switch_lib::proxy::response_processor] [Codex] 流式响应静默期超时 (120秒)\n\
             [2026-09-21][15:00:00][WARN][cc_switch_lib::services::proxy] 无关的 WARN 行不该进摘要\n\
             正文里带 [ERROR] 字样但行首不是时间戳的行不该进摘要\n\
             [2026-09-22][10:00:00][ERROR][tauri_plugin_updater::updater] failed to check for updates: error sending request for url (https://user:secret@example.com/api)\n",
        )
        .expect("log file");

        let summary = build_triage_summary();
        let crash = &summary["lastCrash"];
        assert_eq!(crash["at"], json!("2026-09-15 20:00:17.229"));
        assert_eq!(crash["appVersion"], json!("6.24.0"));
        assert!(crash["message"]
            .as_str()
            .expect("message 应是字符串")
            .contains("LocalPool"));

        let errors = summary["recentErrors"]
            .as_array()
            .expect("recentErrors 应是数组");
        assert_eq!(errors.len(), 3, "同键两次只留最后一次");
        let joined = serde_json::to_string(&errors).expect("serialize");
        assert!(joined.contains("14:49:20"), "同键保留最后一次出现");
        assert!(!joined.contains("21:59:08"), "被去重的旧出现不保留");
        assert!(!joined.contains("WARN"));
        assert!(
            joined.contains("[REDACTED]@example.com"),
            "URL 凭据须过同一脱敏链: {joined}"
        );
    }

    #[test]
    fn triage_summary_is_null_without_signals() {
        let (_guard, root) = with_custom_diagnostic_root();
        std::fs::write(
            root.join("logs").join("loongport.log"),
            "[2026-09-21][15:00:00][INFO][cc_switch_lib] 只有 INFO，无信号\n",
        )
        .expect("log file");

        assert_eq!(build_triage_summary(), Value::Null);
    }

    #[test]
    fn recent_errors_cap_lines_and_count() {
        let mut lines = String::new();
        for i in 0..12u8 {
            let kind = (b'a' + i) as char;
            lines.push_str(&format!(
                "[2026-09-2{i}][10:00:00][ERROR][some::module] 错误 kind-{kind}\n"
            ));
        }
        let summary = summarize_recent_errors(&lines).expect("应有错误行");
        let arr = summary.as_array().expect("数组");
        assert_eq!(arr.len(), TRIAGE_MAX_RECENT_ERRORS, "条数封顶");
        assert!(
            serde_json::to_string(&arr).unwrap().contains("错误 kind-l"),
            "保留最新的，挤掉最旧的"
        );

        let long = "[2026-09-21][15:00:00][ERROR][m] ".to_string() + &"x".repeat(500);
        let capped = summarize_recent_errors(&format!("{long}\n")).unwrap();
        assert!(
            capped[0].as_str().unwrap().chars().count() <= TRIAGE_LINE_CHAR_CAP,
            "单行字符数封顶"
        );
    }
}
