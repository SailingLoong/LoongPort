#!/usr/bin/env bash
#
# 拉**线上**那两个文件，用**代码里那把公钥**验一次。发布后跑。
#
# 为什么需要它（代码仓的单测覆盖不到这件事）：
# 单测用现场生成的密钥对验「机制」对不对，覆盖不了「线上部署的那份文件，
# 跟客户端烧着的那把公钥，是不是配套的」。后者的失败模式最难查 ——
# 客户端静默丢弃整份配置、回落到内置表、**不报任何错**，
# 表现成「改了配置没生效」。
#
# 公钥从 remote_config.rs 里 grep 出来而不是手抄 —— 手抄就多一个能写错的地方。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# shellcheck source=lib.sh
. "$HERE/lib.sh"

# ⚠️ 这道必须在最前面。少了它，LibreSSL 下的验签失败会被报成
# 「线上配置验签失败 —— 改完 JSON 忘了重签？」⇒ **一份正确的配置被误诊**，
# 而真正的原因是 openssl 的品种不对。见 lib.sh 里的说明。
require_openssl_with_ed25519

if [ ! -f "$RS" ]; then
  echo "✘ 找不到 remote_config.rs：$RS" >&2
  echo "  （用 LOONGPORT_REMOTE_CONFIG_RS 指定路径）" >&2
  exit 1
fi

# 三个事实都从代码里取，不手抄。
CONFIG_URL=$(rc_const CONFIG_URL "$RS")
SIGNATURE_URL=$(rc_const SIGNATURE_URL "$RS")
PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")

echo "配置端点：${CONFIG_URL}"
echo "公钥     ：${PUBKEY_HEX}"
echo

curl -fsSL "$CONFIG_URL" -o "$TMP/config.json"
curl -fsSL "$SIGNATURE_URL" -o "$TMP/config.json.sig"

if verify_signature "$TMP/config.json" "$TMP/config.json.sig" "$PUBKEY_HEX"; then
  echo "✔ 线上配置验签通过 —— 客户端会接受这一份"
else
  echo "✘ 线上配置验签失败：客户端会**静默丢弃**它并回落到内置表" >&2
  echo "  常见原因：改了 config.json 但忘了重跑 sign.sh，或两个文件没一起发布" >&2
  exit 1
fi

# ⚠️ 同时比对「线上那份」与「本地待发布那份」——
# 只验签名通过还不够：线上可能是**上一次**发布的（旧但签名有效）。
if ! diff -q "$TMP/config.json" "$HERE/public/v1/config.json" > /dev/null 2>&1; then
  echo "⚠️ 线上那份与本地 public/v1/config.json **不一致** —— 大概是还没部署" >&2
  echo "   （或 CDN 缓存未过期，max-age=300，等 5 分钟再试）" >&2
  exit 1
fi

echo "✔ 线上那份与本地待发布的一致"
echo
echo "线上配置内容："
python3 -m json.tool "$TMP/config.json"
