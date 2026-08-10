#!/usr/bin/env bash
#
# 给 public/v1/config.json 生成 Ed25519 签名（config.json.sig）。
#
# 签的是**原始字节**，不是解析后的结构 —— 客户端 `parse_verified` 先验签再解析，
# 所以这个文件被改一个字节（哪怕只是空白）签名就失效。
# ⇒ **改完 config.json 必须重跑本脚本**，两个文件一起发布。
#
# 私钥在仓外（见 KEY_PATH），绝不进仓库。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG="$HERE/public/v1/config.json"
SIG="$HERE/public/v1/config.json.sig"
KEY_PATH="${LOONGPORT_CONFIG_KEY:-$HOME/Documents/loongport-keys/remote-config-ed25519.pem}"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"

# shellcheck source=lib.sh
. "$HERE/lib.sh"

require_openssl_with_ed25519

if [ ! -f "$KEY_PATH" ]; then
  echo "✘ 找不到私钥：$KEY_PATH" >&2
  echo "  （用 LOONGPORT_CONFIG_KEY 指定别的路径）" >&2
  exit 1
fi

# JSON 先过一遍语法检查 + 客户端契约检查。
# 签一份客户端解不出的 JSON 是最难查的失败模式：签名会验过，
# 然后客户端在 `serde_json::from_slice` 那步失败并**丢弃整份配置**。
validate_config_json "$CONFIG"

# ⚠️ **先写临时文件，验过再 mv 上去** —— 直接 `-out "$SIG"` 的话，
# openssl 一失败就把现有那份好签名**截断成 0 字节**（`-out` 先建文件再写），
# 于是一次失败的签名尝试同时毁掉上一次的成果。`mv` 在同一文件系统内是原子的。
TMP_SIG="$(mktemp)"
trap 'rm -f "$TMP_SIG"' EXIT
openssl pkeyutl -sign -inkey "$KEY_PATH" -rawin -in "$CONFIG" -out "$TMP_SIG"

# ⭐ 自验用**代码里那把公钥**，不是从私钥现导出来的那把。
#
# 用后者只能证明「我确实用刚才那把私钥签的」—— 那是套套逻辑，永远成立。
# 用前者才能抓出真正会出事的那种情形：**私钥换了/指错了，而代码里的公钥没同步**。
# 那种情况下签名本身没问题，但客户端会整份丢弃，且症状与「服务器挂了」一样。
#
# `verify_signature` 顺带判了「恰好 64 字节」（客户端判等而不是判上限）。
PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")
if ! verify_signature "$CONFIG" "$TMP_SIG" "$PUBKEY_HEX"; then
  echo "✘ 签出来的签名，用客户端那把公钥验不过 ——" >&2
  echo "  说明 ${KEY_PATH} 与 remote_config.rs 里的 PUBLIC_KEY_HEX **不配对**。" >&2
  echo "  用错私钥了？还是换过密钥对但没同步代码里的公钥？" >&2
  echo "  （现有的 ${SIG} 未被改动）" >&2
  exit 1
fi

# 全部通过才落到最终位置。
mv "$TMP_SIG" "$SIG"
chmod 644 "$SIG"   # mktemp 建的是 600，发布的文件要可读

# ⚠️ 变量必须写 `${SIG}` 而不是 `$SIG` —— macOS 自带的 bash 3.2 会把紧跟其后的
# 全角括号当成变量名的一部分，报 `unbound variable`（配 `set -u` 时直接退出）。
echo "✔ 已签名（64 字节），并用客户端那把公钥验过：${SIG}"
echo "  公钥 ${PUBKEY_HEX}"
