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

PUBKEY_HEX=$(rc_const PUBLIC_KEY_HEX "$RS")

# ⭐ 发之前按原始字节验每一个已发布的配置对。长度或修改时间只是代理指标；
# 只有签名实际覆盖当前 JSON 才能保证消费者会接受它。
verify_signed_pair() {
  local label="$1" config="$2" sig="$3" sign_command="$4"

  if [ ! -f "$config" ]; then
    echo "✘ 缺 ${label} 配置文件：${config}" >&2
    return 1
  fi
  if [ ! -f "$sig" ]; then
    echo "✘ 缺 ${label} 签名文件：${sig} —— 先跑 ${sign_command}" >&2
    return 1
  fi
  if ! verify_signature "$config" "$sig" "$PUBKEY_HEX"; then
    echo "✘ ${label} 本地签名验不过当前 JSON；部署已停止。" >&2
    echo "  改完 JSON 后先跑 ${sign_command}（公钥取自 remote_config.rs）。" >&2
    return 1
  fi
  echo "✔ ${label} 本地验签通过（公钥取自 remote_config.rs）"
}

verify_signed_pair \
  "v1 config.json" \
  "$HERE/public/v1/config.json" \
  "$HERE/public/v1/config.json.sig" \
  "./sign.sh"
verify_signed_pair \
  "v2 directory.json" \
  "$HERE/public/v2/directory.json" \
  "$HERE/public/v2/directory.json.sig" \
  "./sign-v2.sh"

npx wrangler pages deploy "$HERE/public" \
  --project-name="$PROJECT" --branch=main --commit-dirty=true

echo
echo "✔ 已部署。等 ~30 秒后依次跑 ./verify.sh 和 ./verify-v2.sh 验线上（CDN max-age=300）"
