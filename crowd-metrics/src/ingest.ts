/**
 * POST /v1/ingest：校验 → 限流 → 落 D1。
 *
 * 幂等：桶行 PK (hour, site, app, source) + INSERT OR REPLACE，
 * 客户端对同一小时重发全量桶时覆盖而非累加。
 */

import { hourFloorUtc, MAX_BODY_BYTES, parseIngestPayload } from "./validate";

export interface Env {
  DB: D1Database;
  SNAPSHOT: KVNamespace;
}

/** 每来源 IP 每小时窗的最大上传次数。 */
const MAX_UPLOADS_PER_IP_HOUR = 20;

function jsonResponse(body: unknown, status: number, extraHeaders?: Headers): Response {
  const headers = new Headers({
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  if (extraHeaders) {
    for (const [k, v] of extraHeaders.entries()) headers.set(k, v);
  }
  return new Response(JSON.stringify(body), { status, headers });
}

async function ipHash(ip: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(ip),
  );
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** 限流：返回这次请求是否放行（计数含本次）。状态存 D1 而非 KV —— KV 免费档每天只有 1k 写。 */
async function allowByIp(env: Env, ip: string, nowSec: number): Promise<boolean> {
  const hash = await ipHash(ip);
  const hour = hourFloorUtc(nowSec);
  const result = await env.DB.prepare(
    `INSERT INTO upload_ip_hour (ip_hash, hour, count) VALUES (?1, ?2, 1)
     ON CONFLICT (ip_hash, hour) DO UPDATE SET count = count + 1
     RETURNING count`,
  )
    .bind(hash, hour)
    .first<{ count: number }>();
  return (result?.count ?? 0) <= MAX_UPLOADS_PER_IP_HOUR;
}

export async function handleIngest(request: Request, env: Env): Promise<Response> {
  const contentLength = Number(request.headers.get("content-length") ?? "0");
  if (contentLength > MAX_BODY_BYTES) {
    return jsonResponse({ error: "payload too large" }, 413);
  }

  const text = await request.text();
  if (text.length > MAX_BODY_BYTES) {
    return jsonResponse({ error: "payload too large" }, 413);
  }

  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    return jsonResponse({ error: "invalid json" }, 400);
  }

  const nowSec = Math.floor(Date.now() / 1000);
  const parsed = parseIngestPayload(json, nowSec);
  if (!parsed.ok) {
    return jsonResponse({ error: parsed.error }, 400);
  }

  const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
  if (!(await allowByIp(env, ip, nowSec))) {
    return jsonResponse({ error: "rate limited" }, 429);
  }

  const statements = parsed.payload.hours.map((b) =>
    env.DB.prepare(
      `INSERT OR REPLACE INTO bucket_raw (
         hour, site, app, source, samples, errors, ttft_bins, ttft_count,
         input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
         cost_usd_micros
       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)`,
    ).bind(
      b.hour,
      b.site,
      b.app,
      parsed.payload.sourceId,
      b.samples,
      b.errors,
      JSON.stringify(b.ttftBins),
      b.ttftCount,
      b.inputTokens,
      b.outputTokens,
      b.cacheReadTokens,
      b.cacheCreationTokens,
      b.costUsdMicros,
    ),
  );
  await env.DB.batch(statements);

  return jsonResponse({ accepted: parsed.payload.hours.length }, 202);
}
