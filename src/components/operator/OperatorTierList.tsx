import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus, RefreshCw } from "lucide-react";
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
import type {
  OperatorRow as OperatorRowData,
  TierInfo,
} from "@/lib/api/operator";
import type { VendorAccountRow } from "@/lib/api/vendor";

import { OperatorRow } from "./OperatorRow";
import { parseRowKey, type RowKey, rowKey } from "./rowKey";
import { VendorRow } from "./VendorRow";

/**
 * 「运营商 × 分组」列表：一行一个运营商，点击折叠/展开，展开后档位排在它下面。
 *
 * ## 两类行混在一个列表里（2026-08-04 加）
 *
 * 中转站行（`OperatorRow`，可展开、有档位）与官网直连行（`VendorRow`，一个 endpoint、
 * 不可展开）**并列显示**，官网行排在中转站行之后。
 *
 * ⚠️ **两张表的自增 id 都从 1 起、必然重叠**，所以凡是「会同时装两类行」的地方
 * （React `key`、dnd-kit 的 `items`、`openState` / `balances` 的 Record 键）
 * 一律用判别式的 `RowKey`（`"operator:3"` / `"vendor:3"`），不用裸 number ——
 * 撞了会让官网行的余额显示到同 id 的中转站行上，且没有任何报错。
 *
 * 只在 operator 内部流转的 id（`onLogin(op.id)` 传出去、回调里 find 回来）
 * **仍是 number**：那些不参与跨类索引。
 *
 * ## 排序：两类各自内部可拖，跨类不可
 *
 * 两类行的 `sort_index` 各自存在自己的表里，本来就没有一个共同的序
 * （spec §6.2 已裁决）。所以拖动结果按类别分派给两条 reorder 命令，
 * 跨类拖动直接忽略。
 *
 * ## props 比 `ProviderList` 少（它有 19 个）
 *
 * 托管档位**没有**档位级的拖拽排序、没有 failover、没有删除 —— 那些能力对托管项
 * 语义上不存在（档位由 provision 生成与回收，不由用户增删）。
 *
 * ⚠️ **「编辑」是例外，它有**：2026-08-05 起托管档位与官网直连行都能手工编辑配置
 * （走 `useTierEditGuard` → cc-switch 的编辑页），编辑过的显示「已手动维护」并可
 * 一键恢复默认。所以别再照「托管项不可编辑」那条老假设加拦截。
 *
 * ## 折叠状态存 localStorage，不进 DB
 *
 * 纯 UI 偏好，不值得进 schema。key 带运营商 id（`loongport:collapsed:<id>`）而不是
 * 数组下标 —— 否则「折叠了第一个站」在站点顺序变化后会错位到别的站。
 */
/**
 * 官网直连行那半边的全部数据与动作。
 *
 * ## 为什么收成一个对象而不是 8 个平铺 prop
 *
 * 这 8 项要么全给要么全不给（给一半的列表是坏的）—— 那就是一个单位而不是 8 个
 * 独立选项。收成一个对象让类型层面就说得清这件事，平铺 8 个 prop 说不清。
 */
