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
  ArrowUpDown,
  Check,
  GripVertical,
  Settings2,
} from "lucide-react";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type { ApplicationRoutingTier } from "@/lib/api/applicationRouting";
import type { AccountRoute } from "@/components/shell/navigation";
import { Button } from "@/components/ui/button";
import { useAccountVisibility } from "@/components/relay/accounts/useAccountVisibility";
import { cn } from "@/lib/utils";
import { tierMetrics, type TierMetric, type TierSort } from "./tierMetrics";

interface Props {
  configurations: ApplicationConfiguration[];
  tiers: ApplicationRoutingTier[];
  orderedIds: string[];
  search: string;
  additive: boolean;
  busy: boolean;
  orderBusy: boolean;
  sort: TierSort | null;
  onSort: (metric: TierMetric) => void;
  onReorder: (ids: string[]) => void;
  onSelect: (item: ApplicationConfiguration) => void;
  onOpenAccount: (account: AccountRoute) => void;
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
  const needle = props.search.trim().toLocaleLowerCase();
  const visible = props.orderedIds.flatMap((id, index) => {
    const item = configurations.get(id);
    return item &&
      (!needle ||
        [
          item.name,
          item.serviceName,
          item.accountLabel,
          item.configurationName,
          item.model,
        ].some((value) => value?.toLocaleLowerCase().includes(needle)))
      ? [{ item, priority: index + 1 }]
      : [];
  });
  const dragDisabled = props.orderBusy || Boolean(needle);
  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={({ active, over }) => {
        if (dragDisabled || !over || active.id === over.id) return;
        const from = props.orderedIds.indexOf(String(active.id)),
          to = props.orderedIds.indexOf(String(over.id));
        if (from >= 0 && to >= 0)
          props.onReorder(arrayMove(props.orderedIds, from, to));
      }}
    >
      <div className="overflow-x-auto rounded-xl border border-border bg-card">
        <table
          className="w-full border-collapse text-sm"
          aria-label={t("applications.availableTiers")}
        >
          <thead className="bg-muted/50 text-xs text-muted-foreground">
            <tr>
              <th
                scope="col"
                className="min-w-24 px-3 py-3 text-left font-medium"
              >
                {t("applications.priority")}
              </th>
              <th
                scope="col"
                className="min-w-52 px-3 py-3 text-left font-medium"
              >
                {t("applications.tier")}
              </th>
              {tierMetrics.map((metric) => {
                const active = props.sort?.key === metric.key;
                const Icon = active
                  ? props.sort?.descending
                    ? ArrowDown
                    : ArrowUp
                  : ArrowUpDown;
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
                      <Icon className="h-3 w-3" />
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
              {visible.map(({ item, priority }) => (
                <TierRow
                  key={item.providerId}
                  item={item}
                  priority={priority}
                  tier={metrics.get(item.providerId)}
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
  current,
  additive,
  dragDisabled,
  busy,
  onSelect,
  onOpenAccount,
}: {
  item: ApplicationConfiguration;
  priority: number;
  tier?: ApplicationRoutingTier;
  current: boolean;
  additive: boolean;
  dragDisabled: boolean;
  busy: boolean;
  onSelect: Props["onSelect"];
  onOpenAccount: Props["onOpenAccount"];
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
  const name = item.configurationName ?? item.name;
  const { isAccountDetailsHidden } = useAccountVisibility();
  const accountHidden = item.account
    ? isAccountDetailsHidden(item.account)
    : false;
  return (
    <tr
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn(
        "group border-t border-border/60",
        current ? "bg-blue-500/5" : "hover:bg-muted/30",
        isDragging && "relative z-20 bg-card shadow-lg",
      )}
    >
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
          <span className="tabular-nums text-muted-foreground">{priority}</span>
        </div>
      </td>
      <td className="max-w-80 px-3 py-3">
        <div className="flex min-h-6 flex-wrap items-center gap-2">
          <span className="break-words font-medium">{name}</span>
          {current && (
            <span className="inline-flex items-center gap-1 whitespace-nowrap rounded bg-blue-500/10 px-1.5 py-0.5 text-xs text-blue-600 dark:text-blue-400">
              <Check className="h-3 w-3" />
              {t(additive ? "applications.enabled" : "applications.configured")}
            </span>
          )}
          {item.presentation.isDefaultModel && (
            <span className="text-xs text-muted-foreground">
              {t("applications.default")}
            </span>
          )}
        </div>
        <p className="break-words text-xs leading-5 text-muted-foreground">
          {[item.serviceName, accountHidden ? null : item.accountLabel]
            .filter(Boolean)
            .join(" · ") || t(`applications.sources.${item.source}`)}
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
          <span className="inline-block py-1.5">
            {(accountHidden && metric.key === "balanceUsd") ||
            tier?.[metric.key] == null ? (
              <span className="text-muted-foreground">—</span>
            ) : (
              metric.format(tier[metric.key]!)
            )}
          </span>
        </td>
      ))}
      <td className="sticky right-0 bg-card px-3 py-3 text-right align-top">
        <div className="flex justify-end gap-1">
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
          <Button
            size="sm"
            variant={current ? "outline" : "default"}
            disabled={busy || !item.canSelect}
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
