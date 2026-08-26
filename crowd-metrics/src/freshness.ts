/**
 * 快照新鲜度与清理节流的纯判定（IO 在 index.ts）。
 *
 * 背景（2026-08-26 线上实证）：这个 Worker 的 cron 触发器**从未触发过**——
 * 三次部署、tail 零调用、KV 无任何 cron 写入。不与平台玄学纠缠，改为
 * 「GET 自愈」：快照超过 [`STALE_AFTER_SECS`] 就在请求路径里现算重写，
 * cron 降级为冗余 belt。清理同理折叠进现算路径，按小时时间闸节流。
 */

/** 快照超过这个岁数，下一次 GET 就现算重写。 */
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
