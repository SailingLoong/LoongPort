#!/usr/bin/env bash
#
# 线上验证：healthz、snapshot（含 CORS/缓存头）、OPTIONS 预检。
#
# 用法：./verify.sh [BASE_URL]
# 不传 BASE 时用正式自定义域（workers.dev 在国内网络不可达，见 wrangler.jsonc 注释）。

set -euo pipefail

BASE="${1:-https://metrics.loongport.dev}"
BASE="${BASE%/}"

fail() { echo "✘ $1" >&2; exit 1; }

echo "── GET $BASE/healthz"
curl -fsS "$BASE/healthz" | grep -q ok || fail "healthz 不通"

echo "── GET $BASE/v1/snapshot"
SNAP_HEADERS=$(curl -fsS -D - -o /tmp/crowd-snapshot.json "$BASE/v1/snapshot") || fail "snapshot 拉取失败"
echo "$SNAP_HEADERS" | grep -qi "access-control-allow-origin: \*" || fail "snapshot 缺 CORS 头"
echo "$SNAP_HEADERS" | grep -qi "cache-control: public" || fail "snapshot 缺缓存头"
node -e '
  const snap = JSON.parse(require("fs").readFileSync("/tmp/crowd-snapshot.json", "utf8"));
  if (snap.version !== 1 || typeof snap.generatedAt !== "number" || typeof snap.sites !== "object") {
    console.error("snapshot 形状不对: " + JSON.stringify(snap).slice(0, 200));
    process.exit(1);
  }
  console.log("snapshot ok：站点数 " + Object.keys(snap.sites).length + "，generatedAt " + new Date(snap.generatedAt * 1000).toISOString());
' || fail "snapshot JSON 形状不对"

echo "── OPTIONS $BASE/v1/snapshot"
PREFLIGHT=$(curl -fsS -X OPTIONS -D - -o /dev/null "$BASE/v1/snapshot") || fail "预检失败"
echo "$PREFLIGHT" | grep -qi "access-control-allow-methods:" || fail "预检缺 allow-methods"

echo
echo "✔ 线上验证全部通过（BASE=${BASE}）"
