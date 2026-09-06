#!/usr/bin/env bash
#
# 给 public/v2/config.json 生成 Ed25519 签名（config.json.sig）。
#
# 签的是**原始字节**，不是解析后的结构 —— 客户端 `parse_verified` 先验签再解析，
# 所以这个文件被改一个字节（哪怕只是空白）签名就失效。
# ⇒ **改完 config.json 必须重跑本脚本**，两个文件一起发布。
#
# v2 是现行世代（客户端 CONFIG_URL 指向这里）；v1 已冻结，改 v1 才用 ./sign.sh。
# 私钥由授权维护者在仓外通过 LOONGPORT_CONFIG_KEY 提供，绝不进仓库或日志。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG="$HERE/public/v2/config.json"
SIG="$HERE/public/v2/config.json.sig"
KEY_PATH="${LOONGPORT_CONFIG_KEY:-}"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"

# shellcheck source=lib.sh
. "$HERE/lib.sh"

require_openssl_with_ed25519

if [ -z "$KEY_PATH" ] || [ ! -f "$KEY_PATH" ]; then
  echo "✘ LOONGPORT_CONFIG_KEY 必须由授权维护者提供可读的 Ed25519 私钥文件" >&2
  exit 1
fi
if [ ! -f "$CONFIG" ]; then
  echo "✘ 找不到 $CONFIG" >&2
  exit 1
fi

# JSON 先过一遍语法检查 + 客户端契约检查。
# 签一份客户端解不出的 JSON 是最难查的失败模式：签名会验过，
# 然后客户端在 `serde_json::from_slice` 那步失败并**丢弃整份配置**。
validate_config_json "$CONFIG"

# ⚠️ **先写临时文件，验过再 mv 上去** —— 直接 `-out "$SIG"` 的话，
# openssl 一失败就把现有那份好签名**截断成 0 字节**。`mv` 同文件系统内是原子的。
TMP_SIG="$(mktemp)"
trap 'rm -f "$TMP_SIG"' EXIT
openssl pkeyutl -sign -inkey "$KEY_PATH" -rawin -in "$CONFIG" -out "$TMP_SIG"

# ⭐ 自验用**代码里那把公钥**（rc_const 从 remote_config.rs 取），不是从私钥
# 现导出的那把 —— 后者是套套逻辑，抓不出「私钥换了而代码公钥没同步」。
PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")
if ! verify_signature "$CONFIG" "$TMP_SIG" "$PUBKEY_HEX"; then
  echo "✘ 新签名无法通过 production public key 验证；现有 v2 签名未改动。" >&2
  exit 1
fi

mv "$TMP_SIG" "$SIG"
chmod 644 "$SIG"
echo "✔ v2 config 已按原始字节签名，并通过 production public key 验证"
