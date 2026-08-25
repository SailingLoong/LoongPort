/**
 * transit 摘要的展示格式化与着色。行徽章（`RelayDirectoryRow`）与详情弹窗
 * （`TransitDetailDialog`）共用同一份口径：可用性档位阈值只在这一处定义。
 */

/** 92.5 显示为 93%，95.0 也显示 95% —— 徽章位是整数，精度进 title 不值得。 */
export function formatAvailability(availability: number): string {
  return `${Math.round(availability)}%`;
}

/** 倍率与充值系数的统一后缀形态（0.06x / 3.4x）。 */
export function formatMultiplier(multiplier: number): string {
  return `${multiplier}x`;
}

/** 延迟：秒级以上换算成秒（5391ms → 5.4s），毫秒级取整。 */
export function formatLatency(milliseconds: number): string {
  return milliseconds >= 1000
    ? `${(milliseconds / 1000).toFixed(1)}s`
    : `${Math.round(milliseconds)}ms`;
}

/** 最低充值：CNY 走 ¥ 前缀，其余币种原样标注（币种是站方自报事实）。 */
export function formatTopUp(amount: number, currency: string | null): string {
  return currency === "CNY"
    ? `¥${amount}`
    : currency
      ? `${amount} ${currency}`
      : `${amount}`;
}

/** 可用性徽章的着色，阈值与文字着色（`availabilityTextTone`）同一档位语义。 */
export function availabilityTone(availability: number): string {
  if (availability >= 95)
    return "border-emerald-500/30 bg-emerald-500/5 text-emerald-700 dark:text-emerald-300";
  if (availability >= 85)
    return "border-border-default bg-muted/40 text-foreground";
  return "border-amber-500/30 bg-amber-500/5 text-amber-700 dark:text-amber-300";
}

/** 表格单元格里的可用性文字着色（无徽章底色版，阈值同上）。 */
export function availabilityTextTone(availability: number): string {
  if (availability >= 95) return "text-emerald-600 dark:text-emerald-400";
  if (availability >= 85) return "text-foreground";
  return "text-amber-600 dark:text-amber-400";
}
