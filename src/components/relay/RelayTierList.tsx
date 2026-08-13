import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";
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
import type { RelayRow as RelayRowData, TierInfo } from "@/lib/api/relay";
import type { VerificationVerdict } from "@/lib/api/modelVerification";

import { RelayRow } from "./RelayRow";
import { parseRowKey, type RowKey, rowKey } from "./rowKey";

/**
 * 「中转站」块：一行一个中转站，点击折叠/展开，展开后档位排在它下面。
 *
 * 2026-08-07 起**只含中转站行** —— 官网直连行拆去了 `VendorBlock`（官方 API 块）。
 * 两个区块各归各的 dnd 列表，不再混列。
 *
 * ## 区块头
 *
 * 标题左侧一个 `+` 图标按钮（tooltip 是「添加中转站」）。全量刷新属于
 * `RelaySection` 页面级动作，因为它同时同步中转站与官方 API，不放在任一区块里。
 *
 * ## dnd id 仍是判别式 `RowKey`
 *
 * 块内虽然只有一类行了，但 `openState` 的键、dnd 的 `items` 与
 * `useSortable` 的 id 一路用的都是 `rowKey("relay", id)` —— 保持同形，
 * 别退回裸 number（`parseRowKey` 也因此还用得上）。
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
 * 纯 UI 偏好，不值得进 schema。key 带中转站 id（`loongport:collapsed:<id>`）而不是
 * 数组下标 —— 否则「折叠了第一个站」在站点顺序变化后会错位到别的站。
 */
export interface RelayTierListProps {
  relays: RelayRowData[];
  /**
   * 正在进行的操作集合（`"provision:3"` / `"switch:<providerId>"` / `"refresh:all"`）。
   *
   * **是集合不是单个字符串** —— 中转站之间没有依赖，A 在获取密钥时 B / C 的按钮
   * 不该跟着灰掉。见 `useRowBusy` 的文档。
   */
  busy: ReadonlySet<string>;
  onAddSite: () => void;
  /** 行内动作，**都显式带 relayId**（后端命令直接吃它，不再靠全局「当前站」）。 */
  onLogin: (relayId: number) => void;
  onProvision: (relayId: number) => void | Promise<void>;
  onSwitchTier: (relayId: number, tier: TierInfo) => void;
  onSelectTierModel: (tier: TierInfo, model: string) => void;
  /** 拖动结束后的新顺序（完整 id 序列，下标即 sort_index）。 */
  onReorder: (relayIds: number[]) => void;
  /**
   * 带登录态开某一行的充值页。
   *
   * ⚠️ **余额本身不再从这里穿下去**：它由每一行自己的 `useRowBalanceQuery` 拉
   * （见 `RowBalance`）。曾经这里有个 `balances: Record<RowKey, number|null>`，
   * 由 `RelaySection` 的一个 effect 填 —— 那条路失败一次就再也不重试，见
   * `RowBalance` 的文档。
   */
  onPurchase: (relayId: number) => void;
  /** 检测某个档位的连通性。 */
  onCheckTier: (tier: TierInfo) => void;
  /** 某个档位是不是正在检测中。 */
  isCheckingTier: (providerId: string) => boolean;
  /** 档位行的模型验证呈现输入；结果和运行状态仍由 RelaySection 持有。 */
  verificationVerdictForTier: (
    tier: TierInfo,
  ) => VerificationVerdict | undefined;
  onVerifyTier?: (tier: TierInfo) => void;
  isVerifyingTier: (providerId: string) => boolean;
  /**
   * 把某个档位的配置恢复成默认值（改坏之后的回头路）。
   *
   * 入口是 hover 才出现的小按钮；**已手动维护的档位上它常驻** —— 那种档位
   * 随时可能要退回默认值，见 `RelayRow` 里的说明。
   */
  onResetTier: (tier: TierInfo) => void;
  /** 编辑某个档位的配置（跳 cc-switch 的编辑页）。宿主负责先弹事前警告。 */
  onEditTier: (tier: TierInfo) => void;
  /** 删掉某一行（连带它名下的托管档位）。有档位在用的行不会触发它（按钮不可点）。 */
  onRemoveRelay: (relayId: number) => void;
}

