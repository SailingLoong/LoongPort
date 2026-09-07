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

import { binMidpoint, quantileFromBins, TPS_BIN_COUNT, tpsQuantileFromBins, TTFT_BIN_COUNT } from "./bins";
import { hourToEpochSec } from "./validate";
import type {
  HourSlot,
  ModelWindowStats,
  SiteTrendLite,
  SiteStats,
  SiteTrend,
  Snapshot,
  TrendBucket,
  TrendPayload,
  WindowStats,
} from "./types";

/**
 * k-匿名门槛：少于这么多受信独立来源的聚合不发布。
 * 网络多样性门槛：受信来源横跨的最少 ASN 数。
 *
 * ⚠️ **2026-09-06 起临时放开为 1/1**（用户拍板）：参与上传的用户还很少，
 * 3 源 × 2 ASN 让实测页几乎必然空白。这是一次有意识的隐私让步——单源
 * 站点的公开数据实质上就是该用户一人的使用画像。cohort（需 ≥2 源）与
 * 极值裁剪（需 ≥5 源）在单源下天然不触发，防线代码保持原样。
 *
 * **恢复条件**：快照里稳定出现 ≥3 独立源的站点（或日均独立源 ≥5）时
 * 改回 `MIN_SOURCES = 3 / MIN_ASN = 2`，并同步恢复：
 * aggregate.test.ts 的钉值测试与「同一 ASN」测试、README 的门槛描述、
 * 主仓 crowd/mod.rs 口径节的 k-匿描述。
 */
export const MIN_SOURCES = 1;
export const MIN_ASN = 1;
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
  const exclusiveSources = exclusiveSourcesOf(bySite);

  const sites: Record<string, SiteStats> = {};
  // 站点按字典序产出，快照字节稳定（同数据 → 同输出，便于对账与缓存）。
  for (const site of [...bySite.keys()].sort()) {
    const stats = buildSiteStats(bySite.get(site)!, nowSec, exclusiveSources);
    if (stats !== null) sites[site] = stats;
  }

  return { version: 1, generatedAt: nowSec, sites };
}

/** 「只出现在一家站」的来源集合：cohort 判定里「只此一家」半个条件的唯一
 *  事实源。快照与趋势两处共用（各算一份会漂移）。 */
function exclusiveSourcesOf(bySite: Map<string, ParsedRow[]>): Set<string> {
  const sitesBySource = new Map<string, Set<string>>();
  for (const [site, list] of bySite) {
    for (const r of list) {
      const set = sitesBySource.get(r.source) ?? new Set<string>();
      set.add(site);
      sitesBySource.set(r.source, set);
    }
  }
  const exclusive = new Set<string>();
  for (const [source, sites] of sitesBySource) {
    if (sites.size === 1) exclusive.add(source);
  }
  return exclusive;
}

/** 趋势档位：跨度 + 输出桶粒度。粒度选点让每档点位数落在 24~60 之间
 *  （折线可读、悬停可命中），且都是小时的整数倍（原始桶按小时存）。 */
const TREND_RANGES = [
  { key: "24h" as const, spanSecs: 24 * 3600, bucketSecs: 3600 },
  { key: "7d" as const, spanSecs: 7 * 86400, bucketSecs: 3 * 3600 },
  { key: "30d" as const, spanSecs: 30 * 86400, bucketSecs: 12 * 3600 },
];

/**
 * 一个输出桶的统计：k-匿与时段槽同款（只看受信来源数，不加 ASN 门槛——
 * 桶天然稀疏，主指标在窗口上）。不过门槛返回 null。bins 一并交回供
 * 范围分布累计（与窗口统计同口径：用裁剪后的行）。
 */
function trendBucketOrNull(bucketRows: ParsedRow[]): { bucket: TrendBucket; bins: number[] } | null {
  const trusted = bucketRows.filter((r) => r.uaTrusted);
  const sources = new Set(trusted.map((r) => r.source)).size;
  if (sources < MIN_SOURCES) return null;
  const kept = trimExtremeSources(bucketRows);
  const totals = emptyTotals();
  for (const r of kept) addInto(totals, r);
  const cacheDenom = totals.cacheReadTokens + totals.cacheCreationTokens + totals.inputTokens;
  return {
    bucket: {
      // 桶起点由调用方给（对齐网格），这里只算指标
      start: 0,
      p50Ms: quantileFromBins(totals.bins, 0.5),
      p95Ms: quantileFromBins(totals.bins, 0.95),
      errRate: totals.samples > 0 ? totals.errors / totals.samples : null,
      cacheRate: cacheDenom > 0 ? totals.cacheReadTokens / cacheDenom : null,
    },
    bins: totals.bins,
  };
}

/** 由原始桶行构建三档趋势（30 天原始数据一次查询复用）。
 *  P4：同时按模型出趋势（modelRows 来自 bucket_model_raw；v1 期数据缺省为空）。 */
