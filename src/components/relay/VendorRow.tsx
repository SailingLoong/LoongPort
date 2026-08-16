import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronDown,
  ChevronRight,
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

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { VendorAccountRow, VendorPlanInfo } from "@/lib/api/vendor";

import { RowBalance } from "./RowBalance";
import { rowKey } from "./rowKey";

/**
 * 一行官网直连账号，与 `RelayRow` 并列显示在同一个列表里。
 *
 * ## 与 `RelayRow` 的关系：抄视觉；档位层**按 plan 数量出现**
 *
 * - **单 plan 厂商**（DeepSeek / BigModel）：与旧版逐字同形 —— 扁平行、无箭头、
 *   「使用 / 在用」在行右侧（`plans.length === 1` 的分支）。
 * - **多 plan 厂商**（opencode Zen / Go）：抄 `RelayRow` 的形状 —— 行头是账号
 *   （厂商名 + 账号名，整片可折叠），展开后一行一个 plan（`PlanItem`，抄
 *   `TierItem` 的视觉 token）。
 *
 * 其余差异（无充值按钮等）与旧版相同，见 git 历史。
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
   * ⚠️ **key 必须带类别**（`login:vendor:3`，见下面 `vendorBusyKey`）—— 这个 Set 由
   * 两类行**共享**，而两张表的 id 必然重叠：写成 `login:3` 会让中转站行 3
   * 与官网行 3 的按钮一起转圈。同一个坑、同一个解法（判别式 key）。
   */
  busy: ReadonlySet<string>;
  onLogin: () => void;
  /** 备好密钥（也是「刷新」的实现 —— 本地已有明文时零请求）。 */
  onProvision: () => void | Promise<void>;
  /** 切到某个 plan 的配置。单 plan 行也会被调用（传 `plans[0]`）。 */
  onUse: (plan: VendorPlanInfo) => void;
  onDelete: () => void;
  /**
   * 编辑某个 plan 在**当前 tab 那个平台**上的配置（跳 cc-switch 的编辑页）。
   *
   * 一行背后每 plan 六条 provider 记录，编辑的是当前页那一条 —— 用户要改 Claude
   * 的模型映射时本来就在 Claude 页。与 relay 档位同一条路（`useTierEditGuard`）。
   */
  onEdit: (plan: VendorPlanInfo) => void;
  /** 把某个 plan 在当前 tab 那个平台的配置恢复成默认值（密钥保留）。 */
  onReset: (plan: VendorPlanInfo) => void;
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
/** plan 子行的 hover 组 —— `group/tier` 版（嵌在行里，别被外层 hover 一起点亮）。 */
const PLAN_HOVER_ACTIONS =
  "pointer-events-none opacity-0 group-hover/tier:pointer-events-auto group-hover/tier:opacity-100 group-focus-within/tier:pointer-events-auto group-focus-within/tier:opacity-100";

/**
 * 这一行的 busy key。**类别段不能省** —— 见 `VendorRowProps.busy`。
 *
 * 复用 `rowKey` 而不是另写一个 `vendor-` 前缀：它就是「防两类行同 id 相撞」，
 * 与 Record 键那几处是同一件事，两套写法会让人以为是两种不同的判别。
 */
export function vendorBusyKey(action: string, id: number): string {
  return `${action}:${rowKey("vendor", id)}`;
}

/**
 * plan 档位的 busy key。**按 providerId 判别** —— 多 plan 行里两个档位各自转圈，
 * 与 relay 档位的 `switch:${providerId}` 同一条思路（托管 id 全局唯一，不会撞）。
 */
