/** 客户端上传的一个小时聚合桶。**只有聚合指标，没有原始请求。 */
export interface HourBucketPayload {
  /** UTC 小时，形如 '2026-08-26T07Z'。 */
  hour: string;
  /** 归一化 host（小写、无 scheme/端口/www.）。 */
  site: string;
  /** app 标识（claude/codex/…）。形状校验、不枚举 —— 加新 app 不用改服务端。 */
  app: string;
  /** 请求数。 */
  samples: number;
  /** 失败请求数（status ≥ 400 或网络错误）。 */
  errors: number;
  /** TTFT 直方图计数，长度 = TTFT_BIN_COUNT。 */
  ttftBins: number[];
  /** 有 first_token_ms 的样本数，必须 = sum(ttftBins)。 */
  ttftCount: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  /** 该桶总花费，微美元（整数，避免浮点漂移）。 */
  costUsdMicros: number;
  /** P4（version 2）：模型子桶。version 1 载荷缺省。 */
  models?: ModelBucketPayload[];
  /** P4b：非致命熔断跳闸次数（凭证级致命跳闸不计，同 errors 口径）。 */
  breakerTrips?: number;
}

/** P4：站点 × app × 小时 × 模型 的子聚合（顶层字段仍是全量口径）。 */
export interface ModelBucketPayload {
  /** 服务端模型名（公开目录名，白名单形状校验）。 */
  model: string;
  samples: number;
  errors: number;
  ttftBins: number[];
  /** 输出速度直方图（tok/s），长度 = TPS_BIN_COUNT。 */
  tpsBins: number[];
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsdMicros: number;
}

/** POST /v1/ingest 的载荷。一次 flush 携带若干个已闭合的小时桶。 */
export interface IngestPayload {
  version: number;
  /** 客户端当日轮换的随机 id（32 hex）。用于 k-匿名数「独立来源」与幂等去重。 */
  sourceId: string;
  hours: HourBucketPayload[];
}

/** 一个滚动窗口的站点级统计。k-匿名未达标时整个对象为 null。 */
export interface WindowStats {
  samples: number;
  /** 独立来源数（≈ 贡献用户数，按日轮换 id 粗计）。 */
  sources: number;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  /** errors / samples；无样本为 null。 */
  errRate: number | null;
  /** cache_read / (cache_read + cache_creation + input)；分母为 0 为 null。 */
  cacheHitRate: number | null;
  /** 花费参考值：每百万 token 的美元数（模型混合会把该值拉偏，仅供粗参考）。 */
  costUsdPerMTok: number | null;
  /** 合并后的 TTFT 直方图（与 TTFT_BIN_COUNT 等长）。展示用：真实分布近似
   *  对数正态，刷量的「整齐快」分布肉眼可见 —— 分布公开本身就是防线。 */
  ttftBins: number[];
}

/** 24 个 UTC 时段槽（近 7 天聚合）。k-匿名未达标的槽为 {p50Ms: null, samples: 0}。 */
export interface HourSlot {
  p50Ms: number | null;
  samples: number;
}

export interface SiteStats {
  w24: WindowStats | null;
  w7: WindowStats | null;
  hours: HourSlot[];
}

/** 公共快照（GET /v1/snapshot）。两个窗口都没过 k-匿名的站点整个不出现。 */
export interface Snapshot {
  version: number;
  generatedAt: number;
  sites: Record<string, SiteStats>;
  /** TTFT 桶上边界（唯源 src/bins.ts）。随快照下发，展示端不复制常量 —— 分布图的
   *  悬浮提示需要真实毫秒区间，而边界调整必须与聚合侧同源。 */
  ttftBinEdges?: number[];
}

/** 趋势图的一个时间桶（GET /v1/trend）。k-匿未达标的桶各指标为 null。 */
export interface TrendBucket {
  /** 桶起点（epoch 秒，UTC）。 */
  start: number;
  p50Ms: number | null;
  p95Ms: number | null;
  errRate: number | null;
  cacheRate: number | null;
}

export interface SiteTrend {
  buckets: TrendBucket[];
  /** 该范围内合并的 TTFT 直方图（分布图随时间范围联动用）。 */
  ttftBins: number[];
  /** P4：模型维度的趋势（有 v2 数据才有；键=模型名）。 */
  models?: Record<string, SiteTrendLite>;
}

/** P4：模型趋势（无范围分布 —— 站点级已有，模型级省载荷）。 */
export interface SiteTrendLite {
  buckets: Array<TrendBucket & { tpsP50Ms?: number | null; costUsdPerMTok?: number | null }>;
  /** P4c-2：该范围内该 (站点,模型) 的窗口聚合（斩杀线图阵的散点口径；
   *  范围级 k-匿未达标则缺省）。 */
  window?: ModelWindowStats;
}

/** P4c-2：(站点,模型,范围) 窗口聚合——样本加权，非逐桶平均。 */
export interface ModelWindowStats {
  samples: number;
  p50Ms: number | null;
  p95Ms: number | null;
  errRate: number | null;
  tpsP50Ms: number | null;
  costUsdPerMTok: number | null;
}

/** 档位 → 站点趋势。 */
export type TrendPayload = {
  version: 1;
  generatedAt: number;
  ranges: Record<'24h' | '7d' | '30d', { bucketSeconds: number; sites: Record<string, SiteTrend> }>;
  ttftBinEdges?: number[];
};
