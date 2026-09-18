//! Panic Hook 模块
//!
//! 在应用崩溃时捕获 panic 信息并记录到 `<app_config_dir>/crash.log` 文件中（默认 `~/.loongport/crash.log`）。
//! 便于用户和开发者诊断闪退问题。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 应用版本号（从 Cargo.toml 读取）
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const CRASH_LOG_MAX_SIZE: u64 = 5 * 1024 * 1024;
const CRASH_LOG_ARCHIVES_TO_KEEP: usize = 2;

static DIAGNOSTIC_ROOT: OnceLock<PathBuf> = OnceLock::new();
static CRASH_LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn init_app_config_dir(dir: PathBuf) -> Result<(), crate::error::AppError> {
    let root = diagnostic_root_for(&dir)?;
    crate::config::ensure_private_directory(&root)?;
    let _ = DIAGNOSTIC_ROOT.set(root);
    Ok(())
}

pub(crate) fn diagnostic_root_for(root: &Path) -> Result<PathBuf, crate::error::AppError> {
    if !crate::secrets::reset::pending(root)? {
        return Ok(root.to_owned());
    }
    // Resolve once during startup. Both ordinary and panic logging must stay
    // outside a root whose directory entry belongs to reset recovery.
    let mut name = root
        .file_name()
        .ok_or_else(|| crate::error::AppError::Config("secret.invalid_path".into()))?
        .to_os_string();
    name.push(".recovery-diagnostics");
    let parent = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| crate::error::AppError::Config("secret.invalid_path".into()))?;
    Ok(parent.join(name))
}

/// 获取默认应用配置目录（不会 panic）
fn default_app_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::config::APP_DIR_NAME)
}

/// 本进程固定使用的诊断目录；日志读取不触发凭据恢复。
fn diagnostic_root() -> PathBuf {
    DIAGNOSTIC_ROOT
        .get()
        .cloned()
        .unwrap_or_else(default_app_config_dir)
}

/// 获取崩溃日志文件路径
fn get_crash_log_path() -> PathBuf {
    diagnostic_root().join(CRASH_LOG_FILE)
}

fn rotated_crash_log_path(path: &Path, index: usize) -> PathBuf {
    let mut rotated = path.as_os_str().to_os_string();
    rotated.push(format!(".{index}"));
    PathBuf::from(rotated)
}

fn rotate_crash_log_if_needed_with_limit(
    path: &Path,
    max_size: u64,
    archives_to_keep: usize,
) -> std::io::Result<()> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if size < max_size || archives_to_keep == 0 {
        return Ok(());
    }

    for index in (1..=archives_to_keep).rev() {
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            rotated_crash_log_path(path, index - 1)
        };
        if !source.exists() {
            continue;
        }

        let destination = rotated_crash_log_path(path, index);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        fs::rename(source, destination)?;
    }

    Ok(())
}

fn rotate_crash_log_if_needed(path: &Path) -> std::io::Result<()> {
    rotate_crash_log_if_needed_with_limit(path, CRASH_LOG_MAX_SIZE, CRASH_LOG_ARCHIVES_TO_KEEP)
}

/// 获取日志目录路径
pub(crate) const LOG_DIRECTORY: &str = "logs";
pub(crate) const CRASH_LOG_FILE: &str = "crash.log";

pub(crate) fn diagnostic_entries() -> Vec<std::ffi::OsString> {
    std::iter::once(std::ffi::OsString::from(LOG_DIRECTORY))
        .chain(std::iter::once(std::ffi::OsString::from(CRASH_LOG_FILE)))
        .chain(
            (1..=CRASH_LOG_ARCHIVES_TO_KEEP)
                .map(|index| std::ffi::OsString::from(format!("{CRASH_LOG_FILE}.{index}"))),
        )
        .collect()
}

pub(crate) fn is_diagnostic_path(relative: &Path) -> bool {
    relative.components().next().is_some_and(|first| {
        diagnostic_entries()
            .iter()
            .any(|name| name == first.as_os_str())
    })
}

