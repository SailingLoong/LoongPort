import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import {
  DndContext,
  closestCenter,
  useSensor,
  useSensors,
  PointerSensor,
  KeyboardSensor,
} from "@dnd-kit/core";
import type { DragEndEvent } from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import type { VendorAccountRow } from "@/lib/api/vendor";

import { parseRowKey, rowKey } from "./rowKey";
import { VendorRow } from "./VendorRow";

/**
 * 「官方 API」块：一行一个官网直连账号（目前唯一厂商是 DeepSeek，将来会有更多）。
 *
 * 2026-08-07 从 `RelayTierList` 拆出来：原来两类行（中转站 / 官网）混在同一个
 * dnd 列表里，现在各归各的区块 —— 中转站行归 `RelayTierList`，官网行走这里。
 *
 * 区块头只有标题；「添加官网账号」入口在顶栏大「+」（`AddEntryMenu`）。
 *
 * ## 为什么标题文案不带厂商名
 *
 * 将来官方账号不一定只有 DeepSeek —— 这一层是「官网账号」这一**类**的落位，
 * 不是某个厂商专属。文案 vendor 无关，接第二家时不用改 UI。
 *
 * ## 与 `RelayTierList` 的边界
 *
 * 排序、余额、busy 全部沿用 `RelayTierList` 的既有模式：
 * - dnd id 仍是判别式 `RowKey`（`"vendor:3"`），虽然这里只有一个类型，但
 *   `openState` 的键与中转站那块的 `RowKey` 同一套，别退回裸 number。
 * - 余额是**已格式化的字符串**（`"¥547.08"`，后端给的），与 relay 那条
 *   `number` 契约有意不同（改那边要动 sub2api）。
 */
export interface VendorListSlice {
  /**
   * 官网直连账号行。**只含官网行** —— 本区块不掺中转站行（拆块前是混列的）。
   */
  accounts: VendorAccountRow[];
  /** 登录 / 重新登录某个官网账号。 */
  onLogin: (rowId: number) => void;
  /** 备好某个官网账号的密钥（也是「刷新密钥」的实现）。 */
  onProvision: (rowId: number) => void | Promise<void>;
  /** 切到某个官网账号的配置。 */
  onUse: (rowId: number) => void;
  onRemove: (rowId: number) => void;
  /**
   * 编辑某个官网账号在**当前 tab 那个平台**上的配置。
   *
   * 传整行而不是 rowId：宿主要拿 `providerId` / `accountLabel` / `isCurrent`
   * 去喂 `useTierEditGuard`（它吃 `EditableTier`），只给 id 还得再 find 回来。
   */
  onEdit: (account: VendorAccountRow) => void;
  /** 把某个官网账号在当前 tab 那个平台上的配置恢复成默认值。 */
  onReset: (account: VendorAccountRow) => void;
  /** 官网行的排序（**只含官网行的 id**，走另一条命令）。 */
  onReorder: (rowIds: number[]) => void;
}

export interface VendorBlockProps {
  /** 官网直连行那半边。 */
  vendor: VendorListSlice;
  /**
   * 正在进行的操作集合。行内登录等动作的 busy 判定用
   * （`"vendorLogin:<rowId>"` 等）。
   */
  busy: ReadonlySet<string>;
  /** 空态占位（「尚未添加服务商账号」虚框）整块的点击动作：跳聚合页「官方 API」。 */
  onAddAccount: () => void;
}

export function VendorBlock({ vendor, busy, onAddAccount }: VendorBlockProps) {
  const { t } = useTranslation();
  const accounts = vendor.accounts;

  // 与 `RelayTierList` 同一个 sensor 配置 —— 视觉与手感一致。
  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  /**
   * 拖动结束：只重排官网行（本区块只有这一类）。
   *
   * dnd id 仍是 `RowKey` 字符串（`"vendor:3"`）—— 用 `parseRowKey` 取回数字 id。
   * 立刻落库（`vendor_reorder`），排序不是纯 UI 状态。
   */
  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const from = parseRowKey(String(active.id));
      const to = parseRowKey(String(over.id));
      const ids = accounts.map((v) => v.id);
      const fromIdx = ids.indexOf(from.id);
      const toIdx = ids.indexOf(to.id);
      if (fromIdx < 0 || toIdx < 0) return;
      const next = arrayMove(ids, fromIdx, toIdx);
      vendor.onReorder(next);
    },
    [accounts, vendor],
  );

  return (
    <div className="space-y-3">
      <h2 className="text-sm font-medium">
        {t("loongport.sections.official")}
      </h2>

      {accounts.length === 0 ? (
        // 空态占位整块是碰撞框：点一下直达聚合页「官方 API」标签。
        <button
          type="button"
          onClick={onAddAccount}
          title={t("loongport.officialApi.title")}
          aria-label={t("loongport.officialApi.title")}
          className="w-full rounded-xl border border-dashed border-border p-6 text-center text-sm text-muted-foreground transition-colors hover:border-blue-400/60 hover:bg-muted/40 hover:text-foreground"
        >
          {t("loongport.vendor.empty")}
        </button>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={accounts.map((v) => rowKey("vendor", v.id))}
            strategy={verticalListSortingStrategy}
          >
            <div className="space-y-3">
              {accounts.map((v) => (
                <SortableVendorRow
                  key={rowKey("vendor", v.id)}
                  account={v}
                  busy={busy}
                  onLogin={() => vendor.onLogin(v.id)}
                  onProvision={() => vendor.onProvision(v.id)}
                  onUse={() => vendor.onUse(v.id)}
                  onDelete={() => vendor.onRemove(v.id)}
                  onEdit={() => vendor.onEdit(v)}
                  onReset={() => vendor.onReset(v)}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      )}
    </div>
  );
}

/** 给 `VendorRow` 套一层 dnd-kit 的 sortable 壳（与 `RelayTierList` 里那个逐字同形）。 */
function SortableVendorRow(
  props: Omit<React.ComponentProps<typeof VendorRow>, "dragHandleProps">,
) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: rowKey("vendor", props.account.id) });

  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={isDragging ? "z-10" : undefined}
    >
      <VendorRow
        {...props}
        dragHandleProps={{ attributes, listeners, isDragging }}
      />
    </div>
  );
}