export function buildTrends(rows: RawRow[], nowSec: number, modelRows: RawModelRow[] = []): TrendPayload {
  const bySite = new Map<string, ParsedRow[]>();
  for (const row of rows) {
    const parsed = parseRow(row);
    if (parsed === null) continue;
    const list = bySite.get(parsed.site) ?? [];
    list.push(parsed);
    bySite.set(parsed.site, list);
  }
  const bySiteModel = new Map<string, ParsedModelRow[]>();
  for (const row of modelRows) {
    const parsed = parseModelRow(row);
    if (parsed === null) continue;
    const list = bySiteModel.get(parsed.site) ?? [];
    list.push(parsed);
    bySiteModel.set(parsed.site, list);
  }

  const ranges: TrendPayload["ranges"] = {} as TrendPayload["ranges"];
  // cohort 剔除的「只此一家」来源集合：与快照同源同算（唯源，两处共用）。
  const exclusiveSources = exclusiveSourcesOf(bySite);
  for (const { key, spanSecs, bucketSecs } of TREND_RANGES) {
    // 网格锚在「整点对齐的窗口末尾」，最后一格是当前（可能未满的）小时。
    const endHour = Math.floor(nowSec / 3600) * 3600;
    const start = endHour - spanSecs + 3600;
    const sites: Record<string, SiteTrend> = {};

    for (const [site, siteRows] of bySite) {
      const inRange = siteRows.filter((r) => r.epoch >= start && r.epoch <= endHour);
      const bucketCount = spanSecs / bucketSecs;
      const buckets: TrendBucket[] = [];
      const rangeBins = new Array<number>(TTFT_BIN_COUNT).fill(0);
      let anyPublished = false;

      for (let i = 0; i < bucketCount; i++) {
        const bStart = start + i * bucketSecs;
        const bEnd = bStart + bucketSecs;
        const bucketRows = inRange.filter((r) => r.epoch >= bStart && r.epoch < bEnd);
        const computed = bucketRows.length > 0 ? trendBucketOrNull(bucketRows) : null;
        if (computed) {
          computed.bucket.start = bStart;
          buckets.push(computed.bucket);
          anyPublished = true;
          for (let j = 0; j < TTFT_BIN_COUNT; j++) rangeBins[j] += computed.bins[j];
        } else {
          buckets.push({ start: bStart, p50Ms: null, p95Ms: null, errRate: null, cacheRate: null });
        }
      }
      if (!anyPublished) continue;

      // 站点在此档的窗口统计（与快照 w24/w7 同款聚合与门禁）——展示侧的
      // 指标格随时间档切换读它，30d 档由此首次有了窗口口径。
      const window = windowOrNull(inRange, exclusiveSources);

      // P4：该站在此档的模型趋势（逐 (site, model, bucket) 过 k-匿，
      // 与站点桶同款规则；tpsP50Ms 从 tps 直方图求）。
      const models: Record<string, SiteTrendLite> = {};
      const siteModelRows = bySiteModel.get(site) ?? [];
      const modelsSeen = new Map<string, ParsedModelRow[]>();
      for (const row of siteModelRows) {
        if (row.epoch < start || row.epoch > endHour) continue;
        const list = modelsSeen.get(row.model) ?? [];
        list.push(row);
        modelsSeen.set(row.model, list);
      }
      for (const [model, modelRowsOf] of modelsSeen) {
        const modelBuckets: TrendBucket[] = [];
        let modelPublished = false;
        for (let i = 0; i < bucketCount; i++) {
          const bStart = start + i * bucketSecs;
          const bEnd = bStart + bucketSecs;
          const bucketRows = modelRowsOf.filter((r) => r.epoch >= bStart && r.epoch < bEnd);
          const computed = bucketRows.length > 0 ? trendModelBucketOrNull(bucketRows) : null;
          if (computed) {
            computed.start = bStart;
            modelBuckets.push(computed);
            modelPublished = true;
          } else {
            modelBuckets.push({ start: bStart, p50Ms: null, p95Ms: null, errRate: null, cacheRate: null });
          }
        }
        if (modelPublished) {
          const window = modelWindowOrNull(modelRowsOf);
          models[model] = { buckets: modelBuckets, ...(window ? { window } : {}) } satisfies SiteTrendLite;
        }
      }

      sites[site] = {
        buckets,
        ttftBins: rangeBins,
        ...(window ? { window } : {}),
        ...(Object.keys(models).length > 0 ? { models } : {}),
      };
    }

    ranges[key] = { bucketSeconds: bucketSecs, sites };
  }

  return { version: 1, generatedAt: nowSec, ranges };
}

/** P4：bucket_model_raw 的一行。 */
export interface RawModelRow {
  hour: string;
  site: string;
  app: string;
  model: string;
  source: string;
  asn: number;
  ua_trusted: number;
  samples: number;
  errors: number;
  ttft_bins: string;
  tps_bins: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  cost_usd_micros: number;
  /** P5：被动观察到的模型真伪异常次数（列带 DEFAULT 0，旧行/旧行来源缺省）。 */
  anomalies?: number;
}

interface ParsedModelRow {
  epoch: number;
  site: string;
  model: string;
  source: string;
  uaTrusted: boolean;
  samples: number;
  errors: number;
  ttftBins: number[];
  tpsBins: number[];
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsdMicros: number;
  anomalies: number;
}

