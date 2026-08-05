import { useTranslation } from "react-i18next";
import {
  Check,
  GripVertical,
  Loader2,
  Pencil,
  PencilLine,
  Play,
  RefreshCw,
  Trash2,
  Undo2,
  Wallet,
} from "lucide-react";

import type { DraggableAttributes } from "@dnd-kit/core";
import type { SyntheticListenerMap } from "@dnd-kit/core/dist/hooks/utilities";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { VendorAccountRow } from "@/lib/api/vendor";

import { rowKey } from "./rowKey";

/**
 * 一行官网直连账号（DeepSeek 之类），与 `OperatorRow` 并列显示在同一个列表里。
 *
 * ## 与 `OperatorRow` 的关系：抄视觉，结构上少一层
 *
 * 差异只有三处（其余 className / 间距 / hover 全部逐字抄它，判据是
 * CLAUDE.md §一「和旧页面放一起看不出是两个人写的」）：
 *
 * 1. **不可展开** —— 一个官网账号就一个 endpoint，没有档位层可展开
 *    （所以没有 `Collapsible`、没有箭头、也不需要 `open` / `onOpenChange`）
 * 2. **无档位列表** —— 同上
 * 3. **余额直接渲染字符串** —— 后端给的已经是 `"¥547.08"`（币种符号在里面），
 *    前端**不做任何数值转换或格式化**。operator 那边是 `number` + `toFixed(2)`，
 *    两条契约有意不同（见 `vendorApi.balance`）
 *
 * ## 官网行的余额不可点
 *
 * operator 那边余额是个按钮（点开充值页），因为它有 `operator_purchase` 命令。
 * vendor 侧**没有对应命令**（官网充值要走厂商自己的收银台，我们没做也不该做），
 * 所以这里余额就是一段文字 —— 做成看起来能点的样子是骗人。
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
  /**
   * 这一行的余额，**已格式化**（`"¥547.08"`）。`null` = 还没拉到 / 拉失败
   * （登录态过期时必然拉不到）—— 那时整块不渲染，不摆「--」占位符。
   */
  balance: string | null;
  /** 这一行的配置是不是当前 tab 正在用的那个。 */
  isCurrent: boolean;
  onLogin: () => void;
  /** 备好密钥（也是「刷新」的实现 —— 本地已有明文时零请求）。 */
  onProvision: () => void;
  /** 切到这个账号的配置。 */
  onUse: () => void;
  onDelete: () => void;
  /**
   * 编辑**当前 tab 那个平台**的配置（跳 cc-switch 的编辑页）。
   *
   * 一行背后六条 provider 记录，编辑的是当前页那一条 —— 用户要改 Claude 的模型
   * 映射时本来就在 Claude 页。与 operator 档位同一条路（`useTierEditGuard`）。
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

/** 逐字抄 `OperatorRow` 的那两条（hover 才显形的动作组）。 */
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
  balance,
  isCurrent,
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

  // ⚠️ **`=== true` 而不是 `??` 或直接判真值** —— 三态，照 `OperatorRow:636`：
  // `true`（改过）/ `false`（没改）/ `null`（**判不了**：还没 provision、
  // 这个平台不适用、或读不出密钥）。`null` 时什么都不显示 —— 显示「未手动维护」
  // 是在断言「刷新不会覆盖你的改动」，而事实是不知道，让用户误信比不说更糟。
  const userEdited = account.userEdited === true;

  // 编辑与恢复都要读那条 provider 记录 —— 还没 provision 过就没有它。
  // 那种行显示的是「获取密钥」引导态（下面 `keyReady` 那几个分支），
  // 摆一个点了必然报错的编辑按钮是骗人。
  const canEditConfig = account.keyReady && account.providerId !== "";

  return (
    <div
      className={cn(
        // `group/row` 承接下面的 `group-hover/row:` —— 与 `OperatorRow` 同名，
        // 两类行不会嵌套（都是列表的直接子项），所以不必再取第二个名字。
        "group/row rounded-xl border bg-card p-4 text-card-foreground transition-all duration-300",
        dragHandleProps?.isDragging
          ? "cursor-grabbing border-primary shadow-lg"
          : isCurrent
            ? // 当前在用的行用蓝框，与 `TierItem` 的当前态同一个 token
              // （`ProviderCard.tsx:306`）—— 用户扫一眼列表首先要找到在用的那个。
              "border-blue-500/60 shadow-sm shadow-blue-500/10"
            : userEdited
              ? // 已手动维护：amber，与 `OperatorRow` 的档位行同一套语义
                // （蓝 = 在用、绿 = 代理接管都已有主，amber = 需要留意）。
                // **优先级低于「当前在用」**，同那边的三态排序。
                "border-amber-500/50 bg-amber-500/5 hover:border-amber-500/70"
              : "border-border hover:border-border-active",
      )}
    >
      <div className="flex items-center gap-2">
        {/* 拖动手柄。位置与视觉抄 `OperatorRow` —— 用户在同一个列表里
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
            两行式的布局与 `OperatorRow` 一致（站名 / 账号名）。 */}
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <span className="truncate text-sm font-medium">
              {account.vendorName}
            </span>
            {/* 「已手动维护」标记。视觉逐字抄 `OperatorRow:673`。
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

        {/* 余额。**直接渲染后端给的字符串**，不 `toFixed`、不拼币种符号 ——
            那些都在 Rust 侧做完了（见 `vendorApi.balance`）。
            `null` 时整块不渲染（与 operator 侧同一条判据）。 */}
        {balance !== null && (
          <span className="flex shrink-0 items-center gap-1 px-1.5 py-1 text-xs text-muted-foreground">
            <Wallet className="h-3.5 w-3.5" />
            {balance}
          </span>
        )}

        {/* 删除。**hover 才出**，与 `OperatorRow` 同一条规矩（破坏性动作值得藏）。

            与 operator 行的区别：这里**前端不预先判「在用所以不能删」**，由后端
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
              `useTierEditGuard`）。视觉抄 `OperatorRow:790` 那个。 */}
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
              不会同时出现两个恢复按钮。同 `OperatorRow:797` 的分法。 */}
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
        </div>

        {/* 「恢复默认配置」的**常驻**版，只给已手动维护的那些。
            理由同 `OperatorRow:825` 那段：对普通行它是兜底（没改过，没什么可恢复），
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

        <VendorStatus
          account={account}
          busy={busy}
          isCurrent={isCurrent}
          onLogin={onLogin}
          onProvision={onProvision}
          onUse={onUse}
        />
      </div>
    </div>
  );
}

/**
 * 行右侧的状态与动作。**四种状态，判定顺序不能变**（后端有意把三个布尔分开，
 * 合并任意两个都会给用户错的处境）：
 *
 * | 条件 | 显示 | 为什么这个顺序 |
 * |---|---|---|
 * | `keyReady` | 使用 / 在用 + 刷新 | ⚠️ **优先于 `sessionExpired`** —— 见下 |
 * | `sessionExpired` | 「登录已过期」+ 重新登录 | 预填已就绪，用户只需补验证 |
 * | `!loggedIn` | 「还没登录」+ 登录 | 从没登录过 |
 * | `loggedIn` | 「获取密钥」 | 登录了但还没备好 sk |
 *
 * ## ⚠️ `keyReady` 必须排在 `sessionExpired` 前面
 *
 * 这两个布尔**独立**：sk 是厂商侧的独立凭据，网页登录态过期**不影响它**
 * （`creds::clear_token` 特意不清 `api_key`，那边有测试钉着）。所以
 * `keyReady && sessionExpired` 的行**照样能用** —— 反过来判会把一个完全可用的
 * 账号显示成「请重新登录」，而用户重登一次什么也没变（sk 本来就没失效）。
 *
 * 那种行唯一真实的损失是**拉不到余额**（余额要网页登录态）。所以「重新登录」
 * 入口不消失、只降级成 hover 才出的小按钮，title 说清是为了什么 ——
 * 让想看余额的人有路可走，而不催所有人。
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
  onProvision: () => void;
  onUse: () => void;
}) {
  const { t } = useTranslation();
  const loggingIn = busy.has(vendorBusyKey("login", account.id));
  const provisioning = busy.has(vendorBusyKey("provision", account.id));
  const switching = busy.has(vendorBusyKey("switch", account.id));

  if (account.keyReady) {
    return (
      <div className="flex shrink-0 items-center gap-2">
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

        {/* 次要动作组，**hover 才出**（与 `OperatorRow` 的「重新拉分组」同一条规矩：
            动作藏进 hover，信息常驻）。进行中钉住可见。 */}
        <div
          className={cn(
            "flex shrink-0 items-center gap-0.5",
            HOVER_ACTIONS_BASE,
            provisioning || loggingIn
              ? HOVER_ACTIONS_PINNED
              : ROW_HOVER_ACTIONS,
          )}
        >
          {/* 登录态过期但 sk 还在用的行：给一条重登的路，但**不催**。
              唯一的实际损失是余额拉不到，title 就说这件事。 */}
          {account.sessionExpired && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 gap-1 px-2 text-muted-foreground"
              disabled={loggingIn}
              onClick={onLogin}
              title={t("loongport.vendor.sessionExpiredUsable")}
            >
              {loggingIn ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                t("loongport.row.reLogin")
              )}
            </Button>
          )}
          {/* 「重新备一次密钥」= 把本地这把 sk 重新写进六个平台的配置。

              ⚠️ **它不会去官网换一把新 key**：`vendor_provision` 只在本地
              `api_key` 为空时才联网建（见它第 2 步「本地已有明文 ⇒ 零请求」）。
              所以用户在官网手动删/撤销了 key 之后，点这个按钮是**无效**的 ——
              它会把同一把已失效的 sk 再写一遍。那种情况目前只能删掉这一行再重新添加
              （没有「换一把 key」的入口，已记在 `TODO.md`）。

              这个按钮真正有用的场景：配置被改坏、或某个平台的记录写失败了，
              重新展开一次把六个平台补齐。 */}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 gap-1 px-2"
            disabled={provisioning}
            onClick={onProvision}
            title={t("loongport.vendor.refreshKey")}
          >
            {provisioning ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RefreshCw className="h-3.5 w-3.5" />
            )}
          </Button>
        </div>
      </div>
    );
  }

  // 判定顺序：**先 `sessionExpired` 后 `!loggedIn`** —— 过期时 `loggedIn` 也是
  // false，反了就永远走不到过期分支，用户会被当成从没登录过（与 `RowStatus` 同）。
  if (account.sessionExpired) {
    return (
      <StatusAction
        hint={t("loongport.row.sessionExpired")}
        label={t("loongport.row.reLogin")}
        loading={loggingIn}
        onClick={onLogin}
      />
    );
  }

  if (!account.loggedIn) {
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

/** 与 `OperatorRow` 里那个同形（hint 文字 + outline 按钮）。 */
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
