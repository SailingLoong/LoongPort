import { Lock, Users } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import type { CrowdSiteStats } from "@/lib/api/crowd";
import { cn } from "@/lib/utils";

import {
  errRateTone,
  formatCostPerMTok,
  formatErrRate,
  formatLatency,
  ttftTone,
} from "./crowdDisplay";
import { formatAvailability } from "./transitDisplay";

interface CrowdMeasuredSectionProps {
  /** 该站的已发布数据；`null` = 样本/来源不足（k-匿名的正常缺席）。 */
  stats: CrowdSiteStats | null;
  /** 共建开关（门禁事实来自后端命令，这里只管渲染形态）。 */
  enabled: boolean;
  /** 打开共建告知弹窗（锁定态的加入入口）。 */
  onJoin: () => void;
}

function StatCell({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: string;
}) {
  return (
    <div className="rounded-lg border border-border-default bg-muted/20 px-3 py-2">
      <div className="text-[10px] text-muted-foreground">{label}</div>
      <div
        className={cn(
          "mt-0.5 text-sm font-semibold tabular-nums",
          tone ?? "text-foreground",
        )}
      >
        {value}
      </div>
    </div>
  );
}

/** 24 时段迷你条形：条高 ∝ p50 延迟（越高越慢），空槽只剩基线。 */
function HourBars({ stats }: { stats: CrowdSiteStats }) {
  const { t } = useTranslation();
  const slots = stats.hours;
  const heights = slots.map((slot) =>
    slot.p50Ms != null && slot.p50Ms > 0 ? slot.p50Ms : null,
  );
  const max = Math.max(...heights.map((h) => h ?? 0), 1);

  return (
    <div className="mt-3">
      <div className="text-[10px] text-muted-foreground">
        {t("loongport.crowd.hoursTitle")}
      </div>
      <div className="mt-1.5 flex h-12 items-end gap-[3px]" aria-hidden>
        {heights.map((height, slot) => (
          <div
            key={slot}
            className={cn(
              "flex-1 rounded-sm",
              height != null ? "bg-primary/35" : "bg-border-default/60",
            )}
            style={{
              height:
                height != null ? `${Math.max((height / max) * 100, 6)}%` : "8%",
            }}
            title={
              height != null
                ? t("loongport.crowd.hourTooltip", {
                    slot: String(slot).padStart(2, "0"),
                    value: formatLatency(height),
                    count: slots[slot].samples,
                  })
                : t("loongport.crowd.hourEmpty", {
                    slot: String(slot).padStart(2, "0"),
                  })
            }
          />
        ))}
      </div>
      <div className="mt-1 flex justify-between text-[9px] text-muted-foreground tabular-nums">
        <span>00</span>
        <span>06</span>
        <span>12</span>
        <span>18</span>
        <span>23</span>
      </div>
    </div>
  );
}

/**
 * 站点详情弹窗里的「用户实测」区块（与站方自报的 transit 数据并排、来源分列）。
 *
 * 三态：未参与 → 锁定卡（对等条款 + 加入入口）；参与但无数据 → 占位说明；
 * 参与且有数据 → 指标卡 + 时段画像。**门禁事实不在这里判断** ——
 * 调用方拿到的 snapshot 在共建关闭时本来就是 null。
 */
export function CrowdMeasuredSection({
  stats,
  enabled,
  onJoin,
}: CrowdMeasuredSectionProps) {
  const { t } = useTranslation();

  if (!enabled) {
    return (
      <section className="mt-5 border-t border-border-default pt-4">
        <h4 className="text-xs font-semibold text-foreground">
          {t("loongport.crowd.sectionTitle")}
        </h4>
        <div className="mt-2 flex items-center gap-3 rounded-lg border border-dashed border-border-default bg-muted/20 px-4 py-3">
          <Lock className="h-4 w-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="text-xs font-medium text-foreground">
              {t("loongport.crowd.lockedTitle")}
            </div>
            <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
              {t("loongport.crowd.lockedBody")}
            </p>
          </div>
          <Button size="sm" variant="outline" onClick={onJoin}>
            {t("loongport.crowd.join")}
          </Button>
        </div>
      </section>
    );
  }

  const active = stats?.w24 ?? stats?.w7 ?? null;

  return (
    <section className="mt-5 border-t border-border-default pt-4">
      <div className="flex items-baseline justify-between gap-2">
        <h4 className="text-xs font-semibold text-foreground">
          {t("loongport.crowd.sectionTitle")}
        </h4>
        {active && (
          <span className="inline-flex items-center gap-1 text-[10px] text-muted-foreground">
            <Users className="h-3 w-3" />
            {t("loongport.crowd.noteSources", {
              count: active.sources,
              samples: active.samples.toLocaleString(),
            })}
            {stats?.w24
              ? ` · ${t("loongport.crowd.window24")}`
              : ` · ${t("loongport.crowd.window7")}`}
          </span>
        )}
      </div>

      {!active ? (
        <p className="mt-2 text-[11px] text-muted-foreground">
          {t("loongport.crowd.noData")}
        </p>
      ) : (
        <>
          <div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-5">
            <StatCell
              label={t("loongport.crowd.ttftP50")}
              value={formatLatency(active.ttftP50Ms ?? 0)}
              tone={
                active.ttftP50Ms != null
                  ? ttftTone(active.ttftP50Ms)
                  : undefined
              }
            />
            <StatCell
              label={t("loongport.crowd.ttftP95")}
              value={
                active.ttftP95Ms != null ? formatLatency(active.ttftP95Ms) : "—"
              }
            />
            <StatCell
              label={t("loongport.crowd.errRate")}
              value={
                active.errRate != null ? formatErrRate(active.errRate) : "—"
              }
              tone={
                active.errRate != null ? errRateTone(active.errRate) : undefined
              }
            />
            <StatCell
              label={t("loongport.crowd.cacheHit")}
              value={
                active.cacheHitRate != null
                  ? formatAvailability(active.cacheHitRate * 100)
                  : "—"
              }
            />
            <StatCell
              label={t("loongport.crowd.costRef")}
              value={
                active.costUsdPerMTok != null
                  ? formatCostPerMTok(active.costUsdPerMTok)
                  : "—"
              }
            />
          </div>
          {stats && <HourBars stats={stats} />}
          <p className="mt-2 text-[10px] text-muted-foreground">
            {t("loongport.crowd.footnote")}
          </p>
        </>
      )}
    </section>
  );
}
