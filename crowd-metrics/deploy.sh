#!/usr/bin/env bash
#
# 部署 loongport-metrics Worker（D1 + KV + cron）。
#
# 顺序：占位闸 → schema 重放（幂等） → wrangler deploy → 线上验证。

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

export CLOUDFLARE_ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-a2bffcffbb0d8145f4ba0a471b1afaec}"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  echo "✘ CLOUDFLARE_API_TOKEN 必须由授权维护者通过运行时环境提供" >&2
  exit 1
fi

# ⭐ 占位闸：D1/KV id 还没填时拒绝部署 —— 带占位的配置能 deploy 成功，
# 但绑定的资源不存在，线上会以 500 回应，而且不容易第一时间发现。
if grep -q "REPLACE_WITH" wrangler.jsonc; then
  echo "✘ wrangler.jsonc 里还有 REPLACE_WITH_* 占位 —— 先按 README「资源创建」填真实 id。" >&2
  exit 1
fi

# schema 幂等（CREATE IF NOT EXISTS），每次部署重放即可拿到最新表形状。
# -y 必须带：远端 execute 有一个「About to run N queries… Ok?」确认提示，
# 不带时脚本就在那里挂住等人敲 y（2026-08-26 首次部署实测踩中）。
npx wrangler d1 execute loongport-metrics -y --remote --file schema.sql

npx wrangler deploy

echo
echo "✔ 已部署。等 ~10 秒后验线上……"
sleep 10

# wrangler deploy 的输出里有 workers.dev 域名；显式传 BASE 验自定义域/其它环境。
"$HERE/verify.sh" "${CROWD_METRICS_BASE:-}"
