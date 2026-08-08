import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertCircle,
  Check,
  ChevronDown,
  ChevronRight,
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
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import type { RelayRow as RelayRowData, TierInfo } from "@/lib/api/relay";
import { isLowBalance, LOW_BALANCE_THRESHOLD_USD } from "./lowBalance";

/**
 * 一行中转站 + 可折叠的档位列表。
 *
 * ## 与 `ProviderCard` 的关系：抄视觉不抄结构
 *
 * `ProviderCard` 626 行，服务的是**用户手工配置的 provider**（可编辑、可删除、
 * 可拖拽排序）。托管档位没有这些操作 —— 硬塞进去会让两种形态互相污染
 * （CLAUDE.md §一「什么时候可以不复用」正是这一条）。
 *
 * 但**视觉 token 全部抄它**，判据是「和旧页面放一起看不出是两个人写的」：
 * `rounded-xl border p-4`、选中态 `border-blue-500/60 shadow-sm shadow-blue-500/10`
 * （`ProviderCard.tsx:299-306`）、当前项用裸 `Check`（不是 `CheckCircle2`）。
 *
 * ## 折叠没有动画，这是事实不是遗漏
 *
 * `ui/collapsible.tsx` 只是 Radix 三个 primitive 的裸 re-export（9 行，无 className
 * 无动画）。`tailwind.config.cjs` 里只定义了 `accordion-down/up`，而且键在
 * `--radix-accordion-content-height` —— Collapsible 用的是
 * `--radix-collapsible-content-height`，**对不上**。所以是直接开合，
 * 与仓库现有消费者（`HermesFormFields` 等）一致。别去找那个不存在的动画。
 */
/**
 * 「hover / focus 才显形的动作组」那套 class。**逐字抄 `ProviderCard.tsx:559`**。
 *
 * ## `pointer-events-none` 不能省
 *
 * 只改 `opacity` 的话透明按钮**仍然可点** —— 鼠标扫过看似空白的地方会误触删除。
 *
 * ## 为什么是两份字面量，而不是拼一个 `group-hover/${name}:` 出来
 *
 * ⚠️ **Tailwind 的 JIT 靠扫源码里的字面 class 生成样式**，模板拼出来的名字它看不见
 * ⇒ 那条样式压根不生成 ⇒ 按钮**永远不显形**，而且 tsc / build 都不报错。
 * 这正是 CLAUDE.md §三点六 说的那类「编译器管不到、不一致时不崩只是功能悄悄没了」。
 *
 * ## 为什么两个 group 都取了名字
 *
 * 档位行**嵌在**中转站行里面。裸 `group-hover:` 编译成 `.group:hover &`，会匹配
 * **任意**带 `group` 的祖先 —— 两层都用裸的话，鼠标停在中转站行上就会把里面所有
 * 档位行的按钮一起点亮。取名后各自只认自己那层（`group/row` 与 `group` 是不同的
 * class token，不会互相匹配）。仓里 `OmoFormFields.tsx:913` 的 `group/tip` 是同一个用法。
 */
const HOVER_ACTIONS_BASE = "transition-opacity duration-200";
/** 操作进行中钉住可见：否则鼠标一移开就看不到自己点的东西还在跑。 */
const HOVER_ACTIONS_PINNED = "pointer-events-auto opacity-100";
const ROW_HOVER_ACTIONS =
  "pointer-events-none opacity-0 group-hover/row:pointer-events-auto group-hover/row:opacity-100 group-focus-within/row:pointer-events-auto group-focus-within/row:opacity-100";
const TIER_HOVER_ACTIONS =
  "pointer-events-none opacity-0 group-hover/tier:pointer-events-auto group-hover/tier:opacity-100 group-focus-within/tier:pointer-events-auto group-focus-within/tier:opacity-100";

