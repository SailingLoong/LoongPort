/**
 * POST /v1/ingest：校验 → 限流 → 落 D1。
 *
 * 幂等：桶行 PK (hour, site, app, source) + INSERT OR REPLACE，
 * 客户端对同一小时重发全量桶时覆盖而非累加。
 */

import { hourFloorUtc, MAX_BODY_BYTES, parseIngestPayload } from "./validate";
import { allowByIp } from "./ratelimit";

export interface Env {
  DB: D1Database;
  SNAPSHOT: KVNamespace;
}

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
  if (!(await allowByIp(env, ip, hourFloorUtc(nowSec)))) {
    return jsonResponse({ error: "rate limited" }, 429);
  }

  // 反作弊维度（都取自请求本身，载荷字段碰不到）：
  // - asn 来自 Cloudflare 边缘（request.cf.asn），客户端伪造不了；
  // - ua_trusted 是最懒脚本过滤器 —— 开源可查、可伪造，只用于 k-匿名计数，不是防御本体。
  const asn = request.cf?.asn ?? 0;
  const uaTrusted = (request.headers.get("user-agent") ?? "").startsWith(
    "LoongPort/",
  )
    ? 1
    : 0;

  const statements = parsed.payload.hours.flatMap((b) => {
    const siteRow = env.DB.prepare(
      `INSERT OR REPLACE INTO bucket_raw (
         hour, site, app, source, asn, ua_trusted,
         samples, errors, ttft_bins, ttft_count,
         input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
         cost_usd_micros, breaker_trips
       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)`,
    ).bind(
      b.hour,
      b.site,
      b.app,
      parsed.payload.sourceId,
      asn,
      uaTrusted,
      b.samples,
      b.errors,
      JSON.stringify(b.ttftBins),
      b.ttftCount,
      b.inputTokens,
      b.outputTokens,
      b.cacheReadTokens,
      b.cacheCreationTokens,
      b.costUsdMicros,
      b.breakerTrips ?? 0,
    );
    // P4：模型子桶落独立表（v1 载荷 models 为空 → 只有站点行）。
    const modelRows = (b.models ?? []).map((m) =>
      env.DB.prepare(
        `INSERT OR REPLACE INTO bucket_model_raw (
           hour, site, app, model, source, asn, ua_trusted,
           samples, errors, ttft_bins, tps_bins,
           input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
           cost_usd_micros, anomalies
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)`,
      ).bind(
        b.hour,
        b.site,
        b.app,
        m.model,
        parsed.payload.sourceId,
        asn,
        uaTrusted,
        m.samples,
        m.errors,
        JSON.stringify(m.ttftBins),
        JSON.stringify(m.tpsBins),
        m.inputTokens,
        m.outputTokens,
        m.cacheReadTokens,
        m.cacheCreationTokens,
        m.costUsdMicros,
        m.anomalies ?? 0,
      ),
    );
    return [siteRow, ...modelRows];
  });
  await env.DB.batch(statements);

  return jsonResponse({ accepted: parsed.payload.hours.length }, 202);
}