/** localStorage 的 key 前缀。值为 `"1"`（折叠）/ `"0"`（展开），不存 = 没表态过。 */
const COLLAPSED_KEY_PREFIX = "loongport:collapsed:";

function collapsedKey(relayId: number): string {
  return `${COLLAPSED_KEY_PREFIX}${relayId}`;
}

/**
 * 读某一行存过的折叠偏好。
 *
 * 返回 `null` 表示**从没表态过**（与「表态为展开」是两件事）——
 * 前者按默认规则（当前档位所在的行展开），后者尊重用户的选择。
 */
function readCollapsed(relayId: number): boolean | null {
  try {
    const raw = localStorage.getItem(collapsedKey(relayId));
    return raw === null ? null : raw === "1";
  } catch {
    // localStorage 可能被禁用（隐私模式）。折叠偏好丢了不影响功能，静默回落默认规则。
    return null;
  }
}

function writeCollapsed(relayId: number, collapsed: boolean): void {
  try {
    localStorage.setItem(collapsedKey(relayId), collapsed ? "1" : "0");
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
function initialOpenState(relays: RelayRowData[]): Record<RowKey, boolean> {
  const state: Record<RowKey, boolean> = {};
  for (const op of relays) {
    // 键是判别式 RowKey（与 `VendorBlock` 那块的键同一套命名，分块后也不会撞）。
    // localStorage 的键仍按 relay id 存（那份偏好只有中转站行有）。
    const key = rowKey("relay", op.id);
    const stored = readCollapsed(op.id);
    if (stored !== null) {
      state[key] = !stored;
      continue;
    }
    state[key] = op.tiers.some((tier) => tier.isCurrent);
  }
  return state;
}

export function RelayTierList({
  relays,
  busy,
  onAddSite,
  onLogin,
  onProvision,
  onSwitchTier,
  onSelectTierModel,
  onReorder,
  onPurchase,
  onCheckTier,
  isCheckingTier,
  verificationVerdictForTier,
  onVerifyTier,
  isVerifyingTier,
  onResetTier,
  onEditTier,
  onRemoveRelay,
}: RelayTierListProps) {
  const { t } = useTranslation();
  // 与 `useDragSort`（ProviderList 用的那个）同样的 sensor 配置 —— 视觉与手感一致。
  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  /**
   * 拖动结束：只重排中转站行（本区块只有这一类）。
   *
   * dnd id 仍是 `RowKey` 字符串（`"relay:3"`）—— 用 `parseRowKey` 取回数字 id。
   * 立刻落库 —— 排序不是纯 UI 状态，换台机器/重开 app 都该记得。
   * 不做乐观更新：父组件 refresh 后列表就是新序，多一次渲染而已。
   */
  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const from = parseRowKey(String(active.id));
      const to = parseRowKey(String(over.id));
      const ids = relays.map((op) => op.id);
      const fromIdx = ids.indexOf(from.id);
      const toIdx = ids.indexOf(to.id);
      if (fromIdx < 0 || toIdx < 0) return;

      onReorder(arrayMove(ids, fromIdx, toIdx));
    },
    [relays, onReorder],
  );

  // 只认 id 集合的变化，不认整个 relays 数组 —— 后者每次 refresh 都是新对象，
  // 会让展开态在每次切换档位后被重算（用户刚展开的行又合回去）。
  const idsKey = useMemo(() => relays.map((op) => op.id).join(","), [relays]);

  // 初始值是空对象而不是 `initialOpenState(relays)`：首次挂载时 `relays`
  // 必然还是空数组（父组件的 refresh 是异步的），算了也是空的。真正的初始化在下面
  // 那个 useEffect 里 —— idsKey 从 "" 变成 "1,2" 时跑。
  const [openState, setOpenState] = useState<Record<RowKey, boolean>>({});

  useEffect(() => {
    setOpenState((prev) => {
      const next = initialOpenState(relays);
      // 已经在界面上的行**保留当前展开态**：用户可能刚手动展开了某行，
      // 而这次 refresh 只是因为别的行状态变了（如另一个站登录成功）。
      // 新出现的行走 initialOpenState 的默认规则。
      for (const op of relays) {
        const key = rowKey("relay", op.id);
        if (key in prev) next[key] = prev[key];
      }
      return next;
    });
    // **依赖是 idsKey 不是 relays**：后者每次 refresh 都是新对象引用，
    // 会让这个 effect 每次都跑 —— 那本身无害（上面保留了 prev），
    // 但配合下面 setOpenState 的写法会多一次无谓渲染。
    // 函数体里读的 relays 是本次渲染的那个，与 idsKey 同源，不会读到旧值。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  const handleOpenChange = useCallback((relayId: number, open: boolean) => {
    setOpenState((prev) => ({
      ...prev,
      [rowKey("relay", relayId)]: open,
    }));
    writeCollapsed(relayId, !open);
  }, []);

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-1.5">
        <h2 className="text-sm font-medium">{t("loongport.sections.relay")}</h2>
        {/* 「添加中转站」入口。与「官方 API」块同构：+ 图标跟在标题后面。 */}
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-6 w-6"
          onClick={onAddSite}
          title={t("loongport.tierList.addSite")}
          aria-label={t("loongport.tierList.addSite")}
        >
          <Plus className="h-3.5 w-3.5" />
        </Button>
      </div>

      {relays.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
          {t("loongport.tierList.empty")}
        </p>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          {/* dnd 的 id 仍是判别式 `RowKey` 字符串（`"relay:3"`）——
              与 `openState` 的键同形，别退回裸 number。 */}
          <SortableContext
            items={relays.map((op) => rowKey("relay", op.id))}
            strategy={verticalListSortingStrategy}
          >
            <div className="space-y-3">
              {relays.map((op) => (
                <SortableRelayRow
                  key={rowKey("relay", op.id)}
                  relay={op}
                  open={openState[rowKey("relay", op.id)] ?? false}
                  onOpenChange={(open) => handleOpenChange(op.id, open)}
                  busy={busy}
                  onLogin={() => onLogin(op.id)}
                  onProvision={() => onProvision(op.id)}
                  onSwitchTier={(tier) => onSwitchTier(op.id, tier)}
                  onSelectTierModel={onSelectTierModel}
                  onPurchase={() => onPurchase(op.id)}
                  onCheckTier={onCheckTier}
                  isCheckingTier={isCheckingTier}
                  verificationVerdictForTier={verificationVerdictForTier}
                  onVerifyTier={onVerifyTier}
                  isVerifyingTier={isVerifyingTier}
                  onResetTier={onResetTier}
                  onEditTier={onEditTier}
                  onDelete={() => onRemoveRelay(op.id)}
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
 * 给 `RelayRow` 套一层 dnd-kit 的 sortable 壳。
 *
 * 拖动手柄的 props 透传给行组件，由它渲染在最左侧 —— 与 `ProviderCard` 的
 * `dragHandleProps` 同一个形状（`GripVertical` + `cursor-grab`），
 * 判据仍是「和旧页面放一起看不出是两个人写的」。
 */
function SortableRelayRow(
  props: Omit<React.ComponentProps<typeof RelayRow>, "dragHandleProps"> & {
    relay: RelayRowData;
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
  } = useSortable({ id: rowKey("relay", props.relay.id) });

  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={isDragging ? "z-10" : undefined}
    >
      <RelayRow
        {...props}
        dragHandleProps={{ attributes, listeners, isDragging }}
      />
    </div>
  );
}