export interface VendorListSlice {
  /**
   * 官网直连账号行。**排在中转站行之后**，两类行在同一个 dnd 上下文里但不可跨类拖。
   */
  accounts: VendorAccountRow[];
  /**
   * 官网行的余额，按 `RowKey` 索引。**值是后端格式化好的字符串**（`"¥547.08"`）——
   * 与 operator 那条 `number` 契约有意分开（改后者要动 sub2api 那半边）。
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

export interface OperatorTierListProps {
  operators: OperatorRowData[];
  /** 官网直连行那半边。 */
  vendor: VendorListSlice;
  /**
   * 正在进行的操作集合（`"provision:3"` / `"switch:<providerId>"` / `"refresh:all"`）。
   *
   * **是集合不是单个字符串** —— 运营商之间没有依赖，A 在获取密钥时 B / C 的按钮
   * 不该跟着灰掉。见 `useRowBusy` 的文档。
   */
  busy: ReadonlySet<string>;
  onAddSite: () => void;
  /** 顶部「刷新」：对所有已登录运营商重新拉分组（不只是刷倍率）。 */
  onRefresh: () => void;
  /** 行内动作，**都显式带 operatorId**（后端命令直接吃它，不再靠全局「当前站」）。 */
  onLogin: (operatorId: number) => void;
  onProvision: (operatorId: number) => void;
  onSwitchTier: (operatorId: number, tier: TierInfo) => void;
  /** 拖动结束后的新顺序（完整 id 序列，下标即 sort_index）。 */
  onReorder: (operatorIds: number[]) => void;
  /**
   * 各行的余额，**按 `RowKey` 索引**（`"operator:3"`）。缺键 / `null` = 还没拉到
   * 或拉失败，那一行不显示余额。
   *
   * **是 map 而不是塞进 `OperatorRow`**：`listOperators` 的契约是「只读本地不发网络」，
   * 而余额必须发请求 —— 混进去会破坏那条契约（首屏就得等网络）。
   * 与倍率同一个模式：先渲染，再异步填。
   *
   * ⚠️ **官网行的余额不在这里** —— 它是后端格式化好的 `string`（`"¥547.08"`），
   * 与这条 `number` 契约不同（那边前端不做任何格式化）。走 `vendorBalances`。
   */
  balances: Readonly<Record<RowKey, number | null>>;
  /** 点某一行的余额 → 带登录态开那一行的充值页。 */
  onPurchase: (operatorId: number) => void;
  /** 检测某个档位的连通性。 */
  onCheckTier: (tier: TierInfo) => void;
  /** 某个档位是不是正在检测中。 */
  isCheckingTier: (providerId: string) => boolean;
  /**
   * 把某个档位的配置恢复成默认值（改坏之后的回头路）。
   *
   * 入口是 hover 才出现的小按钮；**已手动维护的档位上它常驻** —— 那种档位
   * 随时可能要退回默认值，见 `OperatorRow` 里的说明。
   */
  onResetTier: (tier: TierInfo) => void;
  /** 编辑某个档位的配置（跳 cc-switch 的编辑页）。宿主负责先弹事前警告。 */
  onEditTier: (tier: TierInfo) => void;
  /** 删掉某一行（连带它名下的托管档位）。有档位在用的行不会触发它（按钮不可点）。 */
  onRemoveOperator: (operatorId: number) => void;
}

/** localStorage 的 key 前缀。值为 `"1"`（折叠）/ `"0"`（展开），不存 = 没表态过。 */
const COLLAPSED_KEY_PREFIX = "loongport:collapsed:";

function collapsedKey(operatorId: number): string {
  return `${COLLAPSED_KEY_PREFIX}${operatorId}`;
}

/**
 * 读某一行存过的折叠偏好。
 *
 * 返回 `null` 表示**从没表态过**（与「表态为展开」是两件事）——
 * 前者按默认规则（当前档位所在的行展开），后者尊重用户的选择。
 */
function readCollapsed(operatorId: number): boolean | null {
  try {
    const raw = localStorage.getItem(collapsedKey(operatorId));
    return raw === null ? null : raw === "1";
  } catch {
    // localStorage 可能被禁用（隐私模式）。折叠偏好丢了不影响功能，静默回落默认规则。
    return null;
  }
}

function writeCollapsed(operatorId: number, collapsed: boolean): void {
  try {
    localStorage.setItem(collapsedKey(operatorId), collapsed ? "1" : "0");
  } catch {
    // 同上：存不进去只是下次不记得，不该让切换档位这个主流程报错。
  }
}

/**
 * 算出每一行的展开态。
 *
 * **默认规则只在用户没表态过时生效**：当前正在用的档位所在那行展开，其余折叠。
 * 用户一旦表过态（哪怕把所有行都折叠了）就完全尊重他 —— 切换档位不重新展开
 * （spec §四 明确要求区分「没存过」与「显式折叠了」）。
 */
