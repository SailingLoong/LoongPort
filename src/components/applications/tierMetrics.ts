import type { ApplicationRoutingTier } from "@/lib/api/applicationRouting";
import { fmtUsd } from "@/components/usage/format";

const relativeTime = new Intl.RelativeTimeFormat(undefined, {
  numeric: "always",
});

/** 重置时刻的相对显示：分/时/天自动选档，太久远落到日期。已过期显示 —（快照过期）。 */
export function formatResetAt(epochSecs: number): string {
  const deltaSecs = epochSecs - Date.now() / 1000;
  if (deltaSecs <= 0) return "—";
  if (deltaSecs < 3600) {
    return relativeTime.format(Math.round(deltaSecs / 60), "minute");
  }
  if (deltaSecs < 48 * 3600) {
    return relativeTime.format(Math.round(deltaSecs / 3600), "hour");
  }
  if (deltaSecs < 45 * 86400) {
    return relativeTime.format(Math.round(deltaSecs / 86400), "day");
  }
  return new Date(epochSecs * 1000).toLocaleDateString();
}

export const tierMetrics = [
  {
    key: "rateMultiplier",
    descending: false,
    format: (value: number) => `${value.toLocaleString()}×`,
  },
  {
    key: "errorRate",
    descending: false,
    format: (value: number) => `${(value * 100).toFixed(1)}%`,
  },
  {
    key: "avgFirstTokenMs",
    descending: true,
    format: (value: number) => `${(value / 1000).toFixed(2)}s`,
  },
  {
    key: "balanceUsd",
    descending: true,
    format: (value: number) => fmtUsd(value, 2),
  },
  {
    key: "cacheHitRate",
    descending: true,
    format: (value: number) => `${(value * 100).toFixed(1)}%`,
  },
  {
    key: "todayCostUsd",
    descending: false,
    format: (value: number) => fmtUsd(value, 2),
  },
  {
    // 下次重置（epoch 秒）：升序 = 最早重置在前——「优先消耗即将作废的额度」。
    key: "nextResetAt",
    descending: false,
    format: formatResetAt,
  },
] as const;
export type TierMetric = (typeof tierMetrics)[number]["key"];
export interface TierSort {
  key: TierMetric;
  descending: boolean;
}

/** 某指标的首次点击方向（用户为每个指标定的「默认那个」↑/↓）。 */
export function defaultDescending(key: TierMetric): boolean {
  return tierMetrics.find((metric) => metric.key === key)!.descending;
}

/** 视图排序：只重排当前展示，不落库。默认序 = 数据库里的档位序（拖拽维护）。 */
export function sortTierIds(
  ids: string[],
  tiers: ApplicationRoutingTier[],
  sort: TierSort,
): string[] {
  const values = new Map(
    tiers.map((tier) => [tier.providerId, tier[sort.key]]),
  );
  return [...ids].sort((a, b) => {
    const av = values.get(a),
      bv = values.get(b);
    if (av == null) return bv == null ? 0 : 1;
    if (bv == null) return -1;
    return (av - bv) * (sort.descending ? -1 : 1);
  });
}
