/**
 * 写入端点共用的每 IP 限流（ingest 与 ping 同一张表、同一个预算）。
 *
 * 状态存 D1 而非 KV —— KV 免费档每天只有 1k 写，限流计数会把它打爆；
 * D1 的写额度是十万行/天。行保留 2 天，清理折叠进快照现算路径。
 *
 * ⚠️ 只存 IP 的 SHA-256，不存 IP 本身（接收端不记 IP 是 stats.rs 隐私评审
 * 定的义务，两个写入端点一体适用）。
 */

export interface RateLimitEnv {
  DB: D1Database;
}

/** 每来源 IP 每小时窗的最大写入次数（ingest + ping 合算）。 */
const MAX_WRITES_PER_IP_HOUR = 20;

async function ipHash(ip: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(ip),
  );
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** 限流：返回这次请求是否放行（计数含本次）。 */
export async function allowByIp(
  env: RateLimitEnv,
  ip: string,
  hour: string,
): Promise<boolean> {
  const hash = await ipHash(ip);
  const result = await env.DB.prepare(
    `INSERT INTO upload_ip_hour (ip_hash, hour, count) VALUES (?1, ?2, 1)
     ON CONFLICT (ip_hash, hour) DO UPDATE SET count = count + 1
     RETURNING count`,
  )
    .bind(hash, hour)
    .first<{ count: number }>();
  return (result?.count ?? 0) <= MAX_WRITES_PER_IP_HOUR;
}
