/**
 * POST /v1/ping：客户端匿名使用统计（`src-tauri/src/relay/stats.rs`）的启动上报。
 *
 * 与 /v1/ingest 的本质区别：这份数据**只落 D1、永不公开** —— 没有快照、没有
 * CORS 读端点、不进任何 KV。维护者用 `wrangler d1 execute` / dashboard 直查
 * （安装量、版本分布、平台分布、在用站点）。公开的只有「接收」这个动作本身。
 *
 * 幂等：一行 = 一个安装。`installId` 是客户端在用户**同意告知那一刻**生成的
 * 随机 UUID v4（模块专属，与 device_id / crowd 的日轮换 source id 永不交叉）。
 * 每次上报 upsert：版本 / OS / 站点列表刷新为最新，first_seen 保留首次时间 ——
 * 去重后即「活跃安装数」，last_seen 即「最近活跃」。
 */

import { allowByIp } from "./ratelimit";
import { hourFloorUtc, isValidSite } from "./validate";

export interface PingEnv {
  DB: D1Database;
}

/** 一次上报载荷（字段集合与客户端 `relay::stats::Report` 逐一对齐）。 */
export interface StatsReport {
  installId: string;
  appVersion: string;
  os: string;
  siteHosts: string[];
  relayAccountCount: number;
}

export type ParseStatsResult =
  | { ok: true; report: StatsReport }
  | { ok: false; error: string };

/** ping 载荷远小于 ingest（无小时桶），给它一个独立的小上限。 */
const MAX_PING_BODY_BYTES = 16 * 1024;
/** 站点列表上限：防垃圾填充（真实用户挂不了这么多站）。 */
const MAX_SITE_HOSTS = 64;
/** 账号行数上限：同上，只防脏数据不防真实值。 */
const MAX_ACCOUNT_COUNT = 10_000;
/** 客户端 `generateUUID` 产出的带横线 UUID v4（32 hex + 4 连字符）。 */
const INSTALL_ID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
/** 版本串：semver 形状（含 prerelease/build 的字符集），不放任意文本。 */
const APP_VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,31}$/;
/** 与客户端 `current_os` 的取值集合一字不差（os 不带版本号是隐私评审定的）。 */
const OS_VALUES = new Set(["macos", "windows", "linux", "other"]);

function isSafeUint(n: unknown, max: number): n is number {
  return (
    typeof n === "number" &&
    Number.isInteger(n) &&
    n >= 0 &&
    n <= max
  );
}

/** 解析并整体校验一份上报。任何一处不合格即整体拒绝（不部分落库）。 */
export function parseStatsReport(json: unknown): ParseStatsResult {
  if (typeof json !== "object" || json === null) {
    return { ok: false, error: "payload must be an object" };
  }
  const obj = json as Record<string, unknown>;

  if (typeof obj.installId !== "string" || !INSTALL_ID_RE.test(obj.installId)) {
    return { ok: false, error: "bad installId" };
  }
  if (
    typeof obj.appVersion !== "string" ||
    !APP_VERSION_RE.test(obj.appVersion)
  ) {
    return { ok: false, error: "bad appVersion" };
  }
  if (typeof obj.os !== "string" || !OS_VALUES.has(obj.os)) {
    return { ok: false, error: "bad os" };
  }
  if (!Array.isArray(obj.siteHosts) || obj.siteHosts.length > MAX_SITE_HOSTS) {
    return { ok: false, error: `siteHosts must be an array (<= ${MAX_SITE_HOSTS})` };
  }
  // 形状复用 ingest 的站点校验（同一套注册域归一语义）；服务端再排一次序 +
  // 去重 —— 不信任客户端的排序（排序本身是防指纹要求，双保险无害）。
  const hosts: string[] = [];
  for (const h of obj.siteHosts) {
    if (typeof h !== "string" || !isValidSite(h)) {
      return { ok: false, error: `bad siteHosts entry: ${String(h)}` };
    }
    hosts.push(h);
  }
  hosts.sort();
  const siteHosts = [...new Set(hosts)];
  if (!isSafeUint(obj.relayAccountCount, MAX_ACCOUNT_COUNT)) {
    return { ok: false, error: "bad relayAccountCount" };
  }

  return {
    ok: true,
    report: {
      installId: obj.installId,
      appVersion: obj.appVersion,
      os: obj.os,
      siteHosts,
      relayAccountCount: obj.relayAccountCount,
    },
  };
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}

export async function handlePing(request: Request, env: PingEnv): Promise<Response> {
  const contentLength = Number(request.headers.get("content-length") ?? "0");
  if (contentLength > MAX_PING_BODY_BYTES) {
    return jsonResponse({ error: "payload too large" }, 413);
  }

  const text = await request.text();
  if (text.length > MAX_PING_BODY_BYTES) {
    return jsonResponse({ error: "payload too large" }, 413);
  }

  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    return jsonResponse({ error: "invalid json" }, 400);
  }

  const parsed = parseStatsReport(json);
  if (!parsed.ok) {
    return jsonResponse({ error: parsed.error }, 400);
  }

  const nowSec = Math.floor(Date.now() / 1000);
  const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
  if (!(await allowByIp(env, ip, hourFloorUtc(nowSec)))) {
    return jsonResponse({ error: "rate limited" }, 429);
  }

  await env.DB.prepare(
    `INSERT INTO stats_installs (
       install_id, app_version, os, site_hosts, relay_account_count,
       first_seen, last_seen
     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
     ON CONFLICT (install_id) DO UPDATE SET
       app_version = excluded.app_version,
       os = excluded.os,
       site_hosts = excluded.site_hosts,
       relay_account_count = excluded.relay_account_count,
       last_seen = excluded.last_seen`,
  )
    .bind(
      parsed.report.installId,
      parsed.report.appVersion,
      parsed.report.os,
      JSON.stringify(parsed.report.siteHosts),
      parsed.report.relayAccountCount,
      nowSec,
    )
    .run();

  return jsonResponse({ accepted: 1 }, 202);
}
