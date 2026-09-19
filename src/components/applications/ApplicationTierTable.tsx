import { useTranslation } from "react-i18next";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowDown,
  ArrowUp,
  Ban,
  Check,
  GripVertical,
  RotateCcw,
  Settings2,
} from "lucide-react";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type {
  ApplicationRoutingTier,
  SubscriptionWindow,
} from "@/lib/api/applicationRouting";
import { formatResetAt } from "./tierMetrics";
import type { AccountRoute } from "@/components/shell/navigation";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { TierVerifyButton } from "@/components/relay/model-verification/TierVerifyButton";
import { TierVerdictChip } from "@/components/relay/model-verification/TierVerdictChip";
import { useTierVerification } from "@/components/relay/model-verification/TierVerificationProvider";
import { tierMetrics, type TierMetric, type TierSort } from "./tierMetrics";

/**
 * 订阅窗口的多行 tooltip 文案（native title）：主列只显示最早的那个，
 * 全量窗口（限额/已用/各自的重置）hover 可见——主显示做减法、细节有去处。
 */
function subscriptionWindowsTitle(
  t: (key: string) => string,
  windows: SubscriptionWindow[],
): string {
  return windows
    .map((window) => {
      const kind = t(`applications.windowKind.${window.kind}`);
      const used =
        window.usedUsd == null
          ? ""
          : ` · ${window.usedUsd.toFixed(2)}/${window.limitUsd.toFixed(2)}`;
      const reset =
        window.resetAt == null ? "" : ` · ${formatResetAt(window.resetAt)}`;
      return `${kind}${used}${reset}`;
    })
    .join("\\n");
}

/**
 * 可见行之间换位、未显示行原位不动（splice 语义）：筛选视图里拖拽只改
 * 可见档位的相对顺序，隐藏档位各守其位。`from`/`to` 是可见序列里的下标。
 */
export function reorderWithinVisible(
  orderedIds: string[],
  visibleIds: string[],
  from: number,
  to: number,
): string[] {
  if (from < 0 || to < 0 || from === to) return orderedIds;
  const reorderedVisible = arrayMove(visibleIds, from, to);
  const visibleSet = new Set(visibleIds);
  let cursor = 0;
  return orderedIds.map((id) =>
    visibleSet.has(id) ? reorderedVisible[cursor++]! : id,
  );
}

/**
 * 可见性唯源：筛选（账号/模型）+ 搜索对显示序的过滤。表格渲染与工作台的
 * 「应用此顺序」目标计算共用这一个判定——两处各写一份必然分叉。
 * 屏蔽不在这层：它是链资格语义，目标计算另行排除，表格另行置灰。
 */
export function visibleTierIds({
  orderedIds,
  configurations,
  metrics,
  search,
  accountFilter,
  modelFilter,
}: {
  orderedIds: string[];
  configurations: Map<string, ApplicationConfiguration>;
  metrics: Map<string, ApplicationRoutingTier>;
  search: string;
  accountFilter: string | null;
  modelFilter: string | null;
}): string[] {
  const needle = search.trim().toLocaleLowerCase();
  return orderedIds.filter((id) => {
    const item = configurations.get(id);
    if (!item) return false;
    if (
      accountFilter &&
      (!item.account ||
        `${item.account.kind}:${item.account.id}` !== accountFilter)
    ) {
      return false;
    }
    if (modelFilter) {
      // 模型筛选命中「分组支持」：目录含它，或当前正在用它（目录可能滞后于
      // 实际回显）。无目录的档位回落单模型（effectiveModel）语义。
      const tier = metrics.get(id);
      const effective = tier?.effectiveModel ?? item.model;
      const supports =
        (tier?.models.includes(modelFilter) ?? false) ||
        effective === modelFilter;
      if (!supports) return false;
    }
    if (
      needle &&
      ![
        item.name,
        item.serviceName,
        item.accountLabel,
        item.configurationName,
        item.model,
      ].some((value) => value?.toLocaleLowerCase().includes(needle))
    ) {
      return false;
    }
    return true;
  });
}

