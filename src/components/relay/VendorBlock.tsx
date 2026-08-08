import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus } from "lucide-react";
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

import { Button } from "@/components/ui/button";
import type { VendorAccountRow } from "@/lib/api/vendor";

import { parseRowKey, type RowKey, rowKey } from "./rowKey";
import { VendorRow } from "./VendorRow";

/**
 * 「官方 API」块：一行一个官网直连账号（目前唯一厂商是 DeepSeek，将来会有更多）。
 *
 * 2026-08-07 从 `RelayTierList` 拆出来：原来两类行（中转站 / 官网）混在同一个
 * dnd 列表里，现在各归各的区块 —— 中转站行归 `RelayTierList`，官网行走这里。
 *
 * ## 区块头
 *
 * 标题左侧一个 `+` 图标按钮（tooltip 是「添加官网账号」），与中转站块同构。
 * 点它走 `vendor_open_login`（新增账号；同厂商重登会合并回同一行）。
 *
 * ## 为什么标题与按钮文案不带厂商名
 *
 * 将来官方账号不一定只有 DeepSeek —— 这一层是「官网账号」这一**类**的落位，
 * 不是某个厂商专属。文案 vendor 无关，接第二家时不用改 UI。
 *
 * ## 与 `RelayTierList` 的边界
 *
 * 排序、余额、busy 全部沿用 `RelayTierList` 的既有模式：
 * - dnd id 仍是判别式 `RowKey`（`"vendor:3"`），虽然这里只有一个类型，但
 *   `openState` / `balances` 的键与中转站那块的 `RowKey` 同一套，别退回裸 number。
 * - 余额是**已格式化的字符串**（`"¥547.08"`，后端给的），与 relay 那条
 *   `number` 契约有意不同（改那边要动 sub2api）。
 */
export interface VendorListSlice {
  /**
   * 官网直连账号行。**只含官网行** —— 本区块不掺中转站行（拆块前是混列的）。
   */
  accounts: VendorAccountRow[];
  /**
   * 官网行的余额，按 `RowKey` 索引。**值是后端格式化好的字符串**（`"¥547.08"`）——
   * 与 relay 那条 `number` 契约有意分开（改后者要动 sub2api 那半边）。
   */
  balances: Readonly<Record<RowKey, string | null>>;
  /** 某个官网账号的配置是不是当前 tab 正在用的那个。 */
  isCurrent: (rowId: number) => boolean;
  /** 登录 / 重新登录某个官网账号。 */
  onLogin: (rowId: number) => void;
  /** 备好某个官网账号的密钥（也是「刷新密钥」的实现）。 */
  onProvision: (rowId: number) => void;
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
   * 正在进行的操作集合。`"vendorLogin:new"` 命中时 `+` 按钮禁用并转圈
   * （登录窗正在打开，防连点）。
   */
  busy: ReadonlySet<string>;
  /** 「添加官网账号」入口：新增登录（`rowId` 为 null 的那条路）。 */
  onAddVendor: () => void;
}

export function VendorBlock({ vendor, busy, onAddVendor }: VendorBlockProps) {
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
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <h2 className="text-sm font-medium">
            {t("loongport.sections.official")}
          </h2>
          {/* 「添加官网账号」入口。文案 vendor 无关 —— 将来不只是 DeepSeek。
              + 图标跟在标题后面，与「中转站」块同构。 */}
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={onAddVendor}
            title={t("loongport.vendor.add")}
            aria-label={t("loongport.vendor.add")}
            disabled={busy.has("vendorLogin:new")}
          >
            {busy.has("vendorLogin:new") ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Plus className="h-3.5 w-3.5" />
            )}
          </Button>
        </div>
      </div>

      {accounts.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
          {t("loongport.vendor.empty")}
        </p>
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
                  balance={vendor.balances[rowKey("vendor", v.id)] ?? null}
                  isCurrent={vendor.isCurrent(v.id)}
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
