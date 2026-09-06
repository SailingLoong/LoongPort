#!/usr/bin/env bash
#
# 给 public/v2/plaza.json（广场展示策略）生成 detached Ed25519 签名。
#
# 签名覆盖原始 JSON 字节；任何改动（包括空白）都必须重新签名。私钥不会被
# 写入仓库，调用者通过 LOONGPORT_CONFIG_KEY 在运行时提供它。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLAZA="$HERE/public/v2/plaza.json"
SIG="$HERE/public/v2/plaza.json.sig"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"

# shellcheck source=lib.sh
. "$HERE/lib.sh"

require_openssl_with_ed25519

if [ -z "${LOONGPORT_CONFIG_KEY:-}" ] || [ ! -f "$LOONGPORT_CONFIG_KEY" ]; then
  echo "✘ LOONGPORT_CONFIG_KEY 必须指向可读的 Ed25519 私钥文件" >&2
  exit 1
fi
if [ ! -f "$RS" ]; then
  echo "✘ 找不到 remote_config.rs：$RS" >&2
  echo "  （用 LOONGPORT_REMOTE_CONFIG_RS 指定路径）" >&2
  exit 1
fi

validate_plaza_json "$PLAZA"

TMP_SIG="$(mktemp)"
trap 'rm -f "$TMP_SIG"' EXIT
openssl pkeyutl -sign -inkey "$LOONGPORT_CONFIG_KEY" -rawin -in "$PLAZA" -out "$TMP_SIG"

PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")
if ! verify_signature "$PLAZA" "$TMP_SIG" "$PUBKEY_HEX"; then
  echo "✘ 新签名无法通过 production public key 验证；现有 plaza 签名未改动。" >&2
  exit 1
fi

mv "$TMP_SIG" "$SIG"
chmod 644 "$SIG"
echo "✔ v2 plaza 已按原始字节签名，并通过 production public key 验证"
