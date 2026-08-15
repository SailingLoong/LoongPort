import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2, RefreshCw, Scale } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { fmtUsd } from "@/components/usage/format";

import { relayApi, type ReconciliationWindow } from "@/lib/api/relay";
import { rowBalanceKeys } from "./useRowBalanceQuery";
import {
  reconciliationKeys,
  useReconciliationQuery,
} from "./useReconciliationQuery";

/**
 * 一行中转站的扣费对账弹窗。
 *
 * ## 前端只展示后端事实
 *
 * `suspicious` / `skippedTopUp` / `insufficientData` 全部由后端判定
 * （`relay_reconciliation` 的 `WindowFlag`），这里只把它们映射成徽标、文案与颜色；
 * `ratio` / `baselineRatio` 为 `null` 时**留空** —— 不猜、不算、不填 0
 * （`ratio = null` 意味着「没有可算的比值」，显示 0 会让用户以为估算免费）。
 *
 * ## 「立即采样」复用行级刷新链路，不新增后端命令
 *
 * 采样就是触发既有的 `relayApi.balance(relayId)`（后端查完余额顺手落一枚快照），
 * 完成后把结果写回 `rowBalanceKeys` 缓存（行上那条用量条跟着变新），再 invalidate
 * 对账 query 让 react-query 自己重拉报告。没有一个专门「采样」的命令。
 */