interface Props {
  configurations: ApplicationConfiguration[];
  tiers: ApplicationRoutingTier[];
  orderedIds: string[];
  /** 故障切换开启才出现优先级列与屏蔽按钮（2026-09-16 用户定调的三隐边界）。 */
  failoverEnabled: boolean;
  /** 有未应用的顺序草稿：优先级数字着 amber（所见=草稿，与状态条同语义）。 */
  orderPending: boolean;
  search: string;
  accountFilter: string | null;
  modelFilter: string | null;
  additive: boolean;
  busy: boolean;
  orderBusy: boolean;
  sort: TierSort | null;
  onSort: (metric: TierMetric) => void;
  onReorder: (ids: string[]) => void;
  onSelect: (item: ApplicationConfiguration) => void;
  onOpenAccount: (account: AccountRoute) => void;
  onBlockTier: (providerId: string, blocked: boolean) => void;
  onResetTierErrors: (providerId: string) => void;
}
export function ApplicationTierTable(props: Props) {
  const { t } = useTranslation();
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
  const configurations = new Map(
    props.configurations.map((item) => [item.providerId, item]),
  );
  const metrics = new Map(props.tiers.map((tier) => [tier.providerId, tier]));
  const visible = visibleTierIds({
    orderedIds: props.orderedIds,
    configurations,
    metrics,
    search: props.search,
    accountFilter: props.accountFilter,
    modelFilter: props.modelFilter,
  }).flatMap((id) => {
    const item = configurations.get(id);
    return item ? [item] : [];
  });
  const isBlocked = (id: string) => metrics.get(id)?.skipReason === "blocked";
  // 优先级 = 「应用此顺序」目标的序号（可见 ∧ 未屏蔽，按显示序）：永远从 1 起、
  // 按列表顺序连续编号，不断档不逆序；被屏蔽的行不占号，下一行顶上——
  // 应用写入的就是这串编号对应的 id 序（2026-09-16 定调）。
  let rank = 0;
  const visibleRank = new Map(
    visible.map((item) => {
      const blocked = isBlocked(item.providerId);
      return [item.providerId, blocked ? null : ++rank] as const;
    }),
  );
  // 拖拽只在指标排序期间禁用：排序是临时视图序，与拖拽打架；筛选是稳定子集，
  // 可见行之间换位（未显示行原位不动）。
  const dragDisabled = props.orderBusy || Boolean(props.sort);
  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={({ active, over }) => {
        if (dragDisabled || !over || active.id === over.id) return;
        const visibleIds = visible.map((item) => item.providerId);
        props.onReorder(
          reorderWithinVisible(
            props.orderedIds,
            visibleIds,
            visibleIds.indexOf(String(active.id)),
            visibleIds.indexOf(String(over.id)),
          ),
        );
      }}
    >
      <div className="overflow-x-auto rounded-xl border border-border bg-card">
        <table
          className="w-full border-collapse text-sm"
          aria-label={t("applications.availableTiers")}
        >
          <thead className="bg-muted/50 text-xs text-muted-foreground">
            <tr>
              {props.failoverEnabled && (
                <th
                  scope="col"
                  className="min-w-24 px-3 py-3 text-left font-medium"
                >
                  {t("applications.priority")}
                </th>
              )}
              <th
                scope="col"
                className="min-w-52 px-3 py-3 text-left font-medium"
              >
                {t("applications.tier")}
              </th>
              {tierMetrics.map((metric) => {
                const active = props.sort?.key === metric.key;
                // 只有当前排序的指标常驻箭头；未排序的指标不摆任何方向符号
                //（2026-09-15 用户定调：默认不展示，激活才常驻）。
                const Icon = props.sort?.descending ? ArrowDown : ArrowUp;
                return (
                  <th
                    key={metric.key}
                    scope="col"
                    className="whitespace-nowrap px-2 py-3 text-right font-medium"
                    aria-sort={
                      active
                        ? props.sort?.descending
                          ? "descending"
                          : "ascending"
                        : "none"
                    }
                  >
                    <button
                      type="button"
                      className="inline-flex items-center gap-1 rounded p-1 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                      aria-label={t(`applications.metrics.${metric.key}`)}
                      disabled={props.orderBusy}
                      onClick={() => props.onSort(metric.key)}
                    >
                      {t(`applications.metrics.${metric.key}`)}
                      {active && <Icon className="h-3 w-3" />}
                    </button>
                  </th>
                );
              })}
              <th
                scope="col"
                className="sticky right-0 bg-muted px-3 py-3 text-right font-medium"
              >
                {t("applications.action")}
              </th>
            </tr>
          </thead>
          <SortableContext
            items={props.orderedIds}
            strategy={verticalListSortingStrategy}
          >
            <tbody>
              {visible.map((item) => (
                <TierRow
                  key={item.providerId}
                  item={item}
                  priority={visibleRank.get(item.providerId) ?? null}
                  tier={metrics.get(item.providerId)}
                  failoverEnabled={props.failoverEnabled}
                  orderPending={props.orderPending}
                  onResetTierErrors={(providerId) =>
                    props.onResetTierErrors(providerId)
                  }
                  additive={props.additive}
                  current={
                    props.additive
                      ? item.presentation.isInConfig
                      : item.presentation.isCurrent
                  }
                  dragDisabled={dragDisabled}
                  busy={props.busy}
                  onSelect={props.onSelect}
                  onOpenAccount={props.onOpenAccount}
                  onBlockTier={props.onBlockTier}
                />
              ))}
            </tbody>
          </SortableContext>
        </table>
        {visible.length === 0 && (
          <p className="p-10 text-center text-sm text-muted-foreground">
            {t("applications.noMatches")}
          </p>
        )}
      </div>
    </DndContext>
  );
}
function TierRow({
  item,
  priority,
  tier,
  failoverEnabled,
  orderPending,
  current,
  additive,
  dragDisabled,
  busy,
  onSelect,
  onOpenAccount,
  onBlockTier,
  onResetTierErrors,
}: {
  item: ApplicationConfiguration;
  /** 列表位置号；null = 被屏蔽，不占号（下一行顶上）。 */
  priority: number | null;
  tier?: ApplicationRoutingTier;
  failoverEnabled: boolean;
  orderPending: boolean;
  current: boolean;
  additive: boolean;
  dragDisabled: boolean;
  busy: boolean;
  onSelect: Props["onSelect"];
  onOpenAccount: Props["onOpenAccount"];
  onBlockTier: Props["onBlockTier"];
  onResetTierErrors: Props["onResetTierErrors"];
}) {
  const { t } = useTranslation();
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: item.providerId, disabled: dragDisabled });
  // 验证进行中也是「进行中的操作」：动作组钉住，转圈不因移开鼠标而消失。
  const verifying = useTierVerification().isVerifying(item.providerId);
  const name = item.configurationName ?? item.name;
  const blocked = tier?.skipReason === "blocked";
  return (
    <tr
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn(
        "group border-t border-border/60",
        current ? "bg-blue-500/5" : "hover:bg-muted/30",
        // 屏蔽的档位整行置灰：自动切换永不选它，但仍在列表里，可筛选、可手动切换。
        blocked && "opacity-55",
        isDragging && "relative z-20 bg-card shadow-lg",
      )}
    >
      {failoverEnabled && (
        <td className="px-3 py-3 align-top">
          <div className="flex h-8 items-center gap-3">
            <button
              type="button"
              {...attributes}
              {...listeners}
              disabled={dragDisabled}
              aria-label={t("applications.dragTier", { name })}
              className="cursor-grab rounded p-1 text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring active:cursor-grabbing disabled:cursor-default disabled:opacity-30"
            >
              <GripVertical className="h-4 w-4" />
            </button>
            {/* 挂起草稿时数字着 amber：这串号是「将会应用的序」，还没生效
                （与上方状态条同语义；屏蔽行的「—」保持中性灰）。 */}
            <span
              className={cn(
                "tabular-nums",
                orderPending && priority != null
                  ? "text-amber-600 dark:text-amber-400"
                  : "text-muted-foreground",
              )}
            >
              {priority ?? "—"}
            </span>
          </div>
        </td>
      )}
      <td className="max-w-80 px-3 py-3">
        <div className="flex min-h-6 flex-wrap items-center gap-2">
          <span className="break-words font-medium">{name}</span>
          {current && (
            <span className="inline-flex items-center gap-1 whitespace-nowrap rounded bg-blue-500/10 px-1.5 py-0.5 text-xs text-blue-600 dark:text-blue-400">
              <Check className="h-3 w-3" />
              {t(additive ? "applications.enabled" : "applications.configured")}
            </span>
          )}
          {/* 验真结论 chip：模块自持（下线/无结论时不渲染）。 */}
          <TierVerdictChip providerId={item.providerId} />
          {item.presentation.isDefaultModel && (
            <span className="text-xs text-muted-foreground">
              {t("applications.default")}
            </span>
          )}
        </div>
        <p className="break-words text-xs leading-5 text-muted-foreground">
          {[item.serviceName, item.accountLabel].filter(Boolean).join(" · ") ||
            t(`applications.sources.${item.source}`)}
        </p>
        {(tier?.effectiveModel ?? item.model) && (
          <p className="break-all text-xs leading-5 text-muted-foreground">
            {tier?.effectiveModel ?? item.model}
          </p>
        )}
        {tier?.skipReason && (
          <p className="mt-1 break-words text-xs leading-5 text-amber-700 dark:text-amber-400">
            {t(`applications.skipReasons.${tier.skipReason}`, {
              defaultValue: tier.skipReason,
            })}
          </p>
        )}
        {tier?.lastError && (
          <p
            className="mt-1 line-clamp-2 break-words text-xs leading-5 text-amber-700 dark:text-amber-400"
            title={tier.lastError}
          >
            {t("applications.lastError")}: {tier.lastError}
          </p>
        )}
      </td>
      {tierMetrics.map((metric) => (
        <td
          key={metric.key}
          className="whitespace-nowrap px-3 py-3 text-right align-top tabular-nums"
        >
          <span
            className="inline-block py-1.5"
            {...(metric.key === "nextResetAt" &&
            tier?.subscriptionWindows?.length
              ? { title: subscriptionWindowsTitle(t, tier.subscriptionWindows) }
              : {})}
          >
            {tier?.[metric.key] == null ? (
              <span className="text-muted-foreground">—</span>
            ) : (
              metric.format(tier[metric.key]!)
            )}
          </span>
        </td>
      ))}
      <td className="sticky right-0 bg-card px-3 py-3 text-right align-top">
        {/* 动作组 hover / focus 才显形 —— 信息常驻、动作按需出现（2026-09-13 用户定调：
            档位多时每行一个蓝色按钮全是噪音）。当前行状态由「当前」徽章与行底色表达，
            不需要常驻按钮。进行中的切换/验证钉住可见；触屏无 hover 常显；
            `pointer-events-none` 不能省（透明按钮仍然可点）。 */}
        <div
          className={cn(
            "flex justify-end gap-1 transition-opacity duration-200",
            busy || verifying
              ? "pointer-events-auto opacity-100 [@media(hover:none)]:opacity-100"
              : "pointer-events-none opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 [@media(hover:none)]:pointer-events-auto [@media(hover:none)]:opacity-100",
          )}
        >
          {item.account && (
            <Button
              size="icon"
              variant="ghost"
              className="h-8 w-8"
              aria-label={t("applications.manageAccount")}
              title={t("applications.manageAccount")}
              onClick={() => item.account && onOpenAccount(item.account)}
            >
              <Settings2 className="h-3.5 w-3.5" />
            </Button>
          )}
          {failoverEnabled && (
            <Button
              size="icon"
              variant="ghost"
              className="h-8 w-8"
              disabled={busy}
              aria-label={t(
                blocked ? "applications.unblockTier" : "applications.blockTier",
              )}
              title={t(
                blocked ? "applications.unblockTier" : "applications.blockTier",
              )}
              onClick={() => onBlockTier(item.providerId, !blocked)}
            >
              <Ban className="h-3.5 w-3.5" />
            </Button>
          )}
          {/* 清除错误记录（用户显式动作）：熔断器与健康行一起置空，立刻重新
              参与选路——错误只跳过、位置永不动（2026-09-19 用户定调）。 */}
          {(tier?.skipReason === "circuit_open" ||
            tier?.lastError ||
            (tier?.consecutiveFailures ?? 0) > 0) && (
            <Button
              size="icon"
              variant="ghost"
              className="h-8 w-8"
              aria-label={t("applications.resetTierErrors")}
              title={t("applications.resetTierErrors")}
              onClick={() => onResetTierErrors(item.providerId)}
            >
              <RotateCcw className="h-3.5 w-3.5" />
            </Button>
          )}
          {/* 模型验证入口：模块自持（下线/档位不可验证时不渲染）。 */}
          <TierVerifyButton
            tier={{ providerId: item.providerId, displayName: name }}
            canVerify={tier?.canVerifyModels ?? false}
          />
          <Button
            size="sm"
            variant={current ? "outline" : "default"}
            disabled={busy || !item.canSelect || (!additive && current)}
            aria-label={`${t(additive ? "applications.enable" : "applications.use")} ${name}`}
            onClick={() => onSelect(item)}
          >
            {t(additive ? "applications.enable" : "applications.use")}
          </Button>
        </div>
      </td>
    </tr>
  );
}
