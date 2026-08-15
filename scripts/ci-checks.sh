#!/usr/bin/env bash
# 本地预检：与 .github/workflows/ci.yml 的 Backend Checks **逐字相同**的命令集。
#
# 为什么必须有这个脚本（2026-08-15 教训，PR #129/#134 两次踩实）：
# 本地手敲变体（不带 `-D warnings` 的 clippy、写模式的 fmt、grep 过滤输出）
# 会给出「本地绿、CI 红」的假绿 —— clippy 的警告被 grep 滤掉、fmt 只信写入
# 后的文件状态。工具链本身已被 rust-toolchain.toml 钉死，剩余漂移全部来自
# 命令不一致。改 CI 命令时同步改这里（两处各写一份的漂移由注释互相指认）。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --check --manifest-path src-tauri/Cargo.toml

echo "==> cargo clippy -D warnings"
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings

echo "==> cargo test"
cargo test --manifest-path src-tauri/Cargo.toml

echo "==> 本地预检全绿（WSL2 UNC 契约测试需 Windows runner，不在本地覆盖范围）"
