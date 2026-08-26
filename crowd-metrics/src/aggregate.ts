/**
 * 聚合核心（纯函数）：D1 原始桶行 → k-匿名 + 反作弊后的公共快照。
 *
 * 防线分四层（2026-08-26 加固定稿）：
 *
 * 1. **k-匿名**：任何发布的聚合必须 ≥ MIN_SOURCES 个**受信**（LoongPort 客户端 UA）
 *    独立来源。防稀疏桶反推单个用户。
 * 2. **网络多样性门槛（L1）**：受信来源还必须横跨 ≥ MIN_ASN 个 ASN。ASN 由
 *    Cloudflare 边缘给出、客户端伪造不了 —— 「生成 3 个随机 id」不再够，
 *    刷量需要 ≥2 个不同运营商出口。门槛选 2 不是 3：国内用户高度集中在
 *    电信/联通/移动三大 ASN，要求 3 个会误杀「三个真实用户恰好同一运营商」。
 * 3. **cohort 异常剔除（L3）**：同一 ASN 下、**只出现在这一家站**、联合比
 *    同站其余来源快 ≥COHORT_FACTOR 倍的一组来源 = 典型刷量指纹（真实用户
 *    会跨站使用，LoongPort 的产品形态决定了真源几乎必然出现在多家站的数据里）。
 *    命中即整组剔除。因子取 3：模型混合会合法拉开用户间差异（快模型 vs 慢模型），
 *    阈值收紧会误杀。
 * 4. **极值裁剪**：来源 ≥ TRIM_THRESHOLD 时丢掉 TTFT 均值最小/最大各一个 ——
 *    防单个病态客户端，与 3 互补（3 防协调一致的假源组，这里防单点离群）。
 *
 * 口径：缓存命中率 = cache_read / (cache_read + cache_creation + input)；
 * 花费参考值 = 微美元 / 总 token（$/Mtok，模型混合会拉偏，展示侧标「参考」）。
 */

import { binMidpoint, quantileFromBins, TTFT_BIN_COUNT } from "./bins";
import { hourToEpochSec } from "./validate";
import type { HourSlot, SiteStats, Snapshot, WindowStats } from "./types";

/** k-匿名门槛：少于这么多受信独立来源的聚合不发布。 */
export const MIN_SOURCES = 3;
/** 网络多样性门槛：受信来源横跨的最少 ASN 数。 */
export const MIN_ASN = 2;
/** 触发极值源裁剪的最少来源数（≥5 才裁：保 3 个来源也过 k-匿名）。 */
export const TRIM_THRESHOLD = 5;
/** cohort 剔除的联合快慢因子（快于同行 3 倍）。 */
export const COHORT_FACTOR = 3;
/** 成组判定的最少成员数。 */
const COHORT_MIN_MEMBERS = 2;

/** D1 bucket_raw 表的一行（ttft_bins 是 JSON 字符串）。 */
export interface RawRow {
  hour: string;
  site: string;
  app: string;
  source: string;
  asn: number;
  ua_trusted: number;
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
  asn: number;
  uaTrusted: boolean;
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
    asn: row.asn,
    uaTrusted: row.ua_trusted === 1,
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
  const cacheDenom = t.cacheReadTokens + t.cacheCreationTokens + t.inputTokens;
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
    ttftBins: [...t.bins],
  };
}

/** 中位数（偶数个取中间两值均值）。 */
function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[mid]
    : (sorted[mid - 1] + sorted[mid]) / 2;
}

/** 一组行的 TTFT 加权均值（桶中点近似）。 */
function ttftMean(rows: ParsedRow[]): number | null {
  let count = 0;
  let weighted = 0;
  for (const r of rows) {
    for (let i = 0; i < TTFT_BIN_COUNT; i++) {
      count += r.bins[i];
      weighted += r.bins[i] * binMidpoint(i);
    }
  }
  return count > 0 ? weighted / count : null;
}

/**
 * cohort 剔除（L3）：刷量指纹 = 同一 ASN 下 ≥2 个**只出现在这家站**的来源，
 * 联合 TTFT 均值比同站其余来源快 ≥COHORT_FACTOR 倍。
 *
 * 「只此一家」+「整组异常快」两个条件同时命中才是指纹 —— 单独哪个都不足以
 * 定罪（单用户快网络合法；同站多来源但跨站出现也合法）。真源几乎必然跨站：
 * LoongPort 的产品形态就是多站切换。
 */
function dropSybilCohorts(
  windowRows: ParsedRow[],
  exclusiveSources: Set<string>,
): ParsedRow[] {
  const excluded = new Set<string>();

  // 按来源分桶后，再按（来源 × ASN）把「只此一家」的来源挂到各 ASN 名下。
  // 来源横跨多个 ASN（真实用户换网）时会在多个组里被考察 —— 无害。
  const bySource = new Map<string, ParsedRow[]>();
  for (const r of windowRows) {
    const list = bySource.get(r.source) ?? [];
    list.push(r);
    bySource.set(r.source, list);
  }
  const byAsn = new Map<number, string[]>();
  for (const [source, group] of bySource) {
    if (!exclusiveSources.has(source)) continue;
    for (const asn of new Set(group.map((r) => r.asn))) {
      const list = byAsn.get(asn) ?? [];
      list.push(source);
      byAsn.set(asn, list);
    }
  }

  const sourceMean = (source: string) => ttftMean(bySource.get(source)!);
  for (const members of byAsn.values()) {
    if (members.length < COHORT_MIN_MEMBERS) continue;
    const memberSet = new Set(members);
    // 双重中位数（先每源均值、再组间取中位）：单个病态源灌不大 rest ——
    // 均值版会被一个 12s 的离群源顶高「同行水平」，把正常的同 ASN 组误判成异常快。
    const cohortMedian = median(
      members.map(sourceMean).filter((v): v is number => v != null),
    );
    const restMedian = median(
      [...bySource.keys()]
        .filter((src) => !memberSet.has(src))
        .map(sourceMean)
        .filter((v): v is number => v != null),
    );
    if (cohortMedian == null || restMedian == null || restMedian <= 0) continue;
    if (cohortMedian * COHORT_FACTOR <= restMedian) {
      for (const m of members) excluded.add(m);
    }
  }

  return excluded.size === 0
    ? windowRows
    : windowRows.filter((r) => !excluded.has(r.source));
}

