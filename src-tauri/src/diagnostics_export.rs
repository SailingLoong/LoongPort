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
}