export interface RelayRowProps {
  relay: RelayRowData;
  /** 展开态由父组件持有 —— 它要按中转站 id 存 localStorage（见 `RelayTierList`）。 */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /**
   * 正在进行的操作集合（如 `switch:<providerId>`），用于按钮转圈与禁用。
   *
   * **是集合不是单个字符串**：只禁自己那一行正在跑的那个动作，
   * 别的中转站、乃至本行的其它按钮都不受影响 —— 见 `useRowBusy`。
   */
  busy: ReadonlySet<string>;
  onLogin: () => void;
  onProvision: () => void;
  onSwitchTier: (tier: TierInfo) => void;
  /**
   * 这一行的余额。`null` = 还没拉到 / 拉失败（中转站可能关了用户面板）。
   *
   * **拉失败不是异常状况**，所以是 `null` 而不是抛错：余额是附加信息，
   * 为它打断用户的主流程是错的。UI 在 `null` 时什么都不显示。
   */
  balance: number | null;
  /** 点余额 → 带登录态开这一行的充值页。 */
  onPurchase: () => void;
  /**
   * 检测某个档位的连通性。**复用上游的 `useStreamCheck`** —— 托管档位就是
   * 正常的 provider 记录（`category = "aggregator"`，在同一张表里），
   * 所以那条命令直接就能用。
   */
  onCheckTier: (tier: TierInfo) => void;
  /** 某个档位是不是正在检测中（来自 `useStreamCheck` 的 `isChecking`）。 */
  isCheckingTier: (providerId: string) => boolean;
  /**
   * 把某个档位的配置恢复成默认值（用户在编辑页改坏之后的回头路）。
   *
   * 入口是档位行 hover 才出现的小按钮；**已手动维护的档位上它常驻**
   * （见 `TierItem` 里的说明）—— 那种档位随时可能要退回默认值。
   */
  onResetTier: (tier: TierInfo) => void;
  /**
   * 编辑某个档位的配置（跳 cc-switch 现成的编辑页）。
   *
   * 调用方负责先弹那道「保存后归你自己维护」的警告 —— 判据不在这里，
   * 因为这个组件不知道用户是不是已经勾过「不再提示」。
   */
  onEditTier: (tier: TierInfo) => void;
  /**
   * 删掉这一行（这个「站点 × 账号」）连带它名下的托管档位。
   *
   * **名下有档位正在使用时不会被调用** —— 那种情况按钮渲染成不可点（见 `RowDelete`）。
   */
  onDelete: () => void;
  /** dnd-kit 的拖动手柄 props（由 `SortableRelayRow` 注入）。 */
  dragHandleProps?: {
    attributes?: DraggableAttributes;
    listeners?: SyntheticListenerMap;
    isDragging?: boolean;
  };
}

