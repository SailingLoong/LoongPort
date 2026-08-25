import { ExternalLink, Loader2, LockKeyhole } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { RelayDirectoryItem } from "@/lib/api/relay";
import { cn } from "@/lib/utils";

import { openInBrowser } from "../openInBrowser";
import {
  availabilityTone,
  formatAvailability,
  formatMultiplier,
} from "./transitDisplay";

interface RelayDirectoryRowProps {
  item: RelayDirectoryItem;
  busy: boolean;
  disabled: boolean;
  onAuthenticate: (item: RelayDirectoryItem) => void;
  /** 点倍率徽章打开站方公开数据详情；不传（测试）时徽章退化为纯展示。 */
  onOpenTransit?: (item: RelayDirectoryItem) => void;
}

function scoreTone(score: number): string {
  if (score >= 90) return "text-emerald-600 dark:text-emerald-400";
  if (score >= 75) return "text-blue-600 dark:text-blue-400";
  return "text-amber-600 dark:text-amber-400";
}

export function RelayDirectoryRow({
  item,
  busy,
  disabled,
  onAuthenticate,
  onOpenTransit,
}: RelayDirectoryRowProps) {
  const { t } = useTranslation();

  return (
    <article className="grid grid-cols-[44px_minmax(0,1fr)_86px_148px] items-center gap-3 border-b border-border-default px-4 py-3 last:border-b-0 hover:bg-muted/30">
      <div className="text-center text-xs tabular-nums text-muted-foreground">
        <div className="font-medium text-foreground">
          {item.rank === null
            ? t("loongport.directory.meta.supplementalRank")
            : `#${item.rank}`}
        </div>
      </div>

      <div className="min-w-0">
        <div className="flex min-w-0 items-baseline gap-2">
          <h3 className="truncate text-sm font-semibold text-foreground">
            {item.displayName}
          </h3>
          <span className="truncate text-xs text-muted-foreground">
            {item.siteHost}
          </span>
        </div>

        <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
          {item.protocolScores.map((protocol) => (
            <button
              key={protocol.protocol}
              type="button"
              className={cn(
                "rounded-full border border-border-default bg-muted/40 px-2 py-0.5 text-[11px] tabular-nums text-muted-foreground",
                protocol.reportUrl &&
                  "hover:border-blue-400 hover:text-blue-600",
              )}
              disabled={!protocol.reportUrl}
              onClick={() =>
                protocol.reportUrl && openInBrowser(protocol.reportUrl)
              }
            >
              {protocol.protocol} {protocol.score}
            </button>
          ))}
          {item.claudeSignatureRate !== null && (
            <Badge
              variant="outline"
              className="gap-1 border-emerald-500/30 bg-emerald-500/5 px-2 py-0 text-[11px] font-medium text-emerald-700 dark:text-emerald-300"
              title={t("loongport.directory.signatureHint")}
            >
              <LockKeyhole className="h-3 w-3" />
              {item.claudeSignatureRate}%
            </Badge>
          )}
          {item.transit?.minMultiplier != null && (
            <button
              type="button"
              className={cn(
                "rounded-full border border-border-default bg-muted/40 px-2 py-0 text-[11px] font-medium tabular-nums text-foreground",
                onOpenTransit && "hover:border-blue-400 hover:text-blue-600",
              )}
              disabled={!onOpenTransit}
              title={
                onOpenTransit
                  ? t("loongport.directory.transit.openDetail")
                  : t("loongport.directory.transit.multiplierHint")
              }
              onClick={() => onOpenTransit?.(item)}
            >
              {formatMultiplier(item.transit.minMultiplier)}
            </button>
          )}
          {item.transit?.minAvailability != null && (
            <Badge
              variant="outline"
              className={cn(
                "px-2 py-0 text-[11px] font-medium tabular-nums",
                availabilityTone(item.transit.minAvailability),
              )}
              title={t("loongport.directory.transit.availabilityHint")}
            >
              {formatAvailability(item.transit.minAvailability)}
            </Badge>
          )}
          {item.scenarios.map((scenario) => (
            <Badge
              key={scenario}
              variant="secondary"
              className="px-2 py-0 text-[11px] font-normal"
            >
              {scenario}
            </Badge>
          ))}
          {item.issues.map((issue) => (
            <code
              key={issue}
              className="rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-700 dark:text-amber-300"
            >
              {issue}
            </code>
          ))}
        </div>

        <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
          <span>
            {t("loongport.directory.meta.samples", { count: item.samples })}
          </span>
          <span>
            {t("loongport.directory.meta.latest", { date: item.latestDate })}
          </span>
          <button
            type="button"
            className="inline-flex items-center gap-1 text-blue-600 hover:underline dark:text-blue-400"
            aria-label={t("loongport.directory.actions.history")}
            onClick={() => openInBrowser(item.detailUrl)}
          >
            {t("loongport.directory.actions.history")}
            <ExternalLink className="h-3 w-3" />
          </button>
        </div>
      </div>

      <div className="text-center">
        <div
          className={cn(
            "text-2xl font-semibold tabular-nums tracking-tight",
            scoreTone(item.score),
          )}
        >
          {item.score}
        </div>
        <div className="text-[10px] text-muted-foreground">
          {t("loongport.directory.scoreLabel")}
        </div>
      </div>

      <div className="flex flex-col items-stretch gap-1">
        <Button
          size="sm"
          disabled={disabled}
          onClick={() => onAuthenticate(item)}
        >
          {busy && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
          {t("loongport.directory.actions.authenticate")}
        </Button>
        <span className="text-center text-[10px] text-muted-foreground">
          {/* 广场只展示受管站点（后端 apply_policy 的白名单过滤），一律一键登录。 */}
          {t("loongport.directory.actions.autoAddHint")}
        </span>
      </div>
    </article>
  );
}