/** 极值源裁剪：按「每源 TTFT 均值」排序，来源数 ≥ TRIM_THRESHOLD 时丢最小/最大各一。 */
function trimExtremeSources(rows: ParsedRow[]): ParsedRow[] {
  const bySource = new Map<string, ParsedRow[]>();
  for (const r of rows) {
    const list = bySource.get(r.source) ?? [];
    list.push(r);
    bySource.set(r.source, list);
  }

  const ranked = [...bySource.entries()]
    .map(([source, group]) => ({ source, mean: ttftMean(group) }))
    .filter((x): x is { source: string; mean: number } => x.mean != null)
    .sort((a, b) => a.mean - b.mean);

  const dropped = new Set<string>();
  if (ranked.length >= TRIM_THRESHOLD) {
    dropped.add(ranked[0].source);
    dropped.add(ranked[ranked.length - 1].source);
  }

  return rows.filter((r) => !dropped.has(r.source));
}

/**
 * 窗口统计：cohort 剔除 → 门禁（受信来源 ≥MIN_SOURCES 且横跨 ≥MIN_ASN 个 ASN）
 * → 极值裁剪 → 合并。不过门槛返回 null。
 *
 * 展示的 `sources` 是 cohort 剔除后的**全部**来源（含未受信）—— 门禁只决定
 * 发不发，数字不虚饰。
 */
function windowOrNull(
  windowRows: ParsedRow[],
  exclusiveSources: Set<string>,
): WindowStats | null {
  const afterCohort = dropSybilCohorts(windowRows, exclusiveSources);

  const trusted = afterCohort.filter((r) => r.uaTrusted);
  const trustedSources = new Set(trusted.map((r) => r.source)).size;
  const trustedAsns = new Set(trusted.map((r) => r.asn)).size;
  if (trustedSources < MIN_SOURCES || trustedAsns < MIN_ASN) return null;

  const kept = trimExtremeSources(afterCohort);
  const totals = emptyTotals();
  for (const r of kept) addInto(totals, r);
  return totalsToWindow(totals, new Set(afterCohort.map((r) => r.source)).size);
}

/** 24 个 UTC 时段槽（近 7 天聚合）。k-匿名未达标的槽为 {p50Ms: null, samples: 0}。
 *  槽级不加 ASN 门槛：槽天然稀疏（24×7 的切面），加了会几乎全灭；展示侧
 *  槽只是形态参考，主指标在 w24/w7 窗口上，那里有完整门禁。 */
function buildHourSlots(w7Rows: ParsedRow[]): HourSlot[] {
  const slots: HourSlot[] = [];
  for (let slot = 0; slot < 24; slot++) {
    const slotRows = w7Rows.filter((r) => Number(r.hour.slice(11, 13)) === slot);
    const trusted = slotRows.filter((r) => r.uaTrusted);
    const sources = new Set(trusted.map((r) => r.source)).size;
    if (sources < MIN_SOURCES) {
      slots.push({ p50Ms: null, samples: 0 });
      continue;
    }
    const kept = trimExtremeSources(slotRows);
    const totals = emptyTotals();
    for (const r of kept) addInto(totals, r);
    slots.push({
      p50Ms: quantileFromBins(totals.bins, 0.5),
      samples: totals.samples,
    });
  }
  return slots;
}

/** 一个站点的 w24 / w7 / 时段画像。两个窗口都不过门槛的站点返回 null。 */
function buildSiteStats(
  rows: ParsedRow[],
  nowSec: number,
  exclusiveSources: Set<string>,
): SiteStats | null {
  const w24Rows = rows.filter((r) => r.epoch >= nowSec - 24 * 3600);
  const w7Rows = rows.filter((r) => r.epoch >= nowSec - 7 * 24 * 3600);

  const w24 = windowOrNull(w24Rows, exclusiveSources);
  const w7 = windowOrNull(w7Rows, exclusiveSources);
  if (w24 === null && w7 === null) return null;

  return { w24, w7, hours: buildHourSlots(w7Rows) };
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

  // 跨站来源集合：真源几乎必然出现在多家站（产品形态决定）—— 这是 cohort
  // 判定里「只此一家」那半个条件的唯一事实源。
  const sitesBySource = new Map<string, Set<string>>();
  for (const [site, list] of bySite) {
    for (const r of list) {
      const set = sitesBySource.get(r.source) ?? new Set<string>();
      set.add(site);
      sitesBySource.set(r.source, set);
    }
  }
  const exclusiveSources = new Set<string>();
  for (const [source, sites] of sitesBySource) {
    if (sites.size === 1) exclusiveSources.add(source);
  }

  const sites: Record<string, SiteStats> = {};
  // 站点按字典序产出，快照字节稳定（同数据 → 同输出，便于对账与缓存）。
  for (const site of [...bySite.keys()].sort()) {
    const stats = buildSiteStats(bySite.get(site)!, nowSec, exclusiveSources);
    if (stats !== null) sites[site] = stats;
  }

  return { version: 1, generatedAt: nowSec, sites };
}
