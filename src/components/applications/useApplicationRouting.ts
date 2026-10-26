import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  applicationRoutingApi,
  isApplicationRoutingWriteBlocked,
  type ApplicationRouting,
  type ApplicationModeState,
} from "@/lib/api/applicationRouting";
import { failoverApi } from "@/lib/api/failover";
import type { AppId } from "@/lib/api";
import {
  PROVIDER_SWITCHED,
  PROVIDER_MODELS_UPDATED,
  SITE_BALANCES_UPDATED,
  VENDOR_ACCOUNTS_CHANGED,
  USAGE_LOG_RECORDED,
} from "@/lib/api/events";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { extractErrorMessage } from "@/utils/errorUtils";

export function applicationRoutingErrorModeState(
  error: unknown,
): ApplicationModeState | undefined {
  if (!error || typeof error !== "object" || !("modeState" in error)) return;
  const state = error.modeState;
  if (
    !state ||
    typeof state !== "object" ||
    !("status" in state) ||
    !("canWrite" in state)
  )
    return;
  if (
    (state.status === "pending" || state.status === "unknown") &&
    state.canWrite === false
  )
    return state as ApplicationModeState;
}

/** Routing facts and original immediate table actions; C2 owns route publication. */
export function useApplicationRouting(appId: AppId) {
  const client = useQueryClient();
  const { t } = useTranslation();
  const key = ["applicationRouting", appId];
  const refresh = () => client.invalidateQueries({ queryKey: key });
  useTauriEvent(PROVIDER_SWITCHED, refresh);
  useTauriEvent(PROVIDER_MODELS_UPDATED, refresh);
  useTauriEvent(SITE_BALANCES_UPDATED, refresh);
  useTauriEvent(VENDOR_ACCOUNTS_CHANGED, refresh);
  useTauriEvent(USAGE_LOG_RECORDED, refresh);
  const query = useQuery({
    queryKey: key,
    queryFn: () => applicationRoutingApi.get(appId),
    refetchInterval: 5000,
  });
  const modeState =
    applicationRoutingErrorModeState(query.error) ?? query.data?.modeState;
  const writeBlocked =
    !query.data ||
    Boolean(query.error) ||
    isApplicationRoutingWriteBlocked(appId, modeState);
  // Read the existing cache at invocation time, including an already-open confirmation.
  const requireWritable = () => {
    const current = client.getQueryState<ApplicationRouting>(key);
    if (
      !current?.data ||
      current.error ||
      isApplicationRoutingWriteBlocked(appId, current.data.modeState)
    )
      throw new Error("mode.verification_required");
  };
  const onError = (error: unknown) =>
    toast.error(t("applications.updateFailed"), {
      description: extractErrorMessage(error),
    });
  const order = useMutation({
    mutationFn: (ids: string[]) => {
      requireWritable();
      return applicationRoutingApi.setOrder(appId, ids);
    },
    onSuccess: refresh,
    onError,
  });
  // 屏蔽即时生效（用户显式动作，不等顺序确认）；写入后刷新看板与选路状态。
  const blockTier = useMutation({
    mutationFn: ({
      providerId,
      blocked,
    }: {
      providerId: string;
      blocked: boolean;
    }) => {
      requireWritable();
      return applicationRoutingApi.setTierBlocked(appId, providerId, blocked);
    },
    onSettled: () =>
      Promise.all([
        refresh(),
        client.invalidateQueries({ queryKey: ["applicationOverview", appId] }),
      ]),
    onError,
  });
  // 清除档位错误记录（内存熔断器 + DB 健康行）：用户显式动作，清完立刻
  // 重新参与选路。复用 proxy 侧现成的 reset_circuit_breaker 命令——语义就是「错误置空」。
  const resetTierErrors = useMutation({
    mutationFn: ({ providerId }: { providerId: string }) => {
      requireWritable();
      return failoverApi.resetCircuitBreaker(providerId, appId);
    },
    onSuccess: refresh,
    onError,
  });
  return {
    ...query,
    modeState,
    writeBlocked,
    busy: order.isPending || blockTier.isPending || resetTierErrors.isPending,
    setOrder: async (ids: string[]) => {
      requireWritable();
      return order.mutateAsync(ids);
    },
    blockTier: async (input: { providerId: string; blocked: boolean }) => {
      requireWritable();
      return blockTier.mutateAsync(input);
    },
    resetTierErrors: async (input: { providerId: string }) => {
      requireWritable();
      return resetTierErrors.mutateAsync(input);
    },
  };
}
