/**
 * 省心看板的档位列表：自动模式平铺、手动模式可拖拽排序。
 *
 * dnd 形状抄 `RelayTierList`（同一套 sensors/strategy/把手注入）；卡片只展示
 * 后端看板算好的事实（顺序/倍率/单价/余额/耗时/命中），不在前端拼业务判据。
 */
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Check, Copy, GripVertical, RefreshCw } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { copyText } from "@/lib/clipboard";
import { useResetCircuitBreaker } from "@/lib/query/failover";
import { cn } from "@/lib/utils";
import { fmtInt, fmtUsd } from "@/components/usage/format";
import type { TierBoardTier } from "@/lib/api/autoMode";
import type { DraggableAttributes } from "@dnd-kit/core";
import type { SyntheticListenerMap } from "@dnd-kit/core/dist/hooks/utilities";

export function TierList({
  tiers,
  manual,
  appType,
  onReorder,
}: {
  tiers: TierBoardTier[];
  manual: boolean;
  appType: string;
  onReorder: (orderedIds: string[]) => void;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
  const ids = tiers.map((tier) => tier.providerId);

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    onReorder(arrayMove(ids, from, to));
  };

  if (!manual) {
    return (
      <div className="space-y-2">
        {tiers.map((tier) => (
          <TierCard key={tier.providerId} tier={tier} appType={appType} />
        ))}
      </div>
    );
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        <div className="space-y-2">
          {tiers.map((tier) => (
            <SortableTierCard
              key={tier.providerId}
              tier={tier}
              appType={appType}
            />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}

function SortableTierCard({
  tier,
  appType,
}: {
  tier: TierBoardTier;
  appType: string;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: tier.providerId });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={isDragging ? "z-10" : undefined}
    >
      <TierCard
        tier={tier}
        appType={appType}
        dragHandleProps={{ attributes, listeners }}
      />
    </div>
  );
}

/**
 * 档位卡：序号 + 名称 + 当前命中 + 失败原因 + 重新启用 + 指标行。
 * 指标行顺序按语义分组：定价（倍率/单价）→ 运行（今日/首字/缓存）→ 供给（余额）；
 * 金额/整数走全仓唯源 fmtUsd/fmtInt，未知值统一 —。
 */
function TierCard({
  tier,
  appType,
  dragHandleProps,
}: {
  tier: TierBoardTier;
  appType: string;
  dragHandleProps?: {
    attributes?: DraggableAttributes;
    listeners?: SyntheticListenerMap;
  };
}) {
  const { t } = useTranslation();
  const resetBreaker = useResetCircuitBreaker();
  const failed =
    tier.isHealthy === false || (tier.consecutiveFailures ?? 0) > 0;
  return (
    <div className="flex items-center gap-3 rounded-lg border bg-card p-3">
      {dragHandleProps ? (
        <button
          type="button"
          className="cursor-grab self-stretch text-muted-foreground"
          aria-label={t("autoMode.board.dragHandle", {
            defaultValue: "拖动排序",
          })}
          {...(dragHandleProps?.attributes ?? {})}
          {...(dragHandleProps?.listeners ?? {})}
        >
          <GripVertical className="h-4 w-4" />
        </button>
      ) : null}
      <span className="w-5 text-center text-xs tabular-nums text-muted-foreground">
        {tier.position + 1}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium">{tier.name}</span>
          {tier.isCurrent ? (
            <Badge
              variant="outline"
              className="border-emerald-500/60 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
            >
              {t("autoMode.board.current", { defaultValue: "当前" })}
            </Badge>
          ) : null}
          <TierHealthBadge tier={tier} />
          {tier.verificationVerdict === "anomaly" ||
          tier.verificationVerdict === "suspicious" ? (
            <span
              title={t(
                `loongport.modelVerification.tierVerdict.${tier.verificationVerdict}`,
                {
                  defaultValue:
                    tier.verificationVerdict === "anomaly"
                      ? "检测到异常"
                      : "需要复核",
                },
              )}
              className={`h-2 w-2 shrink-0 rounded-full ${
                tier.verificationVerdict === "anomaly"
                  ? "bg-red-600"
                  : "bg-amber-500"
              }`}
            >
              <span className="sr-only">
                {t(
                  `loongport.modelVerification.tierVerdict.${tier.verificationVerdict}`,
                  {
                    defaultValue:
                      tier.verificationVerdict === "anomaly"
                        ? "检测到异常"
                        : "需要复核",
                  },
                )}
              </span>
            </span>
          ) : null}
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <span className="tabular-nums">
            ×
            {tier.rateMultiplier ??
              t("autoMode.board.unknown", { defaultValue: "?" })}
          </span>
          <span className="tabular-nums">
            {tier.unitPricePerMillion != null
              ? `${fmtUsd(tier.unitPricePerMillion, 2)}/M`
              : t("autoMode.board.priceUnknown", { defaultValue: "价格未知" })}
          </span>
          <span className="tabular-nums">
            {t("autoMode.board.today", { defaultValue: "今日" })}{" "}
            {tier.todayCostUsd != null && tier.todayRequests != null
              ? `${fmtUsd(tier.todayCostUsd, 2, "—")} · ${t(
                  "autoMode.board.requestsUnit",
                  {
                    defaultValue: "{{count}} 次",
                    count: fmtInt(tier.todayRequests, undefined, "—"),
                  },
                )}`
              : "—"}
          </span>
          <span className="tabular-nums">
            {t("autoMode.board.ttft", { defaultValue: "首字" })}{" "}
            {tier.avgFirstTokenMs != null
              ? t("autoMode.board.ms", {
                  defaultValue: "{{value}}ms",
                  value: tier.avgFirstTokenMs,
                })
              : "—"}
          </span>
          <span className="tabular-nums">
            {t("autoMode.board.cache", { defaultValue: "缓存" })}{" "}
            {tier.cacheHitRate != null
              ? `${Math.round(tier.cacheHitRate * 100)}%`
              : "—"}
          </span>
          <span className="tabular-nums">
            {t("autoMode.board.balance", { defaultValue: "余额" })}{" "}
            {fmtUsd(tier.balanceUsd, 2, "—")}
          </span>
        </div>
      </div>
      {failed ? (
        <button
          type="button"
          onClick={() =>
            resetBreaker.mutate({ providerId: tier.providerId, appType })
          }
          disabled={resetBreaker.isPending}
          title={t("autoMode.board.reenable", { defaultValue: "重新启用" })}
          aria-label={t("autoMode.board.reenable", {
            defaultValue: "重新启用",
          })}
          className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-50"
        >
          <RefreshCw
            className={cn(
              "h-3.5 w-3.5",
              resetBreaker.isPending && "animate-spin",
            )}
          />
        </button>
      ) : null}
    </div>
  );
}

/**
 * 失败原因徽章：「为什么不选用」的外露标签 —— 熔断（不健康）/降级（有连续
 * 失败但仍健康）二态，点击展开上游报错原文（可复制）。健康且零失败的档位不
 * 显示任何标记（全绿徽章是噪音）；没有原文时只显示徽章不挂 Popover。
 */
function TierHealthBadge({ tier }: { tier: TierBoardTier }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const failures = tier.consecutiveFailures ?? 0;
  const circuitOpen = tier.isHealthy === false;
  if (!circuitOpen && failures === 0) {
    return null;
  }

  const label = circuitOpen
    ? t("health.circuitOpen", { defaultValue: "熔断" })
    : t("health.degraded", { defaultValue: "降级" });
  const badgeClass = cn(
    "inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium leading-none",
    circuitOpen
      ? "border-red-500/60 bg-red-500/10 text-red-600 dark:text-red-400"
      : "border-amber-500/60 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  );

  const handleCopy = async () => {
    if (!tier.lastError) return;
    try {
      await copyText(tier.lastError);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // 复制失败不打扰：原文一直可见，用户可以手动选中。
    }
  };

  if (!tier.lastError) {
    return <span className={badgeClass}>{label}</span>;
  }

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button type="button" className={cn(badgeClass, "cursor-pointer")}>
          {label}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-96 max-w-[80vw]" sideOffset={6}>
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs font-medium text-muted-foreground">
              {label}
            </span>
            <button
              type="button"
              onClick={() => void handleCopy()}
              aria-label={t("common.copy", { defaultValue: "复制" })}
              className="inline-flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
            >
              {copied ? (
                <Check className="h-3.5 w-3.5 text-emerald-500" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
            </button>
          </div>
          <p className="max-h-40 select-text overflow-y-auto break-all font-mono text-xs leading-relaxed">
            {tier.lastError}
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
