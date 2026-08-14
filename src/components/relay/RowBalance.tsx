import { useTranslation } from "react-i18next";
import { AlertCircle, Loader2, Wallet } from "lucide-react";

import { InlineUsage } from "@/components/UsageFooter";
import { cn } from "@/lib/utils";

import { useRowBalanceQuery, type BalanceRowKind } from "./useRowBalanceQuery";

interface RowBalanceProps {
  rowKind: BalanceRowKind;
  rowId: number;
  /**
   * 这一行有没有东西可查。
   *
   * ⚠️ **不是「登录态还有效」** —— sk 是独立凭据，登录态过期时后端照样查得到
   * （`relay::balance` 的前两步不需要它）。只有「从没登录过」的行才真的无从查起。
   */
  enabled: boolean;
  /**
   * 带登录态开这一行的充值页。
   *
   * ⚠️ **官网行不传** —— 官网充值要走厂商自己的收银台，我们没有对应命令（也不该做）。
   * 摆一个点了没反应、或跳到别处的按钮是骗人。不传时这一块只有用量条。
   */
  onPurchase?: () => void;
  /** 充值窗正在开（那一下按钮转圈）。传了 `onPurchase` 才有意义。 */
  purchaseBusy?: boolean;
  /** 行级远端同步。传入时与额度重查合并为同一个刷新入口。 */
  onRefresh?: () => void | Promise<void>;
  /** 行级统一刷新入口的完整语义。 */
  refreshLabel?: string;
  /** 行级远端同步正在执行。 */
  refreshBusy?: boolean;
}

/**
 * 一行（中转站 / 官网）的余额区：provider 页那条用量条 + 一个独立的充值按钮。
 *
 * ## 为什么用量条直接抄 provider 页
 *
 * 维护者的要求是「复用 ccswitch 的实现，以及 provider 页面的样子」。呈现件就是
 * 上游那个 [`InlineUsage`]（从 `UsageFooter` 抽出来的，见那边的文档），数据走
 * [`useRowBalanceQuery`]（复用 `useUsageQuery` 的缓存语义）。这一层只负责把两者
 * 拼起来，再摆一个充值入口。
 *
 * ## ⚠️ 充值为什么现在是**独立按钮**（推翻 2026-08-04 的结论）
 *
 * 当时的做法是「余额数字本身就是充值按钮，低余额时把它切成琥珀叹号」，理由是
 * 「再加一个按钮的话，两个紧邻的控件点下去做同一件事」。
 *
 * **那个前提这一轮不成立了**：余额数字归了用量条 —— 它是一段**只读**的展示
 * （还带着上次查询时间与刷新按钮），把它做成「点一下跳充值页」会让用户在想刷新
 * 的时候误开一个付款窗口。数字与充值从此是两件事，两个控件语义不同，不再是
 * 「点下去做同一件事」。
 *
 * 当时那条顾虑（相邻按钮只有一个 `stopPropagation` 会误折叠整行）仍然成立，
 * 所以**两个控件都停冒泡**：刷新按钮在 [`InlineUsage`] 里停，充值按钮在这里停。
 *
 * 低余额的警示态照旧：钱包图标换叹号、转琥珀、title 换成催充值的那句 ——
 * 需求原文那个「余额旁边有个小叹号，点它跳充值页」一字不差地保留。
 */
export function RowBalance({
  rowKind,
  rowId,
  enabled,
  onPurchase,
  purchaseBusy = false,
  onRefresh,
  refreshLabel,
  refreshBusy = false,
}: RowBalanceProps) {
  const { t } = useTranslation();
  const { usage, shouldPromptTopUp, loading, lastQueriedAt, refetch } =
    useRowBalanceQuery(rowKind, rowId, { enabled });

  // 后端没有确认可查询凭据的行，不渲染一个必然失败的用量条。
  if (!enabled) return null;

  const handleRefresh = async () => {
    if (onRefresh) {
      await onRefresh();
    } else {
      await refetch();
    }
  };

  return (
    <div className="flex shrink-0 items-center gap-1">
      <InlineUsage
        usage={usage}
        loading={loading || refreshBusy}
        lastQueriedAt={lastQueriedAt}
        onRefresh={handleRefresh}
        refreshLabel={refreshLabel}
        singleLine
        // 预留「已用」展示位：LoongPort 当前没有权威数据源，后端具备能力后再打开。
        showUsed={false}
      />

      {onPurchase && (
        <button
          type="button"
          onClick={(e) => {
            // 这个按钮在折叠触发区**外面**，但与整行的点击区相邻 ——
            // 不阻止冒泡的话点它会顺带折叠这一行。
            e.stopPropagation();
            onPurchase();
          }}
          disabled={purchaseBusy}
          // title 挂在按钮自身：它 disabled 的时间极短（就是开窗那一下），
          // 不像上游那几个常驻 disabled 的按钮需要 wrapper 承接 hover。
          title={
            shouldPromptTopUp
              ? t("loongport.row.lowBalanceHint")
              : t("loongport.row.purchaseHint")
          }
          className={cn(
            "flex shrink-0 items-center rounded-md p-1 transition-colors",
            // 低余额：琥珀色常驻（不是 hover 才出）—— 它是个提醒，藏起来就没用了。
            // 用琥珀而不是红：钱不够是「该处理一下」，不是「出错了」。
            shouldPromptTopUp
              ? "text-amber-600 hover:bg-amber-50 dark:text-amber-500 dark:hover:bg-amber-950/40"
              : "text-muted-foreground hover:bg-muted/60 hover:text-blue-500 dark:hover:text-blue-400",
            purchaseBusy && "cursor-not-allowed opacity-60",
          )}
        >
          {purchaseBusy ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : shouldPromptTopUp ? (
            // 叹号替掉钱包 —— 需求要的那个「小叹号」就是它。
            <AlertCircle className="h-3.5 w-3.5" />
          ) : (
            <Wallet className="h-3.5 w-3.5" />
          )}
        </button>
      )}
    </div>
  );
}
