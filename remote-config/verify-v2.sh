#!/usr/bin/env bash
#
# 验证 v2 provider policy 的原始字节 Ed25519 签名。默认同时下载线上文件并与
# 本地进行字节比对；--local-only 仅验待发布文件，适用于签名后的本地检查。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIRECTORY="$HERE/public/v2/directory.json"
SIG="$HERE/public/v2/directory.json.sig"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"
LOCAL_ONLY=false

if [ "$#" -gt 1 ]; then
  echo "用法：$0 [--local-only]" >&2
  exit 2
fi

case "${1:-}" in
  "") ;;
  --local-only) LOCAL_ONLY=true ;;
  *)
    echo "用法：$0 [--local-only]" >&2
    exit 2
    ;;
esac

# shellcheck source=lib.sh
. "$HERE/lib.sh"

require_openssl_with_ed25519

if [ ! -f "$RS" ]; then
  echo "✘ 找不到 remote_config.rs：$RS" >&2
  echo "  （用 LOONGPORT_REMOTE_CONFIG_RS 指定路径）" >&2
  exit 1
fi

PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")

verify_local() {
  if [ ! -f "$DIRECTORY" ] || [ ! -f "$SIG" ]; then
    echo "✘ 缺 v2 directory.json 或其签名；先跑 ./sign-v2.sh" >&2
    return 1
  fi
  if ! verify_signature "$DIRECTORY" "$SIG" "$PUBKEY_HEX"; then
    echo "✘ 本地 v2 签名验签失败；policy 消费者必须拒绝它。" >&2
    return 1
  fi
  validate_directory_v2_json "$DIRECTORY"
}

verify_local
echo "✔ 本地 v2 policy 原始字节签名与契约均通过验证"

if [ "$LOCAL_ONLY" = true ]; then
  exit 0
fi

DIRECTORY_URL=$(rc_const DIRECTORY_V2_URL "$RS")
SIGNATURE_URL=$(rc_const DIRECTORY_V2_SIGNATURE_URL "$RS")
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL "$DIRECTORY_URL" -o "$TMP/directory.json"
curl -fsSL "$SIGNATURE_URL" -o "$TMP/directory.json.sig"

if ! verify_signature "$TMP/directory.json" "$TMP/directory.json.sig" "$PUBKEY_HEX"; then
  echo "✘ 线上 v2 policy 签名验签失败；消费者必须拒绝它。" >&2
  exit 1
fi
if ! diff -q "$TMP/directory.json" "$DIRECTORY" > /dev/null 2>&1; then
  echo "✘ 线上 v2 policy 与本地待发布 JSON 不一致。" >&2
  exit 1
fi

echo "✔ 线上 v2 policy 已通过签名验证，并与本地 JSON 字节一致"