export function RelayRow({
  relay,
  open,
  onOpenChange,
  busy,
  onLogin,
  onProvision,
  onSwitchTier,
  balance,
  onPurchase,
  onCheckTier,
  isCheckingTier,
  onResetTier,
  onEditTier,
  onDelete,
  dragHandleProps,
}: RelayRowProps) {
  const { t } = useTranslation();
  // 名下有档位正在使用 ⇒ 这一行不许删。
  //
  // 判据延伸自上游 `ProviderActions.tsx:224` 的 `canDelete = ... && !isCurrent`
  // ——「正在用的不许删」。区别是那边一行就是一个 provider，而这里一行有多个档位，
  // 所以判的是「**任一**档位在用」。
  //
  // ⚠️ **这只是提示，不是闸** —— `relay.tiers` 只含**当前 tab 那个 app** 的档位
  // （`relay_list_relays` 吃 `app` 参数），所以这个判据看不见「这个账号在别的
  // 平台是当前项」。真正的闸在后端 `remove_site_impl`（它扫全部 app，撞上就报错并
  // 点名哪个平台、哪个档位）。
  //
  // 两处都留是有意的，不是冗余：按钮先变灰是**即时反馈**（同 tab 内用户不必点下去
  // 才知道），而跨 tab 那一类只能由后端拦 —— 前端要判它得为此新增一条「查全部 app
  // 的当前项」的 IPC，而后端的错误文案已经把用户该做的处置说清了。
  const hasCurrentTier = relay.tiers.some((tier) => tier.isCurrent);

  return (
    <Collapsible open={open} onOpenChange={onOpenChange}>
      <div
        className={cn(
          // `group/row` 承接下面那些 `group-hover/row:` —— 见 `ROW_HOVER_ACTIONS`。
          // 取名而非裸 `group`：档位行嵌在这里面，裸的两层会互相点亮。
          "group/row rounded-xl border border-border bg-card p-4 text-card-foreground transition-all duration-300",
          dragHandleProps?.isDragging
            ? "cursor-grabbing border-primary shadow-lg"
            : "hover:border-border-active",
        )}
      >
        <div className="flex items-center gap-2">
          {/* 拖动手柄。视觉与 `ProviderCard` 那个一致（GripVertical + cursor-grab），
              放最左侧 —— 用户在 provider 页已经习惯了那个位置。 */}
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

          {/* **箭头 + 站名整片都是折叠触发区**（用户要求：不用瞄准那个小箭头）。

              触发区**有意不含**两样东西：
              - 左侧拖动手柄 —— 拖动时的 pointer 事件会被 dnd-kit 吃掉，套进
                trigger 里会让「拖一下」也切换折叠
              - 右侧的登录/获取密钥按钮 —— 点它们是另一个意图，顺带折叠是干扰

              用 `asChild` 把 trigger 塌进这个 div，不额外套一层
              （Radix 的 trigger 默认渲染 button，button 里再放 div 是非法嵌套）。 */}
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

              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">
                  {relay.siteName || relay.siteOrigin}
                </span>
                {/* 账号名单独一行：同一个站可以挂多个账号，光有站名分辨不出是哪个。 */}
                {relay.accountLabel && (
                  <span className="block truncate text-xs text-muted-foreground">
                    {relay.accountLabel}
                  </span>
                )}
              </span>
            </div>
          </CollapsibleTrigger>

          {/* 余额 + 充值入口。放在状态按钮**左边**：它是这一行的属性（多少钱），
              而右边那组是动作（登录 / 获取密钥）—— 混在一起会让用户以为余额也是个按钮组的一部分。
              未登录的行没有余额可言，`balance` 恒为 null，这里自然什么都不渲染。 */}
          <RowBalance
            balance={balance}
            // ⚠️ **必须带 loggedIn 守卫**（review 抓出）：`balances` 是按 id 累积的 map，
            // 它不会因为某一行登出就自动清掉那一项。只判 `balance !== null` 的话，
            // 用户登出后那一行仍显示**登出前的旧金额**和一个可点的充值按钮 ——
            // 点下去才报「请先登录」。余额是账号态的一部分，没登录就不该有余额。
            loggedIn={relay.loggedIn}
            busy={busy.has(`purchase:${relay.id}`)}
            onPurchase={onPurchase}
          />

          {/* 删除这一行（这个「站点 × 账号」）。
              放余额旁边、状态按钮之前 —— 与上游把删除放在动作组末尾同理，
              它是这一行的破坏性操作，不该跟「登录 / 获取密钥」抢位置。

              **hover 才出**（与档位行同一条规矩）。删除是破坏性动作，常驻一个红色垃圾桶
              在每一行上，比档位行那些次要按钮更值得藏 —— 上游对删除正是这么处理的。 */}
          <div
            className={cn(
              "flex shrink-0 items-center",
              HOVER_ACTIONS_BASE,
              busy.has(`removeRelay:${relay.id}`)
                ? HOVER_ACTIONS_PINNED
                : ROW_HOVER_ACTIONS,
            )}
          >
            <RowDelete
              canDelete={!hasCurrentTier}
              busy={busy.has(`removeRelay:${relay.id}`)}
              onDelete={onDelete}
            />
          </div>

          <RowStatus
            relay={relay}
            busy={busy}
            onLogin={onLogin}
            onProvision={onProvision}
          />
        </div>

        <CollapsibleContent className="space-y-2 pt-3">
          {relay.tiers.map((tier) => (
            <TierItem
              key={tier.providerId}
              tier={tier}
              busy={busy}
              onSwitch={() => onSwitchTier(tier)}
              onCheck={() => onCheckTier(tier)}
              checking={isCheckingTier(tier.providerId)}
              onReset={() => onResetTier(tier)}
              onEdit={() => onEditTier(tier)}
            />
          ))}
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}

/**
 * 删掉这一行（这个「站点 × 账号」）。
 *
 * ## 不可删时**仍然渲染**，只是点不动 —— 这是抄上游的取舍
 *
 * `ProviderActions.tsx:358-364` 就是这么做的：`canDelete` 为 false 时按钮照样在，
 * 只是 `opacity-40 cursor-not-allowed` 且不挂 `onClick`。
 *
 * 为什么不直接隐藏：用户得知道「这里有删除这个操作，只是现在不能用」。
 * 藏起来的话，正在用某个档位的用户会以为这一行根本删不了，转而去别处找入口。
 * `title` 说清原因（要先切走），他就知道下一步做什么。
 *
 * ⚠️ **`disabled` 属性不能用**：`Button` 基类带 `disabled:pointer-events-none`，
 * 那会让 `title` 在 hover 时不显示 —— 而这里的 `title` 正是「为什么点不了」的唯一解释。
 * 所以走「不挂 onClick + 视觉变灰」，与上游同一个写法。
 */
function RowDelete({
  canDelete,
  busy,
  onDelete,
}: {
  canDelete: boolean;
  busy: boolean;
  onDelete: () => void;
}) {
  const { t } = useTranslation();

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      className={cn(
        "h-7 w-7 shrink-0 p-1 text-muted-foreground",
        canDelete && !busy
          ? "hover:text-red-500 dark:hover:text-red-400"
          : "cursor-not-allowed opacity-40",
      )}
      onClick={canDelete && !busy ? onDelete : undefined}
      title={
        canDelete
          ? t("loongport.row.remove")
          : t("loongport.row.removeBlockedByCurrent")
      }
    >
      {busy ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Trash2 className="h-3.5 w-3.5" />
      )}
    </Button>
  );
}