pub fn get_log_dir() -> PathBuf {
    diagnostic_root().join(LOG_DIRECTORY)
}

/// 诊断根目录（crash.log 与 logs/ 的父目录）。诊断包导出用它枚举全部诊断件。
pub(crate) fn diagnostic_root_path() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = test_root_override::get() {
        return dir;
    }
    diagnostic_root()
}

/// 测试专用：把诊断根目录指到临时目录（互斥 + 自动复原）。
///
/// 诊断包构建器的单测需要喂假日志/假 crash 件，而 `diagnostic_root()` 读进程级
/// `OnceLock`。形状照 `remote_config` 的 `CacheDirGuard`：安装即持锁（用例串行），
/// Drop 时复原并清理临时目录。
#[cfg(test)]
pub(crate) mod test_root_override {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    static OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

    fn lock() -> &'static Mutex<()> {
        static LOCK: Mutex<()> = Mutex::new(());
        &LOCK
    }

    pub(crate) struct Guard {
        dir: PathBuf,
        _lock: MutexGuard<'static, ()>,
    }

    pub(crate) fn install(dir: PathBuf) -> Guard {
        let lock_guard = lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *OVERRIDE.lock().unwrap() = Some(dir.clone());
        Guard {
            dir,
            _lock: lock_guard,
        }
    }

    pub(crate) fn get() -> Option<PathBuf> {
        OVERRIDE.lock().ok()?.clone()
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            *OVERRIDE.lock().unwrap() = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// 安全获取环境信息（不会 panic）
fn get_system_info() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let family = std::env::consts::FAMILY;

    // 安全获取当前工作目录
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // 安全获取当前线程信息
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let thread_id = format!("{:?}", thread.id());

    format!(
        "OS: {os} ({family})\n\
         Arch: {arch}\n\
         App Version: {APP_VERSION}\n\
         Working Dir: {cwd}\n\
         Thread: {thread_name} (ID: {thread_id})"
    )
}

fn format_crash_entry(
    timestamp: &str,
    system_info: &str,
    message: &str,
    location: &str,
    backtrace: &str,
) -> String {
    let message = crate::diagnostics::redact_log_text(message);
    let separator = "=".repeat(80);
    let sub_separator = "-".repeat(40);
    format!(
        r#"
{separator}
[CRASH REPORT] {timestamp}
{separator}

{sub_separator}
System Information
{sub_separator}
{system_info}

{sub_separator}
Error Details
{sub_separator}
Message: {message}

Location: {location}

{sub_separator}
Stack Trace (Backtrace)
{sub_separator}
{backtrace}

{separator}
"#
    )
}

