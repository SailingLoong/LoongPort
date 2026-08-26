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
}