/**
 * 这一行的余额，点一下带登录态开充值页。余额偏低时变成一个警示。
 *
 * ## 为什么是 button 而不是「一段文字 + 旁边一个按钮」
 *
 * 维护者要的是「鼠标放上面指示可以点击」—— 那就是说这段数字**本身**是控件。
 * 抄仓里已有的同形范例：`ProviderCard` 的 URL 那一处也是「看起来像文字、
 * 实际是 button，hover 出下划线 + 变蓝」（`ProviderCard.tsx` 的 `isClickableUrl`）。
 * 判据仍是「和旧页面放一起看不出是两个人写的」。
 *
 * `balance === null` 时**整块不渲染**：可能是还没拉到、也可能是中转站关了用户面板。
 * 摆一个「--」或「加载中」只会让用户盯着一个永远不变的占位符 —— 那比没有更糟。
 *
 * ## ⚠️ 低余额提醒为什么**不新增一个叹号按钮**（2026-08-04）
 *
 * 需求原文是「在账户余额旁边弄个小叹号，点击跳转对应账号的充值页面」。
 * 而这段余额**本来就是**「点一下跳充值页」的按钮 ⇒ 再加一个按钮的话，
 * 两个紧邻的控件点下去做同一件事，还得各自处理冒泡、busy、disabled。
 *
 * 所以做法是**把这一个控件切到警示态**：钱包图标换成叹号、颜色转琥珀、
 * title 换成催充值的那句。用户看到的仍是「余额旁边有个叹号」，
 * 点它跳充值页 —— 需求原文的行为一字不差，但少一个控件。
 *
 * 这也顺带避免了一个真实的坑：两个相邻按钮里只有一个 `stopPropagation`
 * 的话，点错那个会顺带折叠整行。
 */
