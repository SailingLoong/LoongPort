import { useTranslation } from "react-i18next";
import {
  Check,
  GripVertical,
  Loader2,
  Pencil,
  PencilLine,
  Play,
  Trash2,
  Undo2,
} from "lucide-react";

import type { DraggableAttributes } from "@dnd-kit/core";
import type { SyntheticListenerMap } from "@dnd-kit/core/dist/hooks/utilities";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { VendorAccountRow } from "@/lib/api/vendor";

import { RowBalance } from "./RowBalance";
import { rowKey } from "./rowKey";

/**
 * 一行官网直连账号（DeepSeek 之类），与 `RelayRow` 并列显示在同一个列表里。
 *
 * ## 与 `RelayRow` 的关系：抄视觉，结构上少一层
 *
 * 差异只有三处（其余 className / 间距 / hover 全部逐字抄它，判据是
 * CLAUDE.md §一「和旧页面放一起看不出是两个人写的」）：
 *
 * 1. **不可展开** —— 一个官网账号就一个 endpoint，没有档位层可展开
 *    （所以没有 `Collapsible`、没有箭头、也不需要 `open` / `onOpenChange`）
 * 2. **无档位列表** —— 同上
 * 3. **没有充值按钮** —— relay 那边余额旁边有一个（`relay_purchase`），vendor 侧
 *    **没有对应命令**（官网充值要走厂商自己的收银台，我们没做也不该做）。所以这里
 *    的 `RowBalance` 不传 `onPurchase`，只剩用量条。
 *
 * ## 余额与中转站行**共用**同一个组件（2026-08-13 收敛）
 *
 * 曾经这里是「直接渲染后端拼好的字符串 `"¥547.08"`」，relay 那边是 `number` +
 * `toFixed(2)` —— 同一个事实两套契约。后端统一成 `UsageResult` 之后两边都走
 * `RowBalance`（即 provider 页那条用量条：上次查询时间 + 手动刷新按钮）。
 */
export interface VendorRowProps {
  account: VendorAccountRow;
  /**
   * 正在进行的操作集合。
   *
   * ⚠️ **key 必须带类别**（`login:vendor:3`，见下面 `busyKey`）—— 这个 Set 由
   * 两类行**共享**，而两张表的 id 必然重叠：写成 `login:3` 会让中转站行 3
   * 与官网行 3 的按钮一起转圈。同一个坑、同一个解法（判别式 key）。
   */
  busy: ReadonlySet<string>;
  onLogin: () => void;
  /** 备好密钥（也是「刷新」的实现 —— 本地已有明文时零请求）。 */
  onProvision: () => void | Promise<void>;
  /** 切到这个账号的配置。 */
  onUse: () => void;
  onDelete: () => void;
  /**
   * 编辑**当前 tab 那个平台**的配置（跳 cc-switch 的编辑页）。
   *
   * 一行背后六条 provider 记录，编辑的是当前页那一条 —— 用户要改 Claude 的模型
   * 映射时本来就在 Claude 页。与 relay 档位同一条路（`useTierEditGuard`）。
   */
  onEdit: () => void;
  /** 把当前 tab 那个平台的配置恢复成默认值（密钥保留）。 */
  onReset: () => void;
  /** dnd-kit 的拖动手柄 props（由 `SortableVendorRow` 注入）。 */
  dragHandleProps?: {
    attributes?: DraggableAttributes;
    listeners?: SyntheticListenerMap;
    isDragging?: boolean;
  };
}

/** 逐字抄 `RelayRow` 的那两条（hover 才显形的动作组）。 */
const HOVER_ACTIONS_BASE = "transition-opacity duration-200";
const HOVER_ACTIONS_PINNED = "pointer-events-auto opacity-100";
const ROW_HOVER_ACTIONS =
  "pointer-events-none opacity-0 group-hover/row:pointer-events-auto group-hover/row:opacity-100 group-focus-within/row:pointer-events-auto group-focus-within/row:opacity-100";

/**
 * 这一行的 busy key。**类别段不能省** —— 见 `VendorRowProps.busy`。
 *
 * 复用 `rowKey` 而不是另写一个 `vendor-` 前缀：它就是「防两类行同 id 相撞」，
 * 与 Record 键那几处是同一件事，两套写法会让人以为是两种不同的判别。
 */
export function vendorBusyKey(action: string, id: number): string {
  return `${action}:${rowKey("vendor", id)}`;
}