fn append_crash_entry(path: &Path, entry: &str) -> std::io::Result<()> {
    rotate_crash_log_if_needed(path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(entry.as_bytes())?;
    file.flush()
}

/// 设置 panic hook，捕获崩溃信息并写入日志文件
///
/// 在应用启动时调用此函数，确保任何 panic 都会被记录。
/// 日志格式包含：
/// - 时间戳
/// - 应用版本和系统信息
/// - Panic 信息
/// - 发生位置（文件:行号）
/// - Backtrace（完整调用栈）
pub fn setup_panic_hook() {
    // 启用 backtrace（确保 release 模式也能捕获）
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    panic::set_hook(Box::new(move |panic_info| {
        let log_path = get_crash_log_path();

        // 确保目录存在
        if let Some(parent) = log_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("[LoongPort] failed to create crash log directory: {error}");
            }
        }

        // 构建崩溃信息（使用 catch_unwind 保护时间格式化，避免嵌套 panic）
        let timestamp = std::panic::catch_unwind(|| {
            chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_else(|_| {
            // chrono panic 时回退到 unix timestamp
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| format!("unix:{}.{:03}", d.as_secs(), d.subsec_millis()))
                .unwrap_or_else(|_| "unknown".to_string())
        });

        // 获取系统信息
        let system_info = std::panic::catch_unwind(get_system_info)
            .unwrap_or_else(|_| "Failed to get system info".to_string());

        // 获取 panic 消息（尝试多种方式提取）
        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            // 尝试使用 Display trait
            format!("{panic_info}")
        };

        // 获取位置信息
        let location = if let Some(loc) = panic_info.location() {
            format!(
                "File: {}\n         Line: {}\n         Column: {}",
                loc.file(),
                loc.line(),
                loc.column()
            )
        } else {
            "Unknown location".to_string()
        };

        // 捕获 backtrace（完整调用栈）
        let backtrace = std::backtrace::Backtrace::force_capture();
        let backtrace_str = format!("{backtrace}");

        // 格式化日志条目。panic payload 可能携带请求失败正文或凭据，
        // crash.log 不经过 tauri-plugin-log formatter，因此必须显式复用同一脱敏策略。
        let crash_entry = format_crash_entry(
            &timestamp,
            &system_info,
            &message,
            &location,
            &backtrace_str,
        );

        // 将 size check、轮转和追加合成同一个临界区，避免多线程同时 panic
        // 时两个 hook 竞争 rename 而丢失归档。
        let crash_log_guard = CRASH_LOG_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let save_result = append_crash_entry(&log_path, &crash_entry);
        drop(crash_log_guard);

        match save_result {
            Ok(()) => eprintln!("\n[LoongPort] Crash log saved to: {}", log_path.display()),
            Err(error) => eprintln!(
                "\n[LoongPort] Failed to save crash log at {}: {error}",
                log_path.display()
            ),
        }

        // 同时输出到 stderr（便于开发调试）
        eprintln!("{crash_entry}");

        // 不再调用默认 hook：它会把未经脱敏的 panic payload 再次写到 stderr。
        // 上面的 crash_entry 已包含同等的消息、位置和完整 backtrace。
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crash_log_path() {
        let path = get_crash_log_path();
        assert!(path.ends_with("crash.log"));
        assert!(path.to_string_lossy().contains(crate::config::APP_DIR_NAME));
    }

    #[test]
    fn test_system_info() {
        let info = get_system_info();
        assert!(info.contains("OS:"));
        assert!(info.contains("Arch:"));
        assert!(info.contains("App Version:"));
    }

    #[test]
    fn crash_entries_redact_sensitive_panic_messages() {
        let entry = format_crash_entry(
            "2026-08-11 12:00:00",
            "OS: test",
            "Authorization: Bearer sk-private-token\nresponse_body=<html>secret</html>",
            "src/example.rs:1",
            "stack",
        );

        assert!(!entry.contains("sk-private-token"));
        assert!(!entry.contains("<html>secret</html>"));
        assert!(entry.contains("[REDACTED]"));
        assert!(entry.contains("[REDACTED BODY]"));
    }

    #[test]
    fn crash_log_rotation_keeps_bounded_archives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.log");

        fs::write(&path, b"first").unwrap();
        rotate_crash_log_if_needed_with_limit(&path, 4, 2).unwrap();
        assert!(!path.exists());
        assert_eq!(
            fs::read(rotated_crash_log_path(&path, 1)).unwrap(),
            b"first"
        );

        fs::write(&path, b"second").unwrap();
        rotate_crash_log_if_needed_with_limit(&path, 4, 2).unwrap();
        assert_eq!(
            fs::read(rotated_crash_log_path(&path, 1)).unwrap(),
            b"second"
        );
        assert_eq!(
            fs::read(rotated_crash_log_path(&path, 2)).unwrap(),
            b"first"
        );

        fs::write(&path, b"third").unwrap();
        rotate_crash_log_if_needed_with_limit(&path, 4, 2).unwrap();
        assert_eq!(
            fs::read(rotated_crash_log_path(&path, 1)).unwrap(),
            b"third"
        );
        assert_eq!(
            fs::read(rotated_crash_log_path(&path, 2)).unwrap(),
            b"second"
        );
        assert!(!rotated_crash_log_path(&path, 3).exists());
    }
}