function initialOpenState(
  operators: OperatorRowData[],
): Record<RowKey, boolean> {
  const state: Record<RowKey, boolean> = {};
  for (const op of operators) {
    // 键是判别式 RowKey：这个 Record 与列表里的两类行共存，
    // 裸 number 会让官网行 3 的存在影响中转站行 3 的展开态。
    // localStorage 的键仍按 operator id 存（那份偏好只有中转站行有）。
    const key = rowKey("operator", op.id);
    const stored = readCollapsed(op.id);
    if (stored !== null) {
      state[key] = !stored;
      continue;
    }
    state[key] = op.tiers.some((tier) => tier.isCurrent);
  }
  return state;
}

export function OperatorTierList({
  operators,
  vendor,
  busy,
  onAddSite,
  onRefresh,
  onLogin,
  onProvision,
  onSwitchTier,
  onReorder,
  balances,
  onPurchase,
  onCheckTier,
  isCheckingTier,
  onResetTier,
  onEditTier,
  onRemoveOperator,
}: OperatorTierListProps) {
  const { t } = useTranslation();
  const refreshing = busy.has("refresh:all");
  const vendors = vendor.accounts;

  // 与 `useDragSort`（ProviderList 用的那个）同样的 sensor 配置 —— 视觉与手感一致。
  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  /**
   * 拖动结束。**两类行各自内部排序，跨类拖动忽略。**
   *
   * dnd-kit 的 id 现在是 `RowKey` 字符串（`"vendor:3"`）而不是裸 number ——
   * 后者在两类行混列时会撞（两张表的 id 都从 1 起）。所以先解析出类别：
   * 不同类就直接返回（不是「移动到那个位置」而是「这个操作没有意义」——
   * 两类行的 `sort_index` 存在不同的表里，没有一个共同的序可写）。
   */
  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const from = parseRowKey(String(active.id));
      const to = parseRowKey(String(over.id));
      if (from.kind !== to.kind) return;

      const ids =
        from.kind === "operator"
          ? operators.map((op) => op.id)
          : vendors.map((v) => v.id);
      const fromIdx = ids.indexOf(from.id);
      const toIdx = ids.indexOf(to.id);
      if (fromIdx < 0 || toIdx < 0) return;

      // 立刻落库 —— 排序不是纯 UI 状态，换台机器/重开 app 都该记得。
      // 不做乐观更新：父组件 refresh 后列表就是新序，多一次渲染而已。
      const next = arrayMove(ids, fromIdx, toIdx);
      if (from.kind === "operator") onReorder(next);
      else vendor.onReorder(next);
    },
    [operators, vendors, onReorder, vendor],
  );

  // 只认 id 集合的变化，不认整个 operators 数组 —— 后者每次 refresh 都是新对象，
  // 会让展开态在每次切换档位后被重算（用户刚展开的行又合回去）。
  const idsKey = useMemo(
    () => operators.map((op) => op.id).join(","),
    [operators],
  );

  // 初始值是空对象而不是 `initialOpenState(operators)`：首次挂载时 `operators`
  // 必然还是空数组（父组件的 refresh 是异步的），算了也是空的。真正的初始化在下面
  // 那个 useEffect 里 —— idsKey 从 "" 变成 "1,2" 时跑。
  const [openState, setOpenState] = useState<Record<RowKey, boolean>>({});

  useEffect(() => {
    setOpenState((prev) => {
      const next = initialOpenState(operators);
      // 已经在界面上的行**保留当前展开态**：用户可能刚手动展开了某行，
      // 而这次 refresh 只是因为别的行状态变了（如另一个站登录成功）。
      // 新出现的行走 initialOpenState 的默认规则。
      for (const op of operators) {
        const key = rowKey("operator", op.id);
        if (key in prev) next[key] = prev[key];
      }
      return next;
    });
    // **依赖是 idsKey 不是 operators**：后者每次 refresh 都是新对象引用，
    // 会让这个 effect 每次都跑 —— 那本身无害（上面保留了 prev），
    // 但配合下面 setOpenState 的写法会多一次无谓渲染。
    // 函数体里读的 operators 是本次渲染的那个，与 idsKey 同源，不会读到旧值。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  const handleOpenChange = useCallback((operatorId: number, open: boolean) => {
    setOpenState((prev) => ({
      ...prev,
      [rowKey("operator", operatorId)]: open,
    }));
    writeCollapsed(operatorId, !open);
  }, []);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium">{t("loongport.tierList.title")}</h2>
        <div className="flex items-center gap-2">
          {/* 全局刷新会对每个已登录运营商各发一轮请求，所以它自己进行中时要禁用
              （防连点）；但**别的行的操作不该禁它**，也不该被它禁 ——
              运营商之间无依赖。 */}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 gap-1"
            disabled={refreshing}
            onClick={onRefresh}
          >
            {refreshing ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RefreshCw className="h-3.5 w-3.5" />
            )}
            {t("loongport.tierList.refresh")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 gap-1"
            onClick={onAddSite}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("loongport.tierList.addSite")}
          </Button>
        </div>
      </div>

      {operators.length === 0 && vendors.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
          {t("loongport.tierList.empty")}
        </p>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          {/* dnd 的 id 是**判别式 RowKey 字符串**，不是裸 number ——
              两类行混在一个 SortableContext 里，裸 number 会撞
              （两张表的自增 id 都从 1 起）。跨类拖动由 `handleDragEnd` 忽略。 */}
          <SortableContext
            items={[
              ...operators.map((op) => rowKey("operator", op.id)),
              ...vendors.map((v) => rowKey("vendor", v.id)),
            ]}
            strategy={verticalListSortingStrategy}
          >
            <div className="space-y-3">
              {operators.map((op) => (
                <SortableOperatorRow
                  key={rowKey("operator", op.id)}
                  operator={op}
                  open={openState[rowKey("operator", op.id)] ?? false}
                  onOpenChange={(open) => handleOpenChange(op.id, open)}
                  busy={busy}
                  onLogin={() => onLogin(op.id)}
                  onProvision={() => onProvision(op.id)}
                  onSwitchTier={(tier) => onSwitchTier(op.id, tier)}
                  // `?? null` 而不是 `?? undefined`：缺键与「拉失败」在 UI 上是
                  // 同一件事（都不显示余额），统一成 null 让行组件只判一种。
                  balance={balances[rowKey("operator", op.id)] ?? null}
                  onPurchase={() => onPurchase(op.id)}
                  onCheckTier={onCheckTier}
                  isCheckingTier={isCheckingTier}
                  onResetTier={onResetTier}
                  onEditTier={onEditTier}
                  onDelete={() => onRemoveOperator(op.id)}
                />
              ))}
              {/* 官网直连行排在中转站行**之后**：中转站是主线（有档位、有倍率、
                  用户日常在这一层选），官网账号是补充的一条直连路径。 */}
              {vendors.map((v) => (
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

/**
 * 给 `OperatorRow` 套一层 dnd-kit 的 sortable 壳。
 *
 * 拖动手柄的 props 透传给行组件，由它渲染在最左侧 —— 与 `ProviderCard` 的
 * `dragHandleProps` 同一个形状（`GripVertical` + `cursor-grab`），
 * 判据仍是「和旧页面放一起看不出是两个人写的」。
 */
function SortableOperatorRow(
  props: Omit<React.ComponentProps<typeof OperatorRow>, "dragHandleProps"> & {
    operator: OperatorRowData;
  },
) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
    // **id 是 RowKey 不是 op.id** —— 与 `SortableContext.items` 必须同形，
    // 否则 dnd-kit 找不到这一项（拖动直接失效，且不报错）。
  } = useSortable({ id: rowKey("operator", props.operator.id) });

  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={isDragging ? "z-10" : undefined}
    >
      <OperatorRow
        {...props}
        dragHandleProps={{ attributes, listeners, isDragging }}
      />
    </div>
  );
}

/** 给 `VendorRow` 套同一层 sortable 壳（与上面那个逐字同形）。 */
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
