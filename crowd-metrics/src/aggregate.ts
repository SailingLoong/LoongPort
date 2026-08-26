/**
 * 聚合核心（纯函数）：D1 原始桶行 → k-匿名后的公共快照。
 *
 * 三条纪律在这里落地：
 * 1. **k-匿名**：任何发布的聚合必须 ≥ MIN_SOURCES 个独立来源（按日轮换 id 粗计）。
 *    既防「站长自己刷自家数据」，也防稀疏桶反推单个用户。
 * 2. **极端源裁剪**：窗口内来源 ≥ TRIM_THRESHOLD 时，丢掉 TTFT 均值最小/最大
 *    各一个来源再合并 —— 单个病态客户端（或定向投毒）拉不偏分位数。
 * 3. **口径写死**：缓存命中率 = cache_read / (cache_read + cache_creation + input)；
 *    花费参考值 = 总花费 / 总 token × 1e6（模型混合会拉偏，展示侧必须标「参考」）。
 */

import { binMidpoint, quantileFromBins, TTFT_BIN_COUNT } from "./bins";
import { hourToEpochSec } from "./validate";
import type { HourSlot, SiteStats, Snapshot, WindowStats } from "./types";

/** k-匿名门槛：少于这么多独立来源的聚合不发布。 */
export const MIN_SOURCES = 3;
/** 触发极端源裁剪的最少来源数（≥5 才裁：保 3 个中位来源也过 k-匿名）。 */
export const TRIM_THRESHOLD = 5;

/** D1 bucket_raw 表的一行（ttft_bins 是 JSON 字符串）。 */
export interface RawRow {
  hour: string;
  site: string;
  app: string;
  source: string;
  samples: number;
  errors: number;
  ttft_bins: string;
  ttft_count: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  cost_usd_micros: number;
}

/** 解析后的桶（bins 已是数组，epoch 已算好）。 */
interface ParsedRow {
  hour: string;
  epoch: number;
  site: string;
  source: string;
  samples: number;
  errors: number;
  bins: number[];
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsdMicros: number;
}

function parseRow(row: RawRow): ParsedRow | null {
  let bins: number[];
  try {
    const parsed: unknown = JSON.parse(row.ttft_bins);
    if (
      !Array.isArray(parsed) ||
      parsed.length !== TTFT_BIN_COUNT ||
      !parsed.every((c) => typeof c === "number" && Number.isInteger(c) && c >= 0)
    ) {
      return null;
    }
    bins = parsed as number[];
  } catch {
    return null;
  }
  return {
    hour: row.hour,
    epoch: hourToEpochSec(row.hour),
    site: row.site,
    source: row.source,
    samples: row.samples,
    errors: row.errors,
    bins,
    inputTokens: row.input_tokens,
    outputTokens: row.output_tokens,
    cacheReadTokens: row.cache_read_tokens,
    cacheCreationTokens: row.cache_creation_tokens,
    costUsdMicros: row.cost_usd_micros,
  };
}

/** 一组桶的合并结果（可继续合并）。 */
interface Totals {
  samples: number;
  errors: number;
  bins: number[];
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsdMicros: number;
}

function emptyTotals(): Totals {
  return {
    samples: 0,
    errors: 0,
    bins: new Array<number>(TTFT_BIN_COUNT).fill(0),
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    costUsdMicros: 0,
  };
}

function addInto(t: Totals, r: ParsedRow): void {
  t.samples += r.samples;
  t.errors += r.errors;
  for (let i = 0; i < TTFT_BIN_COUNT; i++) t.bins[i] += r.bins[i];
  t.inputTokens += r.inputTokens;
  t.outputTokens += r.outputTokens;
  t.cacheReadTokens += r.cacheReadTokens;
  t.cacheCreationTokens += r.cacheCreationTokens;
  t.costUsdMicros += r.costUsdMicros;
}

function totalsToWindow(t: Totals, sources: number): WindowStats {
  const ttftTotal = t.bins.reduce((a, b) => a + b, 0);
  const cacheDenom =
    t.cacheReadTokens + t.cacheCreationTokens + t.inputTokens;
  const tokenTotal =
    t.inputTokens + t.outputTokens + t.cacheReadTokens + t.cacheCreationTokens;
  return {
    samples: t.samples,
    sources,
    ttftP50Ms: quantileFromBins(t.bins, 0.5),
    ttftP95Ms: quantileFromBins(t.bins, 0.95),
    errRate: t.samples > 0 ? t.errors / t.samples : null,
    cacheHitRate: cacheDenom > 0 ? t.cacheReadTokens / cacheDenom : null,
    // $/Mtok = (micros/1e6) / (tokens/1e6) = micros / tokens。
    costUsdPerMTok: tokenTotal > 0 ? t.costUsdMicros / tokenTotal : null,
  };
}

