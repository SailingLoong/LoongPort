/**
 * loongport-metrics Worker 入口。
 *
 * - POST /v1/ingest   客户端上传小时聚合桶（见 ingest.ts）
 * - GET  /v1/snapshot 公共快照（CORS *、CDN 60s；KV 命中，冷启动兜底现算）
 * - scheduled（每 5 分钟） 重算快照写 KV + 清理 30 天前的原始桶 / 2 天前的限流计数
 */

import { buildSnapshot, type RawRow } from "./aggregate";
import { TTFT_BIN_EDGES_MS } from "./bins";
import { handleIngest, type Env } from "./ingest";
import { hourFloorUtc } from "./validate";
import type { Snapshot } from "./types";

/** 原始桶保留期。 */
const RAW_RETENTION_SECS = 30 * 86400;
/** 限流计数保留期（覆盖跨小时窗查询即可）。 */
const RATE_LIMIT_RETENTION_SECS = 2 * 86400;
/** KV 快照键。 */
const SNAPSHOT_KEY = "snapshot:v1";

const CORS_HEADERS: Record<string, string> = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, OPTIONS",
  "access-control-max-age": "86400",
};

const SNAPSHOT_CACHE_CONTROL = "public, max-age=60, stale-while-revalidate=300";

async function queryRawRows(env: Env, nowSec: number): Promise<RawRow[]> {
  // 7 天窗口 + 1 小时余量（整点对齐会把边界小时整桶切进切出）。
  const cutoff = hourFloorUtc(nowSec - 7 * 86400 - 3600);
  const { results } = await env.DB.prepare(
    `SELECT hour, site, app, source, asn, ua_trusted, samples, errors,
            ttft_bins, ttft_count, input_tokens, output_tokens,
            cache_read_tokens, cache_creation_tokens, cost_usd_micros
     FROM bucket_raw WHERE hour >= ?1 ORDER BY hour`,
  )
    .bind(cutoff)
    .all<RawRow>();
  return results ?? [];
}

async function recomputeSnapshot(env: Env, nowSec: number): Promise<Snapshot> {
  const rows = await queryRawRows(env, nowSec);
  const snapshot = buildSnapshot(rows, nowSec);
  snapshot.ttftBinEdges = [...TTFT_BIN_EDGES_MS];
  await env.SNAPSHOT.put(SNAPSHOT_KEY, JSON.stringify(snapshot));
  return snapshot;
}

async function handleSnapshot(env: Env, allowCompute: boolean): Promise<Response> {
  const cached = await env.SNAPSHOT.get(SNAPSHOT_KEY);
  if (cached !== null) {
    return new Response(cached, {
      status: 200,
      headers: {
        "content-type": "application/json; charset=utf-8",
        "cache-control": SNAPSHOT_CACHE_CONTROL,
        ...CORS_HEADERS,
      },
    });
  }
  if (!allowCompute) {
    return jsonResponse({ error: "snapshot not ready" }, 503);
  }
  // 冷启动兜底：部署后第一次 cron 之前也能出数。
  const nowSec = Math.floor(Date.now() / 1000);
  const snapshot = await recomputeSnapshot(env, nowSec);
  return new Response(JSON.stringify(snapshot), {
    status: 200,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": SNAPSHOT_CACHE_CONTROL,
      ...CORS_HEADERS,
    },
  });
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}

export default {
  async fetch(
    request: Request,
    env: Env,
    _ctx: ExecutionContext,
  ): Promise<Response> {
    const { pathname } = new URL(request.url);

    if (request.method === "POST" && pathname === "/v1/ingest") {
      return handleIngest(request, env);
    }
    if (request.method === "GET" && pathname === "/v1/snapshot") {
      return handleSnapshot(env, true);
    }
    if (request.method === "OPTIONS" && pathname === "/v1/snapshot") {
      return new Response(null, { status: 204, headers: CORS_HEADERS });
    }
    if (request.method === "GET" && pathname === "/healthz") {
      return new Response("ok\n", {
        status: 200,
        headers: { "cache-control": "no-store" },
      });
    }
    return jsonResponse({ error: "not found" }, 404);
  },

  async scheduled(
    _event: ScheduledController,
    env: Env,
    ctx: ExecutionContext,
  ): Promise<void> {
    const nowSec = Math.floor(Date.now() / 1000);

    const recompute = recomputeSnapshot(env, nowSec);
    const rawCutoff = hourFloorUtc(nowSec - RAW_RETENTION_SECS);
    const rlCutoff = hourFloorUtc(nowSec - RATE_LIMIT_RETENTION_SECS);
    const cleanup = env.DB.batch([
      env.DB.prepare("DELETE FROM bucket_raw WHERE hour < ?1").bind(rawCutoff),
      env.DB.prepare("DELETE FROM upload_ip_hour WHERE hour < ?1").bind(rlCutoff),
    ]);

    await Promise.all([recompute, cleanup]);
    void ctx;
  },
};

export type { Env };