export interface ReconcileDialogProps {
  relayId: number;
  /** 行显示名（站名 / 账号）。弹窗描述行里定位「这是哪个账号的对账」。 */
  relayLabel: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** 窗口起止时刻。秒 → 本地短格式；同一格里两段挨着排。 */
function fmtWindowTime(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 比值。有值才显示两位小数；`null` 由调用处留空。 */
function fmtRatio(ratio: number): string {
  return ratio.toFixed(2);
}

/**
 * 状态徽标。`normal` 与 `insufficientData` 不出徽标 —— 前者无事可说，
 * 后者的语义已经由「比值留空」表达了（再标一个「数据不足」只是同一件事说两遍）。
 */
function FlagBadge({ flag }: { flag: ReconciliationWindow["flag"] }) {
  const { t } = useTranslation();

  if (flag === "suspicious") {
    return (
      <span
        className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-red-600 ring-1 ring-inset ring-red-500/30 dark:text-red-400"
        title={t("loongport.reconcile.flagSuspiciousHint")}
      >
        {t("loongport.reconcile.flagSuspicious")}
      </span>
    );
  }
  if (flag === "skippedTopUp") {
    return (
      <span
        className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground ring-1 ring-inset ring-border"
        title={t("loongport.reconcile.flagSkippedTopUpHint")}
      >
        {t("loongport.reconcile.flagSkippedTopUp")}
      </span>
    );
  }
  return null;
}

export function ReconcileDialog({
  relayId,
  relayLabel,
  open,
  onOpenChange,
}: ReconcileDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // 弹窗没开就不查：报告是「点开对账」这个动作的产物，不该在页面加载时预取。
  const { report, loading, error, refetch } = useReconciliationQuery(relayId, {
    enabled: open,
  });
  // 「立即采样」的进行态。作用域只在这个弹窗里，不需要走 `useRowBusy`
  // —— 那套集合服务的是「多行并列、各自的按钮互不干扰」，这里天然只有一个按钮。
  const [sampling, setSampling] = useState(false);

  const sampleNow = async () => {
    if (sampling) return;
    setSampling(true);
    try {
      const balance = await relayApi.balance(relayId);
      // 采样顺带刷新了余额 —— 把结果写回行上那条用量条的缓存，让两个视图同源。
      queryClient.setQueryData(rowBalanceKeys.row("relay", relayId), balance);
      await queryClient.invalidateQueries({
        queryKey: reconciliationKeys.report(relayId),
      });
    } catch (e) {
      // 用户明确点了「立即采样」，采样没做成要让他知道（否则按钮点完毫无反应）。
      toast.error(String(e));
    } finally {
      setSampling(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Scale className="h-4 w-4 text-muted-foreground" />
            {t("loongport.reconcile.title")}
          </DialogTitle>
          <DialogDescription className="truncate">
            {relayLabel}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-6 py-4">
          {/* 摘要行：快照数 + 基线比值 + 立即采样。
              ⚠️ baselineRatio 为 null 时值留空 —— 不足 3 个有效窗口判不出基线，
              填 0 或 "--" 都是在替后端猜事实。 */}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 text-sm">
            <span className="text-muted-foreground">
              {report
                ? t("loongport.reconcile.snapshotCount", {
                    count: report.snapshotCount,
                  })
                : ""}
            </span>
            <span className="flex items-center gap-1.5 text-muted-foreground">
              {t("loongport.reconcile.baselineRatioLabel")}
              <span className="tabular-nums">
                {report?.baselineRatio != null
                  ? fmtRatio(report.baselineRatio)
                  : ""}
              </span>
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="ml-auto h-7 gap-1"
              disabled={sampling}
              onClick={() => void sampleNow()}
            >
              {sampling ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
              {t("loongport.reconcile.sampleNow")}
            </Button>
          </div>

          {/* 首次加载：居中转圈。后台刷新（loading 且已有 report）不打断表格 ——
              keep-last-good 语义，见 useReconciliationQuery 的文档。 */}
          {loading && !report ? (
            <div className="flex justify-center py-8">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          ) : error ? (
            <div className="flex flex-col items-center gap-2 py-6 text-sm text-red-500">
              <span>
                {t("loongport.reconcile.loadFailed")}：{error}
              </span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7"
                onClick={() => void refetch()}
              >
                {t("loongport.reconcile.retry")}
              </Button>
            </div>
          ) : report && report.windows.length > 0 ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="h-9 px-3">
                    {t("loongport.reconcile.colWindow")}
                  </TableHead>
                  <TableHead className="h-9 px-3 text-right">
                    {t("loongport.reconcile.colEstimatedCost")}
                  </TableHead>
                  <TableHead className="h-9 px-3 text-right">
                    {t("loongport.reconcile.colBalanceChange")}
                  </TableHead>
                  <TableHead className="h-9 px-3 text-right">
                    {t("loongport.reconcile.colRatio")}
                  </TableHead>
                  <TableHead className="h-9 px-3">
                    {t("loongport.reconcile.colStatus")}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {/* 报告本身按新 → 旧排好，这里照序渲染。 */}
                {report.windows.map((win) => (
                  <TableRow key={`${win.startSecs}-${win.endSecs}`}>
                    <TableCell className="whitespace-nowrap px-3 py-2 text-xs text-muted-foreground">
                      {fmtWindowTime(win.startSecs)} –{" "}
                      {fmtWindowTime(win.endSecs)}
                    </TableCell>
                    <TableCell className="px-3 py-2 text-right tabular-nums">
                      {fmtUsd(win.estimatedCostUsd, 4)}
                    </TableCell>
                    {/* 负数 = 扣减（正常用量），正数 = 充值/返利。
                        用 tabular-nums 让 +/- 对齐。 */}
                    <TableCell className="px-3 py-2 text-right tabular-nums">
                      {fmtUsd(win.balanceDeltaUsd, 2)}
                    </TableCell>
                    {/* ⚠️ ratio 为 null 时留空（不填 0 / "--"），见组件文档。 */}
                    <TableCell className="px-3 py-2 text-right tabular-nums">
                      {win.ratio != null ? fmtRatio(win.ratio) : ""}
                    </TableCell>
                    <TableCell className="px-3 py-2">
                      <FlagBadge flag={win.flag} />
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : (
            <p className="py-6 text-center text-sm text-muted-foreground">
              {t("loongport.reconcile.empty")}
            </p>
          )}
        </div>

        {/* 一行中立说明：解释比值是什么、什么时候才值得看。
            口径是「不自动定罪」—— 偏低 ≠ 站点算错了，持续显著偏低才值得关注。 */}
        <p className="flex-shrink-0 border-t border-border-default px-6 py-3 text-xs leading-relaxed text-muted-foreground">
          {t("loongport.reconcile.note")}
        </p>
      </DialogContent>
    </Dialog>
  );
}
