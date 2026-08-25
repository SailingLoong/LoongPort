import { ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type { RelayDirectoryItem } from "@/lib/api/relay";
import { cn } from "@/lib/utils";

import { openInBrowser } from "../openInBrowser";
import {
  availabilityTextTone,
  formatAvailability,
  formatLatency,
  formatMultiplier,
  formatTopUp,
} from "./transitDisplay";

interface TransitDetailDialogProps {
  item: RelayDirectoryItem;
  open: boolean;
  onDismiss: () => void;
}

/** meta 行的一个「标签 + 值」段；值缺席就不渲染（后端没有的事实不展示）。 */
function MetaField({ label, value }: { label: string; value: string }) {
  return (
    <span className="whitespace-nowrap">
      {label}
      <span className="ml-1.5 tabular-nums text-foreground">{value}</span>
    </span>
  );
}

/**
 * 站方公开数据详情（ai-transit.v1 快照投影）：充值口径、逐分组
 * 倍率/缓存命中/可用性/延迟、来源披露。广场行保持两个徽章，丰富信息
 * 全部落在这里。
 */
export function TransitDetailDialog({
  item,
  open,
  onDismiss,
}: TransitDetailDialogProps) {
  const { t, i18n } = useTranslation();
  const transit = item.transit;
  if (!transit) return null;

  // 取成 const 让 JSX 回调闭包保留收窄（避免非空断言）。
  const supportUrl = transit.supportUrl;
  const priceUrl = transit.priceUrl;

  const dataTime =
    transit.syncedAt > 0
      ? new Intl.DateTimeFormat(i18n.resolvedLanguage || undefined, {
          dateStyle: "medium",
          timeStyle: "short",
        }).format(new Date(transit.syncedAt * 1000))
      : null;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onDismiss();
      }}
    >
      <DialogContent className="max-w-2xl gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {item.displayName} · {t("loongport.directory.transit.detailTitle")}
        </DialogTitle>

        <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1.5 text-xs text-muted-foreground">
          {transit.rechargeMultiplier != null && (
            <MetaField
              label={t("loongport.directory.transit.rechargeMultiplier")}
              value={formatMultiplier(transit.rechargeMultiplier)}
            />
          )}
          {transit.minimumTopUp != null && (
            <MetaField
              label={t("loongport.directory.transit.minimumTopUp")}
              value={formatTopUp(transit.minimumTopUp, transit.currency)}
            />
          )}
          {transit.upstreamType != null && (
            <MetaField
              label={t("loongport.directory.transit.upstreamType")}
              value={transit.upstreamType}
            />
          )}
          {transit.isReverse != null && (
            <MetaField
              label={t("loongport.directory.transit.reversePool")}
              value={t(
                transit.isReverse
                  ? "loongport.directory.transit.reverseYes"
                  : "loongport.directory.transit.reverseNo",
              )}
            />
          )}
        </div>

        {transit.groups.length > 0 && (
          <div className="mt-4 max-h-[50vh] overflow-auto rounded-lg border border-border-default">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="h-9">
                    {t("loongport.directory.transit.colGroup")}
                  </TableHead>
                  <TableHead className="h-9">
                    {t("loongport.directory.transit.colPlatform")}
                  </TableHead>
                  <TableHead className="h-9 text-right">
                    {t("loongport.directory.transit.colMultiplier")}
                  </TableHead>
                  <TableHead className="h-9 text-right">
                    {t("loongport.directory.transit.colCacheHit")}
                  </TableHead>
                  <TableHead className="h-9 text-right">
                    {t("loongport.directory.transit.colAvailability")}
                  </TableHead>
                  <TableHead className="h-9 text-right">
                    {t("loongport.directory.transit.colLatency")}
                  </TableHead>
                  <TableHead className="h-9 text-right">
                    {t("loongport.directory.transit.colModels")}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {transit.groups.map((group) => (
                  <TableRow key={group.name}>
                    <TableCell className="max-w-[180px] truncate py-2 text-xs font-medium">
                      {group.name}
                    </TableCell>
                    <TableCell className="py-2 text-xs text-muted-foreground">
                      {group.platform}
                    </TableCell>
                    <TableCell className="py-2 text-right text-xs tabular-nums">
                      {group.multiplier != null
                        ? formatMultiplier(group.multiplier)
                        : "—"}
                    </TableCell>
                    <TableCell className="py-2 text-right text-xs tabular-nums text-muted-foreground">
                      {group.cacheHitRate7d != null
                        ? formatAvailability(group.cacheHitRate7d)
                        : "—"}
                    </TableCell>
                    <TableCell
                      className={cn(
                        "py-2 text-right text-xs font-medium tabular-nums",
                        group.availability != null
                          ? availabilityTextTone(group.availability)
                          : "text-muted-foreground",
                      )}
                    >
                      {group.availability != null
                        ? formatAvailability(group.availability)
                        : "—"}
                    </TableCell>
                    <TableCell className="py-2 text-right text-xs tabular-nums text-muted-foreground">
                      {group.avgLatencyMs != null
                        ? formatLatency(group.avgLatencyMs)
                        : "—"}
                    </TableCell>
                    <TableCell className="py-2 text-right text-xs tabular-nums text-muted-foreground">
                      {group.modelCount}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}

        <div className="mt-4 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
          <span>
            {dataTime &&
              t("loongport.directory.transit.dataTime", { time: dataTime })}
          </span>
          <span className="flex items-center gap-3">
            {supportUrl && (
              <button
                type="button"
                className="inline-flex items-center gap-1 text-blue-600 hover:underline dark:text-blue-400"
                onClick={() => openInBrowser(supportUrl)}
              >
                {t("loongport.directory.transit.contactSupport")}
                <ExternalLink className="h-3 w-3" />
              </button>
            )}
            {priceUrl && (
              <button
                type="button"
                className="inline-flex items-center gap-1 text-blue-600 hover:underline dark:text-blue-400"
                onClick={() => openInBrowser(priceUrl)}
              >
                {t("loongport.directory.transit.viewPricePage")}
                <ExternalLink className="h-3 w-3" />
              </button>
            )}
          </span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
