/**
 * 快照新鲜度与清理节流的纯判定（IO 在 index.ts）。
 *
 * 背景：2026-08-26 曾实证 cron 触发器不触发，落地「GET 自愈」兜底架构；
 * 2026-09-07 由 KV 写入配额对账证实 cron 已恢复实跑（每 5 分钟 × 2 键
 * ≈ 576 写/天，触发免费档 50% 告警线），周期放宽到每 10 分钟。现行分工：
 * cron 每 10 分钟重算是常规通道，快照超过 [`STALE_AFTER_SECS`] 就由
 * 下一次 GET 在请求路径里现算重写兜底；清理同理折叠进重算路径，
 * 按小时时间闸节流。
 */

/** 快照超过这个岁数，下一次 GET 就现算重写。与 cron 周期（10 分钟）刻意对齐：
 * cron 偶发迟到几秒由这里兜底；若把 cron 放得比这更宽，每个周期都会出现
 * 过期窗，重算会被稳定推到用户请求路径上（响应变慢、写入回升）。 */
export const STALE_AFTER_SECS = 10 * 60;

/** 清理（保留期删除）的最小间隔：每小时最多一次。 */
export const CLEANUP_EVERY_SECS = 3600;

/** 缓存的快照内容是否还算新鲜。解析失败按陈旧处理（触发重算自愈）。 */
export function isFresh(
  snapshotJson: string | null,
  nowSec: number,
): boolean {
  if (snapshotJson == null) return false;
  try {
    const generatedAt = (JSON.parse(snapshotJson) as { generatedAt?: unknown })
      .generatedAt;
    return (
      typeof generatedAt === "number" &&
      nowSec - generatedAt <= STALE_AFTER_SECS
    );
  } catch {
    return false;
  }
}

/** 距上次清理是否已过节流间隔（`lastRunSec` 为 null = 从没跑过）。 */
export function cleanupDue(lastRunSec: number | null, nowSec: number): boolean {
  if (lastRunSec == null) return true;
  return nowSec - lastRunSec >= CLEANUP_EVERY_SECS;
}
