/**
 * 预设第三方厂商的返佣注册链接解析。
 *
 * 远端配置的 `preset_referral_urls`（host → 完整注册 URL）是**商务覆盖**，
 * 优先于代码里预设自带的中性链接——谈成新码改远端配置即可、无需发版。
 * 查键顺序：精确 host → 去 `www.` 再试（配置录入规则与后端 `aff_codes` 同：
 * 键不带 `www.`）。不做更激进的 apex 推导（`co.uk` 这类二级后缀会推错，
 * 预设链接的真实形态只需要去 www 这一层）。
 */

/** 候选链接命中返佣覆盖时返回覆盖 URL，否则 `null`（调用方回落中性链接）。 */
export function resolvePresetReferralUrl(
  candidateUrl: string | null | undefined,
  referrals: Record<string, string> | null | undefined,
): string | null {
  if (!candidateUrl || !referrals) return null;
  let hostname: string;
  try {
    hostname = new URL(candidateUrl).hostname.toLowerCase();
  } catch {
    return null;
  }
  const exact = referrals[hostname];
  if (typeof exact === "string" && exact) return exact;
  if (hostname.startsWith("www.")) {
    const bare = hostname.slice("www.".length);
    const hit = referrals[bare];
    if (typeof hit === "string" && hit) return hit;
  }
  return null;
}