/**
 * 极端源裁剪：返回应参与合并的行。
 * 按「每源 TTFT 均值」排序，来源数 ≥ TRIM_THRESHOLD 时丢掉最小/最大各一个。
 * 没有 TTFT 样本的源不参与排序（它们不影响分位数，也没理由被裁/裁人）。
 */
function trimExtremeSources(rows: ParsedRow[]): ParsedRow[] {
  const bySource = new Map<string, ParsedRow[]>();
  for (const r of rows) {
    const list = bySource.get(r.source) ?? [];
    list.push(r);
    bySource.set(r.source, list);
  }

  const means: { source: string; mean: number | null }[] = [];
  for (const [source, list] of bySource) {
    let count = 0;
    let weighted = 0;
    for (const r of list) {
      for (let i = 0; i < TTFT_BIN_COUNT; i++) {
        count += r.bins[i];
        weighted += r.bins[i] * binMidpoint(i);
      }
    }
    means.push({ source, mean: count > 0 ? weighted / count : null });
  }

  const ranked = means
    .filter((m): m is { source: string; mean: number } => m.mean !== null)
    .sort((a, b) => a.mean - b.mean);
  const dropped = new Set<string>();
  if (ranked.length >= TRIM_THRESHOLD) {
    dropped.add(ranked[0].source);
    dropped.add(ranked[ranked.length - 1].source);
  }

  return rows.filter((r) => !dropped.has(r.source));
}

/** 一个站点的 w24 / w7 / 时段画像。两个窗口都不过 k-匿名的站点返回 null。 */
function buildSiteStats(
  rows: ParsedRow[],
  nowSec: number,
): SiteStats | null {
  const w24Rows = rows.filter((r) => r.epoch >= nowSec - 24 * 3600);
  const w7Rows = rows.filter((r) => r.epoch >= nowSec - 7 * 24 * 3600);

  const windowOrNull = (windowRows: ParsedRow[]): WindowStats | null => {
    const sources = new Set(windowRows.map((r) => r.source)).size;
    if (sources < MIN_SOURCES) return null;
    const kept = trimExtremeSources(windowRows);
    const totals = emptyTotals();
    for (const r of kept) addInto(totals, r);
    return totalsToWindow(totals, sources);
  };

  const w24 = windowOrNull(w24Rows);
  const w7 = windowOrNull(w7Rows);
  if (w24 === null && w7 === null) return null;

  // 时段画像：近 7 天按 UTC 小时槽聚合，每槽独立过 k-匿名。
  const hours: HourSlot[] = [];
  for (let slot = 0; slot < 24; slot++) {
    const slotRows = w7Rows.filter((r) => Number(r.hour.slice(11, 13)) === slot);
    const sources = new Set(slotRows.map((r) => r.source)).size;
    if (sources < MIN_SOURCES) {
      hours.push({ p50Ms: null, samples: 0 });
      continue;
    }
    const kept = trimExtremeSources(slotRows);
    const totals = emptyTotals();
    for (const r of kept) addInto(totals, r);
    hours.push({
      p50Ms: quantileFromBins(totals.bins, 0.5),
      samples: totals.samples,
    });
  }

  return { w24, w7, hours };
}

/** 由原始桶行构建整份快照。脏行（bins 解析失败）跳过，不让一行毒死整份快照。 */
export function buildSnapshot(rows: RawRow[], nowSec: number): Snapshot {
  const bySite = new Map<string, ParsedRow[]>();
  for (const row of rows) {
    const parsed = parseRow(row);
    if (parsed === null) continue;
    const list = bySite.get(parsed.site) ?? [];
    list.push(parsed);
    bySite.set(parsed.site, list);
  }

  const sites: Record<string, SiteStats> = {};
  // 站点按字典序产出，快照字节稳定（同数据 → 同输出，便于对账与缓存）。
  for (const site of [...bySite.keys()].sort()) {
    const stats = buildSiteStats(bySite.get(site)!, nowSec);
    if (stats !== null) sites[site] = stats;
  }

  return { version: 1, generatedAt: nowSec, sites };
}
