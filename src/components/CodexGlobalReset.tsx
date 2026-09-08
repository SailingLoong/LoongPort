import React, { useEffect, useState } from "react";
import { ExternalLink, Timer } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useCodexResetFeed } from "@/lib/query/codexReset";

/**
 * Codex 全局重置预告（挂在官方订阅额度展示下方）。
 *
 * OpenAI 的「善意全局重置」不定期、由负责人在 X 公告；这里展示社区
 * （codex-reset.com）聚合的预告：
 * - 有可解析的预计落地时间 → **活的倒计时**（逐秒走到预计时刻）；
 * - 没有预告/已过期 → 上次已验证重置时间 + 公告原文链接，不猜下一次。
 * 数据拿不到时整块不渲染（宁缺毋认）。
 */
const CodexGlobalReset: React.FC = () => {
  const { t } = useTranslation();
  const { data: feed } = useCodexResetFeed();
  const [now, setNow] = useState(() => Date.now());

  const landing = feed?.landingAt ?? null;
  const counting = landing != null && landing * 1000 > now;

  // 倒计时逐秒走；无倒计时时不挂定时器。
  useEffect(() => {
    if (!counting) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [counting]);

  if (!feed || (!feed.announcementAt && !feed.lastVerifiedAt)) return null;

  const eta = counting ? formatCountdown(landing! * 1000 - now) : null;
  const lastVerified =
    feed.lastVerifiedAt != null ? new Date(feed.lastVerifiedAt * 1000) : null;

  return (
    <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
      <span className="inline-flex items-center gap-1 font-medium text-foreground">
        <Timer className="size-3.5" aria-hidden="true" />
        {t("loongport.codexReset.label")}
      </span>
      {eta ? (
        <span
          className="tabular-nums text-foreground"
          title={t("loongport.codexReset.etaTitle")}
        >
          {t("loongport.codexReset.eta", { time: eta })}
        </span>
      ) : lastVerified ? (
        <span title={t("loongport.codexReset.lastResetTitle")}>
          {t("loongport.codexReset.lastReset", {
            time: lastVerified.toLocaleDateString(),
          })}
        </span>
      ) : null}
      {feed.announcementUrl && (
        <a
          href={feed.announcementUrl}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 underline-offset-4 hover:underline"
          title={feed.announcementSummary ?? undefined}
        >
          {t("loongport.codexReset.announcement")}
          <ExternalLink className="size-3" aria-hidden="true" />
        </a>
      )}
      <span>{t("loongport.codexReset.source")}</span>
    </div>
  );
};

/** 剩余毫秒 → 人话倒计时：`3 天 04:12:09` / `05:12:09` / `42 秒`。 */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  const hms = `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;
  if (days > 0) return `${days}d ${hms}`;
  if (total >= 3600) return hms;
  if (total >= 60) return `${pad(minutes)}:${pad(seconds)}`;
  return `${total}s`;
}

export default CodexGlobalReset;
