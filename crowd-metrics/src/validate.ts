/**
 * ingest 载荷校验（纯函数，无 IO）。
 *
 * 原则：**白名单形状 + 上限**，不是黑名单关键字 —— 公开端点，任何字节都可能是恶意的。
 * 所有数值必须是安全非负整数且带合理上限；小时串必须日历合法且不未来、不太老。
 */

import { TTFT_BIN_COUNT } from "./bins";
import type { IngestPayload } from "./types";

export const MAX_BODY_BYTES = 64 * 1024;
export const MAX_HOURS_PER_UPLOAD = 200;
/** 单桶请求数上限：一小时十万次请求必然是脏数据。 */
const MAX_SAMPLES = 100_000;
/** token / 花费字段的数量级上限（防溢出与垃圾填充）。 */
const MAX_COUNT = 1e12;
/** 接受的小时窗口：不接受未来（留 1h 时钟偏差余量）与 35 天前（保留期 30 天 + 余量）。 */
const FUTURE_SLACK_SECS = 3600;
const MAX_AGE_SECS = 35 * 86400;

const HOUR_RE = /^\d{4}-\d{2}-\d{2}T\d{2}Z$/;
const SOURCE_RE = /^[0-9a-f]{32}$/;
const APP_RE = /^[a-z][a-z0-9-]{0,15}$/;
/**
 * 归一化 host 的形状：小写标签 + 点分 + 字母结尾 TLD。
 * 拒绝 scheme/端口/大写/IP 字面量 —— 内网地址（192.168.x.x）对其他用户毫无意义，
 * 上传它只泄漏「这个用户在内网自建了中转」这一个事实。
 */
const HOST_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*\.[a-z]{2,24}$/;

export type ParseResult =
  | { ok: true; payload: IngestPayload }
  | { ok: false; error: string };

function isSafeUint(n: unknown, max: number): n is number {
  return (
    typeof n === "number" &&
    Number.isInteger(n) &&
    n >= 0 &&
    n <= max
  );
}

/** '2026-08-26T07Z' → epoch 秒。格式已由调用方保证。 */
export function hourToEpochSec(hour: string): number {
  const y = Number(hour.slice(0, 4));
  const mo = Number(hour.slice(5, 7));
  const d = Number(hour.slice(8, 10));
  const h = Number(hour.slice(11, 13));
  return Math.floor(Date.UTC(y, mo - 1, d, h) / 1000);
}

/** epoch 秒 → '2026-08-26T07Z'。 */
export function hourFloorUtc(epochSec: number): string {
  const d = new Date(epochSec * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}` +
    `-${p(d.getUTCDate())}T${p(d.getUTCHours())}Z`
  );
}

function isValidHourString(hour: string, nowSec: number): boolean {
  if (!HOUR_RE.test(hour)) return false;
  const mo = Number(hour.slice(5, 7));
  const d = Number(hour.slice(8, 10));
  const h = Number(hour.slice(11, 13));
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23) return false;
  // 日历合法性：2 月 31 日这类构造会向前进位，回读后不再相等。
  if (hourFloorUtc(hourToEpochSec(hour)) !== hour) return false;
  const epoch = hourToEpochSec(hour);
  if (epoch > nowSec + FUTURE_SLACK_SECS) return false;
  if (epoch < nowSec - MAX_AGE_SECS) return false;
  return true;
}

export function isValidSite(site: string): boolean {
  if (typeof site !== "string" || site.length > 253) return false;
  if (site.startsWith("www.")) return false; // 归一化应已去 www —— 防同站双身份
  return HOST_RE.test(site);
}

/** 解析并整体校验一个 ingest 载荷。任何一处不合格即整体拒绝（不部分落库）。 */
export function parseIngestPayload(
  json: unknown,
  nowSec: number,
): ParseResult {
  if (typeof json !== "object" || json === null) {
    return { ok: false, error: "payload must be an object" };
  }
  const obj = json as Record<string, unknown>;

  if (obj.version !== 1) {
    return { ok: false, error: "unsupported version" };
  }
  if (typeof obj.sourceId !== "string" || !SOURCE_RE.test(obj.sourceId)) {
    return { ok: false, error: "bad sourceId" };
  }
  if (!Array.isArray(obj.hours) || obj.hours.length === 0) {
    return { ok: false, error: "hours must be a non-empty array" };
  }
  if (obj.hours.length > MAX_HOURS_PER_UPLOAD) {
    return { ok: false, error: `too many hour buckets (>${MAX_HOURS_PER_UPLOAD})` };
  }

  const hours: IngestPayload["hours"] = [];
  const seen = new Set<string>();
  for (const raw of obj.hours) {
    if (typeof raw !== "object" || raw === null) {
      return { ok: false, error: "hour bucket must be an object" };
    }
    const b = raw as Record<string, unknown>;

    if (typeof b.hour !== "string" || !isValidHourString(b.hour, nowSec)) {
      return { ok: false, error: `bad hour: ${String(b.hour)}` };
    }
    if (typeof b.site !== "string" || !isValidSite(b.site)) {
      return { ok: false, error: `bad site: ${String(b.site)}` };
    }
    if (typeof b.app !== "string" || !APP_RE.test(b.app)) {
      return { ok: false, error: `bad app: ${String(b.app)}` };
    }
    const key = `${b.hour}\u0000${b.site}\u0000${b.app}`;
    if (seen.has(key)) {
      return { ok: false, error: "duplicate hour bucket" };
    }
    seen.add(key);

    const samples = b.samples;
    const errors = b.errors;
    const ttftCount = b.ttftCount;
    if (!isSafeUint(samples, MAX_SAMPLES)) {
      return { ok: false, error: "bad samples" };
    }
    if (!isSafeUint(errors, samples)) {
      return { ok: false, error: "bad errors" };
    }
    if (!isSafeUint(ttftCount, samples)) {
      return { ok: false, error: "bad ttftCount" };
    }
    if (
      !isSafeUint(b.inputTokens, MAX_COUNT) ||
      !isSafeUint(b.outputTokens, MAX_COUNT) ||
      !isSafeUint(b.cacheReadTokens, MAX_COUNT) ||
      !isSafeUint(b.cacheCreationTokens, MAX_COUNT) ||
      !isSafeUint(b.costUsdMicros, MAX_COUNT)
    ) {
      return { ok: false, error: "bad token/cost counters" };
    }
    if (
      !Array.isArray(b.ttftBins) ||
      b.ttftBins.length !== TTFT_BIN_COUNT ||
      !b.ttftBins.every((c) => isSafeUint(c, ttftCount))
    ) {
      return { ok: false, error: "bad ttftBins" };
    }
    const binsSum = (b.ttftBins as number[]).reduce((a, c) => a + c, 0);
    if (binsSum !== ttftCount) {
      return { ok: false, error: "ttftBins sum != ttftCount" };
    }

    hours.push({
      hour: b.hour,
      site: b.site as string,
      app: b.app,
      samples,
      errors,
      ttftBins: b.ttftBins as number[],
      ttftCount,
      inputTokens: b.inputTokens as number,
      outputTokens: b.outputTokens as number,
      cacheReadTokens: b.cacheReadTokens as number,
      cacheCreationTokens: b.cacheCreationTokens as number,
      costUsdMicros: b.costUsdMicros as number,
    });
  }

  return { ok: true, payload: { version: 1, sourceId: obj.sourceId, hours } };
}
