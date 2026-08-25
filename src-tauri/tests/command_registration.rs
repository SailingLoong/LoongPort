//! 命令注册完整性闸：每个 `#[tauri::command]` 函数必须进 `generate_handler!`。
//!
//! 为什么要有这条：Tauri 的 `invoke` 是运行时字符串，命令漏注册时编译器、
//! clippy、CI 全都无感（v6.5.0 的 `relay_open_usage` 漏注册带病发版，
//! 用户点击「查看用量」即报 Command not found，v6.5.2 才修复）。本地
//! 单测又直接驱动内部函数、绕过注册层，所以只能在源码层面收这道闸。
//!
//! 唯一豁免 [`KNOWN_UNREGISTERED`]：定义了但有意不注册的命令。豁免必须
//! 双向核验（确实定义着、确实没注册），防止名单腐化成永久免死金牌。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 有定义、但有意不注册进 IPC 的命令（豁免名单，新增须写明原因）。
const KNOWN_UNREGISTERED: &[&str] = &[
    // 上游传下来的热切换命令，前后端都没有调用方；不删是为了少给上游
    // 合并添冲突。见 src/commands/proxy.rs。
    "switch_proxy_provider",
];

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 从 lib.rs 的 `generate_handler![...]` 块里取已注册的命令名集合。
fn registered_commands() -> BTreeMap<String, ()> {
    let lib_rs = fs::read_to_string(src_dir().join("lib.rs")).expect("读 src/lib.rs");
    let block = lib_rs
        .split("generate_handler![")
        .nth(1)
        .expect("lib.rs 里应有 generate_handler! 注册块")
        .split("];")
        .next()
        .expect("注册块应以 ]; 结束");

    block
        .lines()
        .filter_map(|line| {
            let entry = line.split("//").next()?.trim();
            let entry = entry.strip_suffix(',')?;
            let name = entry.rsplit("::").next()?.trim();
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
                Some((name.to_string(), ()))
            } else {
                None
            }
        })
        .collect()
}

/// 扫全仓 `src/`，收集带 `#[tauri::command]` 属性的函数：名字 → 所在文件。
fn defined_commands() -> BTreeMap<String, String> {
    let mut defined = BTreeMap::new();
    let mut files = vec![src_dir()];
    while let Some(dir) = files.pop() {
        for entry in fs::read_dir(&dir).expect("遍历 src/ 目录") {
            let path = entry.expect("读目录项").path();
            if path.is_dir() {
                files.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            collect_from_file(&path, &mut defined);
        }
    }
    defined
}

fn collect_from_file(path: &Path, defined: &mut BTreeMap<String, String>) {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(_) => return,
    };
    let mut lines = source.lines().enumerate().peekable();
    while let Some((_, line)) = lines.next() {
        if !line.trim_start().starts_with("#[tauri::command") {
            continue;
        }
        // 属性后面可能还隔着别的属性行 / 文档注释（含 #[tauri::command(...)]
        // 带参形态自身的续行），一路向下找到函数签名行。
        for (_, next) in lines.by_ref() {
            // 文档/普通注释行里也可能出现 "fn " 字样，不算签名。
            if next.trim_start().starts_with("//") {
                continue;
            }
            let Some(fn_pos) = next.find("fn ") else {
                continue;
            };
            let after_fn = next[fn_pos + 3..].trim_start();
            let name: String = after_fn
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                defined.insert(
                    name,
                    path.strip_prefix(src_dir())
                        .unwrap_or(path)
                        .display()
                        .to_string(),
                );
            }
            break;
        }
    }
}

#[test]
fn every_defined_command_is_registered() {
    let registered = registered_commands();
    let defined = defined_commands();

    let missing: Vec<&String> = defined
        .keys()
        .filter(|name| !registered.contains_key(*name))
        .filter(|name| !KNOWN_UNREGISTERED.contains(&name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "以下 #[tauri::command] 未注册进 lib.rs 的 generate_handler!（前端 invoke 会报 \
         Command not found）：{missing:?}\n若是有意不注册，请加入本测试的 \
         KNOWN_UNREGISTERED 并写明原因。"
    );
}

#[test]
fn known_unregistered_allowlist_stays_honest() {
    let registered = registered_commands();
    let defined = defined_commands();

    for name in KNOWN_UNREGISTERED {
        assert!(
            defined.contains_key(*name),
            "豁免名单里的 {name} 已不存在于源码，请从 KNOWN_UNREGISTERED 删除"
        );
        assert!(
            !registered.contains_key(*name),
            "豁免名单里的 {name} 已经注册进 generate_handler! 了，请从 KNOWN_UNREGISTERED 删除"
        );
    }
}