export function VendorRow({
  account,
  busy,
  onLogin,
  onProvision,
  onUse,
  onDelete,
  onEdit,
  onReset,
  dragHandleProps,
}: VendorRowProps) {
  const { t } = useTranslation();
  const removing = busy.has(vendorBusyKey("removeVendor", account.id));
  const resetting = busy.has(vendorBusyKey("resetVendor", account.id));

  // ⚠️ **`=== true` 而不是 `??` 或直接判真值** —— 三态，照 `RelayRow:636`：
  // `true`（改过）/ `false`（没改）/ `null`（**判不了**：还没 provision、
  // 这个平台不适用、或读不出密钥）。`null` 时什么都不显示 —— 显示「未手动维护」
  // 是在断言「刷新不会覆盖你的改动」，而事实是不知道，让用户误信比不说更糟。
  const userEdited = account.userEdited === true;

  // 编辑与恢复都要读那条 provider 记录 —— 还没 provision 过就没有它。
  // 后端通过 `canEditConfig` 把这个资格传下来，前端不再根据凭据或 provider id 推导。
  const canEditConfig = account.canEditConfig;

  return (
    <div
      className={cn(
        // `group/row` 承接下面的 `group-hover/row:` —— 与 `RelayRow` 同名，
        // 两类行不会嵌套（都是列表的直接子项），所以不必再取第二个名字。
        "group/row rounded-xl border bg-card p-4 text-card-foreground transition-all duration-300",
        dragHandleProps?.isDragging
          ? "cursor-grabbing border-primary shadow-lg"
          : account.isCurrent
            ? // 当前在用的行用蓝框，与 `TierItem` 的当前态同一个 token
              // （`ProviderCard.tsx:306`）—— 用户扫一眼列表首先要找到在用的那个。
              "border-blue-500/60 shadow-sm shadow-blue-500/10"
            : userEdited
              ? // 已手动维护：amber，与 `RelayRow` 的档位行同一套语义
                // （蓝 = 在用、绿 = 代理接管都已有主，amber = 需要留意）。
                // **优先级低于「当前在用」**，同那边的三态排序。
                "border-amber-500/50 bg-amber-500/5 hover:border-amber-500/70"
              : "border-border hover:border-border-active",
      )}
    >
      <div className="flex items-center gap-2">
        {/* 拖动手柄。位置与视觉抄 `RelayRow` —— 用户在同一个列表里
            两类行都看到同一个手柄。**只在官网行之间可拖**，跨类不可
            （两类行的 sort_index 各自存在自己的表里，没有共同的序）。 */}
        <button
          type="button"
          className={cn(
            "-ml-1.5 flex-shrink-0 cursor-grab p-1.5 active:cursor-grabbing",
            "text-muted-foreground/50 transition-colors hover:text-muted-foreground",
            dragHandleProps?.isDragging && "cursor-grabbing",
          )}
          aria-label={t("loongport.row.dragHandle")}
          {...(dragHandleProps?.attributes ?? {})}
          {...(dragHandleProps?.listeners ?? {})}
        >
          <GripVertical className="h-4 w-4" />
        </button>

        {/* 厂商名 + 账号名。**不是折叠触发区**（这一行没有可展开的东西），
            所以是纯文本而不是 `role="button"` —— 做成看起来能点的样子是骗人。
            两行式的布局与 `RelayRow` 一致（站名 / 账号名）。 */}
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <span className="truncate text-sm font-medium">
              {account.vendorName}
            </span>
            {/* 「已手动维护」标记。视觉逐字抄 `RelayRow:673`。
                **常驻不藏进 hover** —— 它是状态不是动作，藏起来用户就得逐行
                hover 才知道哪些配置脱离了自动维护。

                ⚠️ 文案说的是**当前这个 tab 的平台**（`userEdited` 按平台算）——
                同一行在 Claude 页可能有标记、在 Codex 页没有，那是对的。 */}
            {userEdited && (
              <span
                className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-amber-600 ring-1 ring-inset ring-amber-500/30 dark:text-amber-400"
                title={t("loongport.vendor.userEditedHint")}
              >
                <PencilLine className="h-2.5 w-2.5" />
                {t("loongport.tier.userEdited")}
              </span>
            )}
          </div>
          {account.accountLabel && (
            <span className="block truncate text-xs text-muted-foreground">
              {account.accountLabel}
            </span>
          )}
        </div>

        {/* 余额（provider 页那条用量条，与中转站行同一个组件）。
            ⚠️ **判据是「登录过」而不是「登录态还有效」**：sk 是独立凭据，
            登录态过期时后端照样查得到（第 1 步就命中 `api.deepseek.com`）。
            官网行没有充值命令 ⇒ 不传 `onPurchase`。 */}
        <RowBalance
          rowKind="vendor"
          rowId={account.id}
          enabled={account.canQueryBalance}
          onRefresh={account.canRefresh ? onProvision : undefined}
          refreshBusy={busy.has(vendorBusyKey("provision", account.id))}
          refreshLabel={
            account.canRefresh ? t("loongport.vendor.refreshAll") : undefined
          }
        />

        {/* 状态动作（含「使用 / 在用」主按钮）。**放在动作组最前**，对齐
            cc-switch 的「主按钮在左、图标组在右」（`ProviderActions` 内部次序）。 */}
        <VendorStatus
          account={account}
          busy={busy}
          isCurrent={account.isCurrent}
          onLogin={onLogin}
          onProvision={onProvision}
          onUse={onUse}
        />

        {/* 删除。**hover 才出**，与 `RelayRow` 同一条规矩（破坏性动作值得藏）。

            与 relay 行的区别：这里**前端不预先判「在用所以不能删」**，由后端
            `vendor_remove` 拦（它扫六个平台，撞上当前项就报错并点名是哪个）。

            为什么不在前端也判一道：这一行的六个平台**共用同一个 `providerId`**，
            而这个组件只知道当前 tab 的当前项（`isCurrent` 就是这么来的）⇒
            前端判据必然漏掉「在别的平台正被使用」，而那恰恰是最容易撞上的情况。
            与其给一个半对的按钮态（灰不灰都不可信），不如让后端那条点名文案说话。 */}
        <div
          className={cn(
            "flex shrink-0 items-center",
            HOVER_ACTIONS_BASE,
            removing || resetting ? HOVER_ACTIONS_PINNED : ROW_HOVER_ACTIONS,
          )}
        >
          {/* 「编辑配置」—— 跳 cc-switch 的编辑页（含事前警告，见
              `useTierEditGuard`）。视觉抄 `RelayRow:790` 那个。 */}
          {canEditConfig && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
              onClick={onEdit}
              title={t("loongport.tier.edit")}
            >
              <Pencil className="h-3.5 w-3.5" />
            </Button>
          )}

          {/* 「恢复默认配置」的 hover 版，给**没手动维护过**的那些。
              手动维护过的走下面常驻的那个 —— 两处互斥（`!userEdited` / `userEdited`），
              不会同时出现两个恢复按钮。同 `RelayRow:797` 的分法。 */}
          {canEditConfig && !userEdited && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
              disabled={resetting}
              onClick={onReset}
              title={t("loongport.tier.resetConfig")}
            >
              {resetting ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Undo2 className="h-3.5 w-3.5" />
              )}
            </Button>
          )}

          {account.canDelete && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className={cn(
                "h-7 w-7 shrink-0 p-1 text-muted-foreground",
                removing
                  ? "cursor-not-allowed opacity-40"
                  : "hover:text-red-500 dark:hover:text-red-400",
              )}
              onClick={removing ? undefined : onDelete}
              title={t("loongport.vendor.remove")}
            >
              {removing ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Trash2 className="h-3.5 w-3.5" />
              )}
            </Button>
          )}
        </div>

        {/* 「恢复默认配置」的**常驻**版，只给已手动维护的那些。
            理由同 `RelayRow:825` 那段：对普通行它是兜底（没改过，没什么可恢复），
            而对手动维护过的行，它是那个状态**唯一的出口** —— 用户改坏了配置要退回
            默认值，不该先猜「hover 一下会不会冒出个按钮」。 */}
        {canEditConfig && userEdited && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 shrink-0 p-1 text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-400 dark:hover:text-amber-300"
            disabled={resetting}
            onClick={onReset}
            title={t("loongport.vendor.userEditedHint")}
          >
            {resetting ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Undo2 className="h-3.5 w-3.5" />
            )}
          </Button>
        )}
      </div>
    </div>
  );
}