export function vendorPlanBusyKey(action: string, providerId: string): string {
  return `${action}:vplan:${providerId}`;
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
  // 单 plan 行的编辑 / 恢复挂行级（多 plan 行挂档位子行）。
  const singlePlan = account.plans.length === 1 ? account.plans[0] : null;
  // 多 plan 行的开合由本组件持有 —— 纯前端展示偏好（同 `RelayRow` 的 open
  // 语义），不进后端。默认展开「有档位在用」的行：用户扫一眼就能看到在用的是哪档。
  const [open, setOpen] = useState(
    () =>
      account.plans.length > 1 && account.plans.some((plan) => plan.isCurrent),
  );

  // 行级事实由 plan 派生（展示层对后端事实的纯归并，不是前端重新计算业务判据）：
  // 任一 plan 在用 ⇒ 行亮蓝；任一 plan 被手改 ⇒ 行亮 amber。
  const isCurrent = account.plans.some((plan) => plan.isCurrent);
  const userEdited = account.plans.some((plan) => plan.userEdited === true);

  const body = (
    <div
      className={cn(
        // `group/row` 承接下面的 `group-hover/row:` —— 与 `RelayRow` 同名，
        // 两类行不会嵌套（都是列表的直接子项），所以不必再取第二个名字。
        "group/row rounded-xl border bg-card p-4 text-card-foreground transition-all duration-300",
        dragHandleProps?.isDragging
          ? "cursor-grabbing border-primary shadow-lg"
          : isCurrent
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

        {/* 厂商名 + 账号名。多 plan 行的这片**是折叠触发区**（抄 `RelayRow`：
            箭头 + 名字整片可点，不用瞄准小箭头）；单 plan 行没有可展开的东西，
            是纯文本而不是 `role="button"` —— 做成看起来能点的样子是骗人。 */}
        <AccountHeader account={account} open={open} />

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

        {/* 状态动作（登录 / 获取密钥；「使用」按钮只在单 plan 行挂行级，
            多 plan 行的使用在档位子行里）。**放在动作组最前**，对齐
            cc-switch 的「主按钮在左、图标组在右」。 */}
        <VendorStatus
          account={account}
          busy={busy}
          // 单 plan 行才有行级「使用 / 在用」；多 plan 行传 null 隐藏它。
          plan={singlePlan}
          onLogin={onLogin}
          onProvision={onProvision}
          onUse={() => onUse(account.plans[0])}
        />

        {/* 删除。**hover 才出**，与 `RelayRow` 同一条规矩（破坏性动作值得藏）。

            与 relay 行的区别：这里**前端不预先判「在用所以不能删」**，由后端
            `vendor_remove` 拦（它按全部 plan 扫六个平台，撞上当前项就报错并点名）。

            为什么不在前端也判一道：多 plan 行的 providerId 不止一个，而这个组件
            只知道当前 tab 的当前项 ⇒ 前端判据必然漏掉「在别的平台 / 别的 plan
            正被使用」，而那恰恰是最容易撞上的情况。与其给一个半对的按钮态，
            不如让后端那条点名文案说话。 */}
        <div
          className={cn(
            "flex shrink-0 items-center",
            HOVER_ACTIONS_BASE,
            removing ? HOVER_ACTIONS_PINNED : ROW_HOVER_ACTIONS,
          )}
        >
          {/* 单 plan 行：编辑 / 恢复在行级（多 plan 行的这两个在档位子行里）。 */}
          {singlePlan && (
            <EditResetActions
              plan={singlePlan}
              busy={busy}
              onEdit={onEdit}
              onReset={onReset}
            />
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
      </div>

      {/* plan 子行（多 plan 厂商才有；由下面的 Collapsible 分支渲染）。 */}
      {account.plans.length > 1 && (
        <CollapsibleContent className="space-y-2 pt-3">
          {account.plans.map((plan) => (
            <PlanItem
              key={plan.providerId}
              plan={plan}
              busy={busy}
              onUse={() => onUse(plan)}
              onEdit={() => onEdit(plan)}
              onReset={() => onReset(plan)}
            />
          ))}
        </CollapsibleContent>
      )}
    </div>
  );

  if (account.plans.length <= 1) {
    return body;
  }
  // 多 plan：整行可折叠（抄 `RelayRow` 的 Collapsible 壳）。
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      {body}
    </Collapsible>
  );
}

/** 账号名片区：多 plan 行是折叠触发区，单 plan 行是纯文本。
 *
 * 开合走 Radix Collapsible 的 context（`Collapsible` 是受控的，Trigger 会自己
 * 调 `onOpenChange`），这里只消费 `open` 渲染箭头与 aria。
 */
function AccountHeader({
  account,
  open,
}: {
  account: VendorAccountRow;
  open: boolean;
}) {
  const { t } = useTranslation();
  const nameBlock = (
    <span className="min-w-0 flex-1">
      <span className="block truncate text-sm font-medium">
        {account.vendorName}
      </span>
      {account.accountLabel && (
        <span className="block truncate text-xs text-muted-foreground">
          {account.accountLabel}
        </span>
      )}
    </span>
  );

  if (account.plans.length <= 1) {
    return <div className="min-w-0 flex-1">{nameBlock}</div>;
  }
  return (
    <CollapsibleTrigger asChild>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-label={
          open ? t("loongport.row.collapse") : t("loongport.row.expand")
        }
        className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md py-0.5 transition-colors hover:bg-muted/40"
      >
        <span className="shrink-0 text-muted-foreground">
          {open ? (
            <ChevronDown className="h-4 w-4" />
          ) : (
            <ChevronRight className="h-4 w-4" />
          )}
        </span>
        {nameBlock}
      </div>
    </CollapsibleTrigger>
  );
}

/**
 * 行级（单 plan）的编辑 + 恢复默认动作：编辑在 hover 组，恢复按是否手改过
 * 分 hover / 常驻两处（同 `PlanItem` / 旧版 `VendorRow` 的分法 —— 手改过的行
 * 要有不需 hover 就能点到的恢复出口）。
 */
function EditResetActions({
  plan,
  busy,
  onEdit,
  onReset,
}: {
  plan: VendorPlanInfo;
  busy: ReadonlySet<string>;
  onEdit: (plan: VendorPlanInfo) => void;
  onReset: (plan: VendorPlanInfo) => void;
}) {
  const { t } = useTranslation();
  const resetting = busy.has(vendorPlanBusyKey("resetVendor", plan.providerId));
  const userEdited = plan.userEdited === true;

  return (
    <>
      {plan.canEditConfig && (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
          onClick={() => onEdit(plan)}
          title={t("loongport.tier.edit")}
        >
          <Pencil className="h-3.5 w-3.5" />
        </Button>
      )}
      {plan.canEditConfig && !userEdited && (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
          disabled={resetting}
          onClick={() => onReset(plan)}
          title={t("loongport.tier.resetConfig")}
        >
          {resetting ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Undo2 className="h-3.5 w-3.5" />
          )}
        </Button>
      )}
      {plan.canEditConfig && userEdited && (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 p-1 text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-400 dark:hover:text-amber-300"
          disabled={resetting}
          onClick={() => onReset(plan)}
          title={t("loongport.vendor.userEditedHint")}
        >
          {resetting ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Undo2 className="h-3.5 w-3.5" />
          )}
        </Button>
      )}
    </>
  );
}

/**
 * 行右侧的状态与动作。状态由后端 DTO 计算，前端只按状态展示对应动作：
 *
 * | 后端 `status` | 显示 | 为什么不能混 |
 * |---|---|---|
 * | `ready` | （单 plan：使用 / 在用）| 后端确认凭据和配置可用 |
 * | `sessionExpiredUsable` | （单 plan：使用 / 在用）+ 重登 | 登录态过期，但后端确认 SK 仍可用 |
 * | `sessionExpired` | 「登录已过期」+ 重新登录 | 预填已就绪，用户只需补验证 |
 * | `notLoggedIn` | 「还没登录」+ 登录 | 从没登录过 |
 * | `noKey` | 「获取密钥」 | 登录了但还没备好 SK |
 *
 * 多 plan 行 `plan = null`：使用/在用在档位子行里，行级只剩登录相关的动作。
 */
function VendorStatus({
  account,
  busy,
  plan,
  onLogin,
  onProvision,
  onUse,
}: {
  account: VendorAccountRow;
  busy: ReadonlySet<string>;
  plan: VendorPlanInfo | null;
  onLogin: () => void;
  onProvision: () => void | Promise<void>;
  onUse: () => void;
}) {
  const { t } = useTranslation();
  const loggingIn = busy.has(vendorBusyKey("login", account.id));
  const provisioning = busy.has(vendorBusyKey("provision", account.id));
  const switching = plan
    ? busy.has(vendorPlanBusyKey("switch", plan.providerId))
    : false;

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
          {plan?.isCurrent ? (
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
          ) : plan?.canSwitch ? (
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

/**
 * 一个 plan 档位子行（opencode Zen / Go）。结构与视觉抄 `RelayRow` 的 `TierItem`：
 * 名字 + 「已手动维护」标记常驻，使用/编辑/恢复在 hover 组里（手改过的恢复按钮
 * 常驻 —— 它是那个状态唯一的出口）。比 TierItem 少的东西（倍率、连通检测、模型
 * 验证）都是 relay 侧的事实，vendor 档位没有。
 */
function PlanItem({
  plan,
  busy,
  onUse,
  onEdit,
  onReset,
}: {
  plan: VendorPlanInfo;
  busy: ReadonlySet<string>;
  onUse: () => void;
  onEdit: () => void;
  onReset: () => void;
}) {
  const { t } = useTranslation();
  const switching = busy.has(vendorPlanBusyKey("switch", plan.providerId));
  const resetting = busy.has(vendorPlanBusyKey("resetVendor", plan.providerId));

  // ⚠️ **`=== true` 三态**，同 `TierItem`：`null`（判不了）时什么都不显示。
  const userEdited = plan.userEdited === true;

  return (
    <div
      className={cn(
        // `group/tier` 而不是裸 `group` —— 见 `PLAN_HOVER_ACTIONS` 的说明。
        "group/tier flex flex-wrap items-center gap-2 rounded-lg border border-border px-3 py-2 transition-all",
        // 三态优先级：当前在用 > 已手动维护 > 普通（同 `TierItem`）。
        plan.isCurrent
          ? "border-blue-500/60 shadow-sm shadow-blue-500/10"
          : userEdited
            ? "border-amber-500/50 bg-amber-500/5 hover:border-amber-500/70"
            : "hover:border-border-active",
      )}
    >
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-sm">{plan.planName}</span>
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
      </div>

      {/* 整组 hover / focus 才出现，`pointer-events-none` 不能省（透明的按钮
          仍然可点，鼠标扫过空白处会误触）—— 逐字抄 `TierItem` 那段。 */}
      <div
        className={cn(
          "flex flex-shrink-0 items-center gap-0.5",
          HOVER_ACTIONS_BASE,
          switching || resetting ? HOVER_ACTIONS_PINNED : PLAN_HOVER_ACTIONS,
        )}
      >
        {plan.isCurrent ? (
          <Button
            type="button"
            size="sm"
            className="h-7 shrink-0 cursor-not-allowed bg-gray-200 text-muted-foreground hover:bg-gray-200 hover:text-muted-foreground dark:bg-gray-700 dark:hover:bg-gray-700"
            disabled
          >
            <Check className="mr-1 h-3.5 w-3.5" />
            {t("provider.inUse")}
          </Button>
        ) : (
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
        )}

        {plan.canEditConfig && (
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

        {plan.canEditConfig && !userEdited && (
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
      </div>

      {/* 「恢复默认配置」的**常驻**版，只给已手动维护的档位（同 `TierItem`：
          它是那个状态唯一的出口，用户改坏了要退回默认，不该先猜按钮在哪）。 */}
      {plan.canEditConfig && userEdited && (
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