function RowBalance({
  balance,
  loggedIn,
  busy,
  onPurchase,
}: {
  balance: number | null;
  loggedIn: boolean;
  busy: boolean;
  onPurchase: () => void;
}) {
  const { t } = useTranslation();

  // 两个条件都要：没登录不该有余额（`balances` 是累积的 map，登出不会自动清掉那一项，
  // 只判 balance 会让登出后的行继续显示旧金额）；`null` 是还没拉到或拉失败。
  if (!loggedIn || balance === null) return null;

  // 判据提在 `lowBalance.ts` 里 —— 它有两个消费者（图标与配色），
  // 且是这个面板的领域规则，不该与「怎么画」缠在一起。
  const low = isLowBalance(balance);

  return (
    <button
      type="button"
      onClick={(e) => {
        // 这个按钮在折叠触发区**外面**，但它与整行的点击区相邻 ——
        // 不阻止冒泡的话点余额会顺带折叠这一行。
        e.stopPropagation();
        onPurchase();
      }}
      disabled={busy}
      // title 挂在按钮自身：它 disabled 的时间极短（就是开窗那一下），
      // 不像上游那几个常驻 disabled 的按钮需要 wrapper 承接 hover。
      title={
        low
          ? t("loongport.row.lowBalanceHint", {
              threshold: LOW_BALANCE_THRESHOLD_USD,
            })
          : t("loongport.row.purchaseHint")
      }
      className={cn(
        "flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-xs transition-colors",
        // 低余额：琥珀色常驻（不是 hover 才出）—— 它是个提醒，藏起来就没用了。
        // 用琥珀而不是红：钱不够是「该处理一下」，不是「出错了」。
        // 这两个色阶抄的是仓里已有的警示用法（`AddSiteDialog` 的提示条同一组）。
        low
          ? "text-amber-600 hover:bg-amber-50 hover:underline dark:text-amber-500 dark:hover:bg-amber-950/40"
          : "text-muted-foreground hover:bg-muted/60 hover:text-blue-500 hover:underline dark:hover:text-blue-400",
        busy && "cursor-not-allowed opacity-60 hover:no-underline",
      )}
    >
      {busy ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : low ? (
        // 叹号替掉钱包 —— 需求要的那个「小叹号」就是它。
        <AlertCircle className="h-3.5 w-3.5" />
      ) : (
        <Wallet className="h-3.5 w-3.5" />
      )}
      {/* `$` + 两位小数，与顶部那处显示**一致**（同一个数字在两处必须同一个样子）。
          初稿这里有意省掉 `$`，理由写的是「没拿到站点用什么货币的字段」——
          **那个理由是错的**：sub2api 的 `balance` 就是美元计价（后台文案写的是
          「每付 1 CNY 得到多少 USD 余额」，支付货币默认 CNY 再按汇率换成 USD 余额），
          且 `/settings/public` 里压根没有货币字段可查 ⇒ 写死 `$` 才是对的。 */}
      ${balance.toFixed(2)}
    </button>
  );
}

/**
 * 行右侧的状态与动作。**三种状态必须分清**（spec §四，初版把它们混了）：
 *
 * | 条件 | 显示 | 为什么不能混 |
 * |---|---|---|
 * | `!loggedIn && !sessionExpired` | 「还没登录」+ 登录 | 从没登录过，零分组是必然的；说「没有可用分组」是误导 |
 * | `sessionExpired` | 「登录已过期」+ 重新登录 | 预填已就绪，用户只需补密码 + 人机验证 |
 * | `loggedIn && tiers 为空` | 「没有可用分组」+ 获取密钥 | 真登录了但这个平台没东西 |
 * | `loggedIn && 有档位` | 档位数 + **重新拉分组** | 见下 |
 *
 * 判定顺序是**先 `sessionExpired` 后 `!loggedIn`** —— 过期时 `loggedIn` 也是 false，
 * 反了就永远走不到过期分支，用户会被当成从没登录过。
 *
 * 都**不做整页拦截**：其它中转站还能用。
 *
 * ## 最后一档为什么要给按钮（2026-08-03 加，用户实测发现）
 *
 * 原来「获取密钥」只在 `tiers.length === 0` 时出现 ⇒ **已经有档位的行没有任何
 * 重拉入口**。中转站在网页端新增一个分组后，用户在这一页无论点什么都看不到它
 * （顶部「刷新」当时也只读本地 + 查倍率）。
 *
 * 禁用只看**自己这一个 key**，不看别人 —— 中转站之间无依赖。
 */
