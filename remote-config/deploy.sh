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

# ⭐ 部署面完整性闸（2026-08-21 事故的机制修根）：
# `pages deploy` 会把 --cwd 里的 **functions/ 与整个 public/** 当作完整站点发布——
# 从一个不完整副本（比如缺 functions/ 或 v2 的镜像目录）出发，等于把线上端点
# **整份回退下线**，且不报任何错。所以先验本目录是不是完整部署面，宁可拒发。
for required in functions public/v1 public/v2 public/_headers; do
  if [ ! -e "$HERE/$required" ]; then
    echo "✘ 缺 ${required} —— 本目录不是完整部署面（会把线上端点整份回退）。" >&2
    echo "  编辑与部署只能从主仓的 remote-config/ 走；其他目录一律是只读镜像。" >&2
    exit 1
  fi
done

# 下面那道前置验签要用 Ed25519 —— macOS 自带的 LibreSSL 做不了，见 lib.sh。
require_openssl_with_ed25519

export CLOUDFLARE_ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-a2bffcffbb0d8145f4ba0a471b1afaec}"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  echo "✘ CLOUDFLARE_API_TOKEN 必须由授权维护者通过运行时环境提供" >&2
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

# `pages deploy` discovers Functions from its project working directory. The current
# Wrangler CLI deliberately has no separate Functions-directory option, so retain
# remote-config as `--cwd` while deploying its public/ assets.
npx wrangler pages deploy public --cwd "$HERE" \
  --project-name="$PROJECT" --branch=main --commit-dirty=true

echo
echo "✔ 已部署。等 ~30 秒（CDN max-age=300）后自动验线上……"
sleep 30
"$HERE/verify.sh"
"$HERE/verify-v2.sh"
echo
echo "✔ 线上 v1 + v2 双验签通过。"
