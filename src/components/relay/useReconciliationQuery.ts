import { useQuery } from "@tanstack/react-query";

import { relayApi, type ReconciliationReport } from "@/lib/api/relay";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 一行中转站的扣费对账报告查询。
 *
 * 缓存语义照 `useRowBalanceQuery` 那一套（`retry: 1` / `staleTime` 5 分钟 /
 * refetchOnWindowFocus 关 / keep-last-good）。keep-last-good 这里不另起 ref：
 * 对账报告整份替换、没有字段级合并，而 react-query v5 本身就在后台刷新失败时
 * 保留上一份 `data`（status 仍为 success）—— 手里再存一份是死代码。
 */

/** react-query 的键。`all` 供 `invalidateQueries` 一次刷全部行。 */
export const reconciliationKeys = {
  all: ["reconciliation"] as const,
  report: (relayId: number) => [...reconciliationKeys.all, relayId] as const,
};

export interface UseReconciliationQueryOptions {
  /** 关掉查询（如对账入口还没打开）。 */
  enabled?: boolean;
}

export function useReconciliationQuery(
  relayId: number,
  options: UseReconciliationQueryOptions = {},
) {
  const { enabled = true } = options;

  const query = useQuery<ReconciliationReport>({
    queryKey: reconciliationKeys.report(relayId),
    queryFn: () => relayApi.reconciliation(relayId),
    enabled,
    refetchOnWindowFocus: false,
    // 本地库读，失败基本是确定性错误；1 次重试只为吸收偶发锁竞争。
    retry: 1,
    retryDelay: 1500,
    staleTime: 5 * 60 * 1000,
  });

  return {
    report: query.data,
    loading: query.isFetching,
    error: query.isError ? extractErrorMessage(query.error) : null,
    refetch: async () => {
      await query.refetch();
    },
  };
}
