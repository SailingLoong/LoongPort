import { Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { RelayDirectoryItem } from "@/lib/api/relay";
import { cn } from "@/lib/utils";

import {
  errRateBadgeTone,
  formatErrRate,
  formatLatency,
  ttftBadgeTone,
} from "./crowdDisplay";
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
  /** 点倍率/实测徽章打开站点详情弹窗；不传（测试）时徽章退化为纯展示。 */
  onOpenTransit?: (item: RelayDirectoryItem) => void;
}

export function RelayDirectoryRow({
  item,
  busy,
  disabled,
  onAuthenticate,
  onOpenTransit,
}: RelayDirectoryRowProps) {
  const { t } = useTranslation();
  const measuredP50Ms = item.crowd?.ttftP50Ms ?? null;
  const measuredErrRate = item.crowd?.errRate ?? null;

  return (
    <article className="grid grid-cols-[44px_minmax(0,1fr)_148px] items-center gap-3 border-b border-border-default px-4 py-3 last:border-b-0 hover:bg-muted/30">
      <div className="text-center text-xs tabular-nums text-muted-foreground">
        <div className="font-medium text-foreground">#{item.rank}</div>
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
          {item.transit?.minMultiplier != null && (
            <button
              type="button"
              className={cn(
                "rounded-full border border-border-default bg-muted/40 px-2 py-0.5 text-[11px] font-medium tabular-nums text-foreground",
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
          {measuredP50Ms != null && (
            <button
              type="button"
              className={cn(
                "rounded-full border px-2 py-0 text-[11px] font-medium tabular-nums",
                ttftBadgeTone(measuredP50Ms),
                onOpenTransit && "hover:border-blue-400 hover:text-blue-600",
              )}
              disabled={!onOpenTransit}
              title={t("loongport.crowd.badgeHint")}
              onClick={() => onOpenTransit?.(item)}
            >
              {t("loongport.crowd.badgeLabel", {
                value: formatLatency(measuredP50Ms),
              })}
            </button>
          )}
          {measuredErrRate != null && (
            <button
              type="button"
              className={cn(
                "rounded-full border px-2 py-0 text-[11px] font-medium tabular-nums",
                errRateBadgeTone(measuredErrRate),
                onOpenTransit && "hover:border-blue-400 hover:text-blue-600",
              )}
              disabled={!onOpenTransit}
              title={t("loongport.crowd.errBadgeHint")}
              onClick={() => onOpenTransit?.(item)}
            >
              {t("loongport.crowd.errBadgeLabel", {
                value: formatErrRate(measuredErrRate),
              })}
            </button>
          )}
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
