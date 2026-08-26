/**
 * 用户实测数据的展示格式化与着色。与 `transitDisplay`（站方自报数据）并排：
 * 两类数据来源不同、口径不同，格式化助手也各归各 —— 共用的部分（可用性百分比）
 * 直接复用 transit 的，不复制阈值。
 */

import { formatLatency } from "./transitDisplay";

export { formatLatency };

/** 错误率：0.004 → 0.40%；≥1% 保留一位小数（1.2%）。 */
export function formatErrRate(rate: number): string {
  const percent = rate * 100;
  return `${percent >= 1 ? percent.toFixed(1) : percent.toFixed(2)}%`;
}

/** 花费参考值：$1.25 / 百万 token。 */
export function formatCostPerMTok(usd: number): string {
  return `$${usd.toFixed(2)}`;
}

/**
 * TTFT 着色档位：< 800ms 快（emerald）、< 2000ms 正常、否则偏慢（amber）。
 * 阈值只在这一处定义 —— 徽章与详情区块共用同一档位语义。
 */
export function ttftTone(ms: number): string {
  if (ms < 800) return "text-emerald-600 dark:text-emerald-400";
  if (ms < 2000) return "text-foreground";
  return "text-amber-600 dark:text-amber-400";
}

/** TTFT 徽章底色版（同上档位）。 */
export function ttftBadgeTone(ms: number): string {
  if (ms < 800)
    return "border-emerald-500/30 bg-emerald-500/5 text-emerald-700 dark:text-emerald-300";
  if (ms < 2000) return "border-border-default bg-muted/40 text-foreground";
  return "border-amber-500/30 bg-amber-500/5 text-amber-700 dark:text-amber-300";
}

/** 错误率着色：< 0.5% 好、< 3% 正常、否则偏高。 */
export function errRateTone(rate: number): string {
  if (rate < 0.005) return "text-emerald-600 dark:text-emerald-400";
  if (rate < 0.03) return "text-foreground";
  return "text-amber-600 dark:text-amber-400";
}
