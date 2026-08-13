import { useRef } from "react";
import { useQuery } from "@tanstack/react-query";

import { relayApi } from "@/lib/api";
import { vendorApi } from "@/lib/api/vendor";
import {
  KEEP_LAST_GOOD_MS,
  resolveDisplayUsage,
  type LastGoodUsage,
} from "@/lib/query/queries";
import type { UsageResult } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 一行（中转站 / 官网）的余额查询。
 *
 * ## 为什么是 react-query，而不是原来那两个 effect
 *
 * 原来余额由 `RelaySection` 里两份 state（`balances` / `vendorBalances`）+ 两个
 * effect 拉，依赖键是 `id:accountLabel`。那个形状有一个**死路**：某一行拉失败过
 * 一次、键又不变 ⇒ effect 永远不会再跑 ⇒ 那一行整个会话都没有余额；而充值按钮
 * 只在有余额时渲染 ⇒ 用户连重试的入口都看不到。
 *
 * 换成 react-query 之后，「重查」是一个 `refetch()`，用量条上就有那个按钮。
 *
 * ## 缓存语义**复用** `useUsageQuery` 那一套，不新写
 *
 * `retry: 1` / `staleTime` 5 分钟 / keep-last-good（[`resolveDisplayUsage`]）
 * 全部来自 `src/lib/query/queries.ts` 已经 export 的东西。余额与 provider 页的用量
 * 是同一类事实（跨境第三方端点、单次抖动不该判死），两套策略迟早分叉。
 */

export type BalanceRowKind = "relay" | "vendor";

/** react-query 的键。`all` 供 `invalidateQueries` 一次刷全部行（充值窗关闭时用）。 */
export const rowBalanceKeys = {
  all: ["rowBalance"] as const,
  row: (kind: BalanceRowKind, rowId: number) =>
    [...rowBalanceKeys.all, kind, rowId] as const,
};

export interface UseRowBalanceQueryOptions {
  /** 关掉查询（如这一行还没登录过、根本没有可查的东西）。 */
  enabled?: boolean;
}

/**
 * 查一行的余额。
 *
 * ⚠️ **不因「登录态过期」而 disable** —— 那正是本轮要修的：sk 是独立凭据，后端那条
 * 回落链前两步都不需要登录态（见 `src-tauri/src/relay/balance.rs`）。这里把行关掉
 * 等于在前端把后端刚修好的能力又堵死一次。
 */
export function useRowBalanceQuery(
  kind: BalanceRowKind,
  rowId: number,
  options: UseRowBalanceQueryOptions = {},
) {
  const { enabled = true } = options;

  const query = useQuery<UsageResult>({
    queryKey: rowBalanceKeys.row(kind, rowId),
    queryFn: async () =>
      kind === "relay" ? relayApi.balance(rowId) : vendorApi.balance(rowId),
    enabled,
    refetchOnWindowFocus: false,
    // 与 `useUsageQuery` 同语义：后端只在**瞬时传输失败**时 reject，retry 在那时
    // 才真正有意义；确定性失败以 `success:false` 回来，立即透出。
    retry: 1,
    retryDelay: 1500,
    staleTime: 5 * 60 * 1000,
    gcTime: KEEP_LAST_GOOD_MS,
  });

  // keep-last-good：失败时在窗口内继续显示上一次成功的余额。每个 hook 实例各持一份
  // ref（按行维度），写入幂等。
  const lastGoodRef = useRef<LastGoodUsage | null>(null);
  const { data, lastQueriedAt, lastGood } = resolveDisplayUsage(
    query.data,
    query.dataUpdatedAt,
    lastGoodRef.current,
    Date.now(),
    { rejected: query.isError },
  );
  lastGoodRef.current = lastGood;

  return {
    // reject 且无可展示值：合成一个失败占位，让用量条渲染失败态 + 重试入口，
    // 而不是整块消失（照 `useUsageQuery` 结尾那段）。
    usage:
      data ??
      (query.isError
        ? {
            success: false,
            error: extractErrorMessage(query.error) || undefined,
          }
        : undefined),
    loading: query.isFetching,
    lastQueriedAt,
    refetch: async () => {
      await query.refetch();
    },
  };
}