function RowStatus({
  relay,
  busy,
  onLogin,
  onProvision,
}: {
  relay: RelayRowData;
  busy: ReadonlySet<string>;
  onLogin: () => void;
  onProvision: () => void;
}) {
  const { t } = useTranslation();
  const loggingIn = busy.has(`login:${relay.id}`);
  const provisioning = busy.has(`provision:${relay.id}`);

  if (relay.sessionExpired) {
    return (
      <StatusAction
        hint={t("loongport.row.sessionExpired")}
        label={t("loongport.row.reLogin")}
        loading={loggingIn}
        disabled={loggingIn}
        onClick={onLogin}
      />
    );
  }

  if (!relay.loggedIn) {
    return (
      <StatusAction
        hint={t("loongport.row.notLoggedIn")}
        label={t("loongport.row.login")}
        loading={loggingIn}
        disabled={loggingIn}
        onClick={onLogin}
      />
    );
  }

  if (relay.tiers.length === 0) {
    return (
      <StatusAction
        hint={t("loongport.row.noTiers")}
        label={t("loongport.row.getKeys")}
        loading={provisioning}
        disabled={provisioning}
        onClick={onProvision}
      />
    );
  }

  return (
    <div className="flex shrink-0 items-center gap-2">
      <span className="text-xs text-muted-foreground">
        {t("loongport.row.tierCount", { count: relay.tiers.length })}
      </span>
      {/* 「重新拉分组」。用 ghost + 图标（不是 outline + 文字）——
          这一行已经有档位了，它是个补充动作，不该抢「用哪个档位」的注意力。

          **hover 才出**：同上一条规矩，它是动作而非信息。
          左边那个「N 个档位」是信息，所以留在组外常驻。 */}
      <div
        className={cn(
          "flex shrink-0 items-center",
          HOVER_ACTIONS_BASE,
          provisioning ? HOVER_ACTIONS_PINNED : ROW_HOVER_ACTIONS,
        )}
      >
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 gap-1 px-2"
          disabled={provisioning}
          onClick={onProvision}
          title={t("loongport.row.refetchGroups")}
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

function StatusAction({
  hint,
  label,
  loading,
  disabled,
  onClick,
}: {
  hint: string;
  label: string;
  loading: boolean;
  disabled: boolean;
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
        disabled={disabled}
        onClick={onClick}
      >
        {loading && <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />}
        {label}
      </Button>
    </div>
  );
}

/**
 * 一个档位。
 *
 * ⚠️ **`rateMultiplier` 为 null 时显示「倍率未知」，绝不能显示成 0 或「免费」** ——
 * 列表命令只读本地，倍率是服务端定价，要 provision 才有值
 * （`relay.rs` 的 `TierInfo` 注释钉过这条）。显示成 0 会让用户以为这是最便宜的一档。
 */
function TierItem({
  tier,
  busy,
  onSwitch,
  onCheck,
  checking,
  onReset,
  onEdit,
}: {
  tier: TierInfo;
  busy: ReadonlySet<string>;
  onSwitch: () => void;
  onCheck: () => void;
  checking: boolean;
  onReset: () => void;
  onEdit: () => void;
}) {
  const { t } = useTranslation();
  // 只禁**这一个档位**正在切换的那个按钮。原来是 `disabled={anyBusy}`，
  // 于是别的中转站在获取密钥时，这里所有「使用」按钮都灰掉了。
  const switching = busy.has(`switch:${tier.providerId}`);
  const resetting = busy.has(`reset:${tier.providerId}`);

  // ⚠️ **`=== true` 而不是 `??` 或直接判真值** —— `userEdited` 是三态：
  // `true`（改过）/ `false`（没改）/ `null`（**判不了**，读不出密钥或这个 CLI
  // 没有默认形状）。`null` 时什么都不显示：显示「未手动维护」是在断言
  // 「刷新不会覆盖你的改动」，而事实是不知道 —— 让用户误信比不说更糟。
  const userEdited = tier.userEdited === true;

  return (
    <div
      className={cn(
        // `group/tier` 而不是裸 `group` —— 见 `TIER_HOVER_ACTIONS` 的说明：
        // 这一行嵌在中转站行里面，裸的会被外层 hover 一起点亮。
        "group/tier flex items-center gap-2 rounded-lg border border-border px-3 py-2 transition-all",
        // 三种态的优先级：**当前在用 > 已手动维护 > 普通**。
        //
        // 当前在用压过手动维护，是因为「现在生效的是哪一档」比「这一档谁维护」
        // 更要紧 —— 用户扫一眼列表首先要找到在用的那个。手动维护态在那一行
        // 靠标记与常驻的恢复按钮表达，不靠边框。
        //
        // ## 颜色为什么是 amber
        //
        // 蓝色与绿色在这个仓里**已经有主**：蓝 = 当前在用（本行上一个分支、
        // `ProviderCard.tsx:306`），绿 = 代理接管（`emerald`，同文件 `:304`）。
        // 用它们表示「手动维护」会与那两个语义撞车。
        //
        // amber 的既有语义正是「需要留意」（`ProviderList.tsx:475` 的警示条、
        // `SubscriptionQuotaFooter` 的配额告警）—— 而这一行要说的就是
        // 「它脱离自动维护了，出问题你得自己回退」。判据是尺子1：用仓里已有的
        // 颜色语义，别新造一套。
        tier.isCurrent
          ? "border-blue-500/60 shadow-sm shadow-blue-500/10"
          : userEdited
            ? "border-amber-500/50 bg-amber-500/5 hover:border-amber-500/70"
            : "hover:border-border-active",
      )}
    >
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-sm">{tier.groupName}</span>
          {/* 「已手动维护」标记。**常驻不藏进 hover** —— 它是状态不是动作，
              藏起来用户就得逐行 hover 才知道哪些档位脱离了自动维护。
              与「N 个档位」「余额」同一条判据（信息常驻、动作 hover）。 */}
          {userEdited && (
            <span
              className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-amber-600 ring-1 ring-inset ring-amber-500/30 dark:text-amber-400"
              title={t("loongport.tier.userEditedHint")}
            >
              <PencilLine className="h-2.5 w-2.5" />
              {t("loongport.tier.userEdited")}
            </span>
          )}
        </div>
        <div className="text-xs text-muted-foreground">
          {tier.rateMultiplier === null
            ? t("loongport.tier.rateUnknown")
            : t("loongport.tier.rate", { value: tier.rateMultiplier })}
        </div>
      </div>

      {/* ⚠️ **整组（含主按钮「启用 / 使用中」）都是 hover / focus 才出现** ——
          这一点上一轮弄错了：只把两个图标放进了 hover 组，`启用` 留在容器外面常驻。

          上游的真相在 `ProviderCard.tsx:559`：那个 `opacity-0 pointer-events-none
          group-hover:opacity-100 …` 的容器**包住整个 `ProviderActions`**，而
          `ProviderActions` 的第一个孩子就是主按钮（`ProviderActions.tsx:263-281`）。
          所以上游卡片没 hover 时右侧是**彻底空的**，连「使用中」都不显示 ——
          维护者的截图正是这样：蓝色高亮的 OpenAI Official 右边一片空白，
          只有鼠标底下的 default 那张才浮出「启用 + 5 个图标」。

          那串 class 逐字抄它，`pointer-events-none` 不能省 —— 只改 opacity 的话
          透明按钮仍然可点，鼠标扫过空白处会误触。

          **顺序也照它**：主按钮在左、图标组在右（`ProviderActions` 内部就是这个次序），
          原来我把图标放在了主按钮左边，跟 provider 页反着。

          尺度**有意用 h-7 而不是上游的 h-8**：档位行是中转站卡片内的嵌套行，
          外层那些控件都是 h-7，跟着上游升到 h-8 会让内外不一致。

          进行中的操作用 `opacity-100 pointer-events-auto` 钉住可见：
          否则鼠标一移开就看不到自己点的东西还在跑。**`switching` 也要算进去**
          （原来只算了 checking / resetting，而主按钮以前在组外所以没暴露这个问题）。 */}
      <div
        className={cn(
          "flex flex-shrink-0 items-center gap-0.5",
          HOVER_ACTIONS_BASE,
          checking || resetting || switching
            ? HOVER_ACTIONS_PINNED
            : TIER_HOVER_ACTIONS,
        )}
      >
        {/* 主按钮。**文案与图标复用上游的 `provider.enable` / `provider.inUse`** ——
            那两个 key 四个 locale 早就齐了，另建「使用」是重复发明（而且与上游同一屏
            出现两种叫法，用户会以为是两种不同操作）。

            当前档位那一支**照上游做成禁用的灰色按钮**（`ProviderActions.tsx:188-197`
            的 `isCurrent` 分支：`variant="secondary"` + `Check` + `bg-gray-200`），
            不再是一段裸文字 —— 两者在同一屏并排时，文字与按钮混排看着像没对齐。
            当前态本身由行的蓝色边框表达（与上游卡片同一个做法）。 */}
        {tier.isCurrent ? (
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
            onClick={onSwitch}
          >
            {switching ? (
              <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
            ) : (
              <Play className="mr-1 h-3.5 w-3.5" />
            )}
            {switching ? t("loongport.tier.switching") : t("provider.enable")}
          </Button>
        )}

        {/* 连通检测。**这个按钮是白捡的** —— 托管档位就是正常的 provider 记录
            （`category = "aggregator"`、写在同一张 provider 表里），所以上游那条
            `stream_check_provider` 命令直接就能用，前端也直接复用 `useStreamCheck`
            （它自带 toast、i18n 与 per-id 的 checking 状态）。

            结果只走 toast、不在行上留状态标记：`ProviderHealthBadge` 那个徽章的数据源是
            `provider_health` 表，只被**真实转发流量**写入，而连通检测明确不碰它
            （`stream_check.rs` 开头就写了「不触碰故障转移熔断器」）⇒ 借那个徽章会显示假信息。 */}
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
          disabled={checking}
          onClick={onCheck}
          title={t("loongport.tier.checkConnectivity")}
        >
          {checking ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Activity className="h-3.5 w-3.5" />
          )}
        </Button>

        {/* 「编辑配置」：跳 cc-switch 现成的编辑页 —— 那页支持全部字段，我们不重做
            （CLAUDE.md §一）。点它先弹一道警告（保存后这个档位归用户自己维护），
            那个判断在宿主里，不在这儿：本组件不知道用户勾过「不再提示」没有。 */}
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

        {/* 「恢复默认配置」的 hover 版，给**没手动维护过**的档位。
            手动维护过的那些走下面常驻的那个 —— 两处只会渲染一个（`!userEdited`
            与 `userEdited` 互斥），不会同时出现两个恢复按钮。 */}
        {!userEdited && (
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

      {/* 「恢复默认配置」的**常驻**版，只给已手动维护的档位。
          普通档位那份在上面的 hover 组里。

          为什么分两处而不是都塞进 hover 组：对普通档位它是兜底（用户没改过，
          没什么可恢复的）；而对手动维护过的档位，它是那个状态**唯一的出口** ——
          用户改坏了配置要退回默认值，不该先猜「hover 一下会不会冒出个按钮」。
          维护者的原话是「在手输维护过的分组上常展示，方便用户随时恢复默认设置」。

          用 amber 而不是默认灰：与这一行的边框、标记同一个语义（见上面那段），
          让「哪个按钮能解除这个状态」一眼可见。 */}
      {userEdited && (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 p-1 text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-400 dark:hover:text-amber-300"
          disabled={resetting}
          onClick={onReset}
          // 说明「刷新不会覆盖 + 点这里恢复默认」这两件事都挂在这一个 title 上。
          //
          // 维护者原本提议再加一个 `?` 提示按钮，但那会是**第三个**说同一件事的
          // 信号（已有 amber 标记 + 这个常驻按钮），而且它自己也 hover 才出现 ——
          // 用户得先 hover 才知道有个东西要 hover。业界做法是把说明挂在已有的
          // 信号上：一个控件、一句话、零新增元素。
          title={t("loongport.tier.userEditedHint")}
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
