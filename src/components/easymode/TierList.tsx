/**
 * 省心看板的档位列表：自动模式平铺、手动模式可拖拽排序。
 *
 * dnd 形状抄 `RelayTierList`（同一套 sensors/strategy/把手注入）；卡片只展示
 * 后端看板算好的事实（顺序/倍率/单价/余额/耗时/命中），不在前端拼业务判据。
 */
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
import { GripVertical } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { TierBoardTier } from "@/lib/api/autoMode";
import type { DraggableAttributes } from "@dnd-kit/core";
import type { SyntheticListenerMap } from "@dnd-kit/core/dist/hooks/utilities";

export function TierList({
  tiers,
  manual,
  onReorder,
}: {
  tiers: TierBoardTier[];
  manual: boolean;
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
          <TierCard key={tier.providerId} tier={tier} />
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
            <SortableTierCard key={tier.providerId} tier={tier} />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}

function SortableTierCard({ tier }: { tier: TierBoardTier }) {
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
      <TierCard tier={tier} dragHandleProps={{ attributes, listeners }} />
    </div>
  );
}

/** 档位卡：序号 + 名称 + 当前命中 + 倍率/单价/余额/首字耗时。未知值显示 —/「价格未知」。 */
function TierCard({
  tier,
  dragHandleProps,
}: {
  tier: TierBoardTier;
  dragHandleProps?: {
    attributes?: DraggableAttributes;
    listeners?: SyntheticListenerMap;
  };
}) {
  const { t } = useTranslation();
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
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <span className="tabular-nums">
            ×
            {tier.rateMultiplier ??
              t("autoMode.board.unknown", { defaultValue: "?" })}
          </span>
          <span className="tabular-nums">
            {tier.unitPricePerMillion != null
              ? `$${tier.unitPricePerMillion.toFixed(2)}/M`
              : t("autoMode.board.priceUnknown", { defaultValue: "价格未知" })}
          </span>
          <span className="tabular-nums">
            {t("autoMode.board.balance", { defaultValue: "余额" })}{" "}
            {tier.balanceUsd != null ? `$${tier.balanceUsd.toFixed(2)}` : "—"}
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
        </div>
      </div>
    </div>
  );
}