/** P4c-2：(站点,模型,范围) 窗口聚合 —— 样本加权合并整段（非逐桶平均），
 *  k-匿与模型桶同款（受信来源门槛）。图阵散点用它而不是平均逐桶值：
 *  平均会把 3 个满数据桶和 1 个空桶同等看待。 */
function modelWindowOrNull(rows: ParsedModelRow[]): ModelWindowStats | null {
  const trusted = rows.filter((r) => r.uaTrusted);
  const sources = new Set(trusted.map((r) => r.source)).size;
  if (sources < MIN_SOURCES) return null;
  const ttftBins = new Array<number>(TTFT_BIN_COUNT).fill(0);
  const tpsBins = new Array<number>(TPS_BIN_COUNT).fill(0);
  let samples = 0;
  let errors = 0;
  let tokenTotal = 0;
  let costUsdMicros = 0;
  let anomalies = 0;
  for (const r of rows) {
    samples += r.samples;
    errors += r.errors;
    anomalies += r.anomalies;
    tokenTotal += r.inputTokens + r.outputTokens + r.cacheReadTokens + r.cacheCreationTokens;
    costUsdMicros += r.costUsdMicros;
    for (let i = 0; i < TTFT_BIN_COUNT; i++) ttftBins[i] += r.ttftBins[i];
    for (let i = 0; i < TPS_BIN_COUNT; i++) tpsBins[i] += r.tpsBins[i];
  }
  return {
    samples,
    p50Ms: quantileFromBins(ttftBins, 0.5),
    p95Ms: quantileFromBins(ttftBins, 0.95),
    errRate: samples > 0 ? errors / samples : null,
    tpsP50Ms: tpsQuantileFromBins(tpsBins, 0.5),
    costUsdPerMTok: tokenTotal > 0 ? costUsdMicros / tokenTotal : null,
    anomalies,
  };
}

function parseModelRow(row: RawModelRow): ParsedModelRow | null {
  let ttftBins: number[];
  let tpsBins: number[];
  try {
    const a: unknown = JSON.parse(row.ttft_bins);
    const b: unknown = JSON.parse(row.tps_bins);
    if (!Array.isArray(a) || a.length !== TTFT_BIN_COUNT) return null;
    if (!Array.isArray(b) || b.length !== TPS_BIN_COUNT) return null;
    ttftBins = a as number[];
    tpsBins = b as number[];
  } catch {
    return null;
  }
  return {
    epoch: hourToEpochSec(row.hour),
    site: row.site,
    model: row.model,
    source: row.source,
    uaTrusted: row.ua_trusted === 1,
    samples: row.samples,
    errors: row.errors,
    ttftBins,
    tpsBins,
    inputTokens: row.input_tokens,
    outputTokens: row.output_tokens,
    cacheReadTokens: row.cache_read_tokens,
    cacheCreationTokens: row.cache_creation_tokens,
    costUsdMicros: row.cost_usd_micros,
    anomalies: row.anomalies ?? 0,
  };
}

/** P4：模型桶指标（k-匿与站点桶同款；不过门槛返回 null —— 不发布）。 */
function trendModelBucketOrNull(
  bucketRows: ParsedModelRow[],
): (TrendBucket & { tpsP50Ms: number | null; costUsdPerMTok: number | null; anomalies: number }) | null {
  const trusted = bucketRows.filter((r) => r.uaTrusted);
  const sources = new Set(trusted.map((r) => r.source)).size;
  if (sources < MIN_SOURCES) return null;
  const totals = {
    samples: 0,
    errors: 0,
    ttftBins: new Array<number>(TTFT_BIN_COUNT).fill(0),
    tpsBins: new Array<number>(TPS_BIN_COUNT).fill(0),
    tokenTotal: 0,
    costUsdMicros: 0,
    anomalies: 0,
  };
  for (const r of bucketRows) {
    totals.samples += r.samples;
    totals.errors += r.errors;
    totals.anomalies += r.anomalies;
    for (let i = 0; i < TTFT_BIN_COUNT; i++) totals.ttftBins[i] += r.ttftBins[i];
    for (let i = 0; i < TPS_BIN_COUNT; i++) totals.tpsBins[i] += r.tpsBins[i];
    totals.tokenTotal += r.inputTokens + r.outputTokens + r.cacheReadTokens + r.cacheCreationTokens;
    totals.costUsdMicros += r.costUsdMicros;
  }
  return {
    start: 0,
    p50Ms: quantileFromBins(totals.ttftBins, 0.5),
    p95Ms: quantileFromBins(totals.ttftBins, 0.95),
    errRate: totals.samples > 0 ? totals.errors / totals.samples : null,
    cacheRate: null,
    tpsP50Ms: tpsQuantileFromBins(totals.tpsBins, 0.5),
    // $/Mtok 同站点窗口口径：微美元 / 总 token（含缓存）
    costUsdPerMTok: totals.tokenTotal > 0 ? totals.costUsdMicros / totals.tokenTotal : null,
    anomalies: totals.anomalies,
  };
}
