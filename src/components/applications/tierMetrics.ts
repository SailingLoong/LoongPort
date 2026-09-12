import type { ApplicationRoutingTier } from "@/lib/api/applicationRouting";
import { fmtUsd } from "@/components/usage/format";

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
    descending: false,
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
] as const;
export type TierMetric = (typeof tierMetrics)[number]["key"];
export interface TierSort {
  key: TierMetric;
  descending: boolean;
}

/** A user-requested snapshot order. Metric updates never reorder the list. */
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
