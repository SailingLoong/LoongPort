#!/usr/bin/env bash
#
# 把 public/ 部署到 Cloudflare Pages 项目 loongport-config。
#
# 传**整个目录**而不是逐个文件 —— 那样不会漏掉 .sig
# （只发配置不发签名 = 客户端全部拒绝，见 README 的故障表）。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="loongport-config"
RS="${LOONGPORT_REMOTE_CONFIG_RS:-$HERE/../src-tauri/src/relay/remote_config.rs}"

# shellcheck source=lib.sh
. "$HERE/lib.sh"

# 下面那道前置验签要用 Ed25519 —— macOS 自带的 LibreSSL 做不了，见 lib.sh。
require_openssl_with_ed25519

export CLOUDFLARE_ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-a2bffcffbb0d8145f4ba0a471b1afaec}"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  CLOUDFLARE_API_TOKEN="$(security find-generic-password -a "$USER" \
    -s loongport-cloudflare-token -w 2>/dev/null || true)"
  export CLOUDFLARE_API_TOKEN
fi
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  echo "✘ 拿不到 CLOUDFLARE_API_TOKEN（Keychain 里没有 loongport-cloudflare-token）" >&2
  exit 1
fi

CONFIG="$HERE/public/v1/config.json"
SIG="$HERE/public/v1/config.json.sig"

if [ ! -f "$SIG" ]; then
  echo "✘ 缺 config.json.sig —— 先跑 ./sign.sh" >&2
  exit 1
fi

# ⭐ **发之前真验一次签**，用**客户端那把公钥**。
#
# review 抓出：原来这里只判「签名 64 字节」+「签名比配置新」，那两条都是
# **代理指标**。一个陈旧或不相干的 64 字节 .sig 被拷进来（或 touch 过）
# 两条都过得了，然后部署上去被客户端整份丢弃 —— 而线上看起来「一切正常」，
# 只有事后跑 verify.sh 才发现，那时生产已经坏了。
#
# 判「签名验得过这份 JSON 吗」才是真判据，且它天然覆盖了那两条代理指标。
PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")
if ! verify_signature "$CONFIG" "$SIG" "$PUBKEY_HEX"; then
  echo "✘ 本地签名验不过这份 config.json —— 客户端会整份丢弃它。" >&2
  echo "  改完 JSON 忘了重签？跑 ./sign.sh" >&2
  echo "  （用的公钥来自 ${RS}）" >&2
  exit 1
fi
echo "✔ 本地验签通过（公钥取自 remote_config.rs）"

npx wrangler pages deploy "$HERE/public" \
  --project-name="$PROJECT" --branch=main --commit-dirty=true

echo
echo "✔ 已部署。等 ~30 秒后跑 ./verify.sh 验线上（CDN max-age=300）"