/**
 * 行右侧的状态与动作。状态由后端 DTO 计算，前端只按状态展示对应动作：
 *
 * | 后端 `status` | 显示 | 为什么不能混 |
 * |---|---|---|
 * | `ready` | 使用 / 在用 + 更多 | 后端确认凭据和配置可用 |
 * | `sessionExpiredUsable` | 使用 / 在用 + 更多 | 登录态过期，但后端确认 SK 仍可用 |
 * | `sessionExpired` | 「登录已过期」+ 重新登录 | 预填已就绪，用户只需补验证 |
 * | `notLoggedIn` | 「还没登录」+ 登录 | 从没登录过 |
 * | `noKey` | 「获取密钥」 | 登录了但还没备好 SK |
 *
 * 登录态过期后余额仍优先用 SK 查询；「重新登录」入口保留为 hover 次要动作，
 * 供需要更新网页登录信息的用户使用，但不把一个仍可用的账号渲染成故障态。
 */
function VendorStatus({
  account,
  busy,
  isCurrent,
  onLogin,
  onProvision,
  onUse,
}: {
  account: VendorAccountRow;
  busy: ReadonlySet<string>;
  isCurrent: boolean;
  onLogin: () => void;
  onProvision: () => void | Promise<void>;
  onUse: () => void;
}) {
  const { t } = useTranslation();
  const loggingIn = busy.has(vendorBusyKey("login", account.id));
  const provisioning = busy.has(vendorBusyKey("provision", account.id));
  const switching = busy.has(vendorBusyKey("switch", account.id));

  if (account.status === "ready" || account.status === "sessionExpiredUsable") {
    return (
      <div className="flex shrink-0 items-center gap-2">
        {/* 主按钮（「使用 / 在用」）+ 次要动作，**整组 hover 才出** —— 与
            `TierItem:712` / cc-switch `ProviderCard` 同一形态：没 hover 时
            右侧是空的，「在用」态靠行上的蓝色边框表达。进行中钉住可见。 */}
        <div
          className={cn(
            "flex shrink-0 items-center gap-0.5",
            HOVER_ACTIONS_BASE,
            switching || provisioning || loggingIn
              ? HOVER_ACTIONS_PINNED
              : ROW_HOVER_ACTIONS,
          )}
        >
          {/* 主按钮。**文案与图标复用上游的 `provider.enable` / `provider.inUse`**
              （四 locale 早就齐了）—— 与 `TierItem` 里那两支逐字相同，
              否则同一个操作会在两种行上有两种叫法。 */}
          {isCurrent ? (
            <Button
              type="button"
              size="sm"
              variant="secondary"
              className="h-7 shrink-0 cursor-not-allowed bg-gray-200 text-muted-foreground hover:bg-gray-200 hover:text-muted-foreground dark:bg-gray-700 dark:hover:bg-gray-700"
              disabled
            >
              <Check className="mr-1 h-3.5 w-3.5" />
              {t("provider.inUse")}
            </Button>
          ) : account.canSwitch ? (
            <Button
              type="button"
              size="sm"
              className="h-7 shrink-0"
              disabled={switching}
              onClick={onUse}
            >
              {switching ? (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Play className="mr-1 h-3.5 w-3.5" />
              )}
              {switching ? t("loongport.tier.switching") : t("provider.enable")}
            </Button>
          ) : null}

          {/* 登录态过期但 sk 还在用的行：给一条重登的路，但**不催**。
              唯一的实际损失是余额拉不到，title 就说这件事。 */}
          {account.status === "sessionExpiredUsable" && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 gap-1 px-2 text-muted-foreground"
              disabled={loggingIn}
              onClick={onLogin}
              title={t("loongport.row.sessionExpiredUsable")}
            >
              {loggingIn ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                t("loongport.row.reLogin")
              )}
            </Button>
          )}
        </div>
      </div>
    );
  }

  if (account.status === "sessionExpired") {
    return (
      <StatusAction
        hint={t("loongport.row.sessionExpired")}
        label={t("loongport.row.reLogin")}
        loading={loggingIn}
        onClick={onLogin}
      />
    );
  }

  if (account.status === "notLoggedIn") {
    return (
      <StatusAction
        hint={t("loongport.row.notLoggedIn")}
        label={t("loongport.row.login")}
        loading={loggingIn}
        onClick={onLogin}
      />
    );
  }

  return (
    <StatusAction
      hint={t("loongport.vendor.noKey")}
      label={t("loongport.row.getKeys")}
      loading={provisioning}
      onClick={onProvision}
    />
  );
}

/** 与 `RelayRow` 里那个同形（hint 文字 + outline 按钮）。 */
function StatusAction({
  hint,
  label,
  loading,
  onClick,
}: {
  hint: string;
  label: string;
  loading: boolean;
  onClick: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2">
      <span className="text-xs text-muted-foreground">{hint}</span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="h-7"
        disabled={loading}
        onClick={onClick}
      >
        {loading && <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />}
        {label}
      </Button>
    </div>
  );
}
