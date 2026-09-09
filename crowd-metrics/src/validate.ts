/**
 * ingest 载荷校验（纯函数，无 IO）。
 *
 * 原则：**白名单形状 + 上限**，不是黑名单关键字 —— 公开端点，任何字节都可能是恶意的。
 * 所有数值必须是安全非负整数且带合理上限；小时串必须日历合法且不未来、不太老。
 */

import { TPS_BIN_COUNT, TTFT_BIN_COUNT } from "./bins";
import type { IngestPayload, ModelBucketPayload } from "./types";

export const MAX_BODY_BYTES = 256 * 1024;
export const MAX_HOURS_PER_UPLOAD = 200;
/** P4b：单桶跳闸计数上限（一小时一万次跳闸必然是脏数据）。 */
const MAX_TRIPS_PER_BUCKET = 10_000;
/** P4：单小时桶的模型子桶数上限（长尾模型归 UI 侧聚合，这里只防垃圾填充）。 */
const MAX_MODELS_PER_BUCKET = 64;
/** 模型名形状：公开目录名 —— 字母数字与常规分隔符，拒绝任意可注入文本。 */
const MODEL_RE = /^[A-Za-z0-9][A-Za-z0-9._\/:+-]{0,119}$/;
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

  if (obj.version !== 1 && obj.version !== 2) {
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
    // errSamples 可缺省（旧客户端）；给了就必须 ≤ samples —— 它是错误率的
    // 分母，比总样本数还大说明口径坏了。
    if (b.errSamples !== undefined && !isSafeUint(b.errSamples, samples)) {
      return { ok: false, error: "bad errSamples" };
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

    // P4（version 2）：模型子桶。v1 载荷没有 models 键 —— 跳过。
    let models: ModelBucketPayload[] | undefined;
    if (obj.version === 2) {
      if (!Array.isArray(b.models)) {
        return { ok: false, error: "v2 hour bucket must carry models array" };
      }
      if (b.models.length > MAX_MODELS_PER_BUCKET) {
        return { ok: false, error: `too many model buckets (>${MAX_MODELS_PER_BUCKET})` };
      }
      const modelSeen = new Set<string>();
      models = [];
      for (const rawModel of b.models) {
        if (typeof rawModel !== "object" || rawModel === null) {
          return { ok: false, error: "model bucket must be an object" };
        }
        const m = rawModel as Record<string, unknown>;
        if (typeof m.model !== "string" || !MODEL_RE.test(m.model)) {
          return { ok: false, error: `bad model: ${String(m.model)}` };
        }
        if (modelSeen.has(m.model)) {
          return { ok: false, error: "duplicate model bucket" };
        }
        modelSeen.add(m.model);
        const mSamples = m.samples;
        const mErrors = m.errors;
        if (!isSafeUint(mSamples, samples)) {
          return { ok: false, error: "bad model samples (must be <= bucket samples)" };
        }
        if (!isSafeUint(mErrors, mSamples)) {
          return { ok: false, error: "bad model errors" };
        }
        if (m.errSamples !== undefined && !isSafeUint(m.errSamples, mSamples)) {
          return { ok: false, error: "bad model errSamples" };
        }
        if (
          !isSafeUint(m.inputTokens, MAX_COUNT) ||
          !isSafeUint(m.outputTokens, MAX_COUNT) ||
          !isSafeUint(m.cacheReadTokens, MAX_COUNT) ||
          !isSafeUint(m.cacheCreationTokens, MAX_COUNT) ||
          !isSafeUint(m.costUsdMicros, MAX_COUNT)
        ) {
          return { ok: false, error: "bad model token/cost counters" };
        }
        if (
          !Array.isArray(m.ttftBins) ||
          m.ttftBins.length !== TTFT_BIN_COUNT ||
          !m.ttftBins.every((c) => isSafeUint(c, mSamples))
        ) {
          return { ok: false, error: "bad model ttftBins" };
        }
        if (
          !Array.isArray(m.tpsBins) ||
          m.tpsBins.length !== TPS_BIN_COUNT ||
          !m.tpsBins.every((c) => isSafeUint(c, mSamples))
        ) {
          return { ok: false, error: "bad model tpsBins" };
        }
        // P5：模型异常计数（可选；恒 ≤ 该模型样本数 —— 异常响应必然是被
        // 计数的请求之一，超限即乱填）。
        const mAnomalies = m.anomalies ?? 0;
        if (!isSafeUint(mAnomalies, mSamples)) {
          return { ok: false, error: "bad model anomalies (must be <= model samples)" };
        }
        models.push({
          model: m.model,
          samples: mSamples,
          errors: mErrors,
          errSamples: m.errSamples,
          ttftBins: m.ttftBins as number[],
          tpsBins: m.tpsBins as number[],
          inputTokens: m.inputTokens as number,
          outputTokens: m.outputTokens as number,
          cacheReadTokens: m.cacheReadTokens as number,
          cacheCreationTokens: m.cacheCreationTokens as number,
          costUsdMicros: m.costUsdMicros as number,
          anomalies: mAnomalies,
        });
      }
    }

    // P4b：跳闸计数（可选；上限远宽于合理值 —— 防垃圾填充不防真实值）。
    let breakerTrips: number | undefined;
    if (b.breakerTrips !== undefined) {
      if (!isSafeUint(b.breakerTrips, MAX_TRIPS_PER_BUCKET)) {
        return { ok: false, error: "bad breakerTrips" };
      }
      breakerTrips = b.breakerTrips;
    }

    hours.push({
      hour: b.hour,
      site: b.site as string,
      app: b.app,
      samples,
      errors,
      errSamples: b.errSamples,
      breakerTrips,
      ttftBins: b.ttftBins as number[],
      ttftCount,
      inputTokens: b.inputTokens as number,
      outputTokens: b.outputTokens as number,
      cacheReadTokens: b.cacheReadTokens as number,
      cacheCreationTokens: b.cacheCreationTokens as number,
      costUsdMicros: b.costUsdMicros as number,
      models,
    });
  }

  return { ok: true, payload: { version: obj.version, sourceId: obj.sourceId, hours } };
}
