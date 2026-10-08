import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  applicationRoutingApi,
  type ApplicationRoutingChange,
  type ApplicationRouting,
  type ApplicationModeState,
} from "@/lib/api/applicationRouting";
import { failoverApi } from "@/lib/api/failover";
import { proxyKeys } from "@/lib/query/proxy";
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

function errorModeState(error: unknown): ApplicationModeState | undefined {
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
  const modeState = errorModeState(query.error) ?? query.data?.modeState;
  const writeBlocked =
    !query.data || Boolean(query.error) || modeState?.canWrite === false;
  // Read the existing cache at invocation time, including an already-open confirmation.
  const requireWritable = () => {
    const current = client.getQueryState<ApplicationRouting>(key);
    if (
      !current?.data ||
      current.error ||
      current.data.modeState?.canWrite === false
    )
      throw new Error("mode.verification_required");
  };
  const onError = (error: unknown) =>
    toast.error(t("applications.updateFailed"), {
      description: extractErrorMessage(error),
    });
  const apply = useMutation({
    mutationFn: ({
      change,
      quitChatgpt,
    }: {
      change: ApplicationRoutingChange;
      quitChatgpt?: boolean;
    }) => {
      requireWritable();
      return applicationRoutingApi.apply(appId, change, quitChatgpt);
    },
    onSettled: async (result) => {
      if (result?.status === "confirmationRequired") return;
      await Promise.all([
        refresh(),
        client.invalidateQueries({ queryKey: ["applicationOverview", appId] }),
        client.invalidateQueries({ queryKey: ["providers", appId] }),
        client.invalidateQueries({ queryKey: ["orderProfiles", appId] }),
        client.invalidateQueries({ queryKey: ["autoModeStatus", appId] }),
      ]);
    },
    onError,
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
  const failover = useMutation({
    mutationFn: (enabled: boolean) => {
      requireWritable();
      return applicationRoutingApi.setFailover(appId, enabled);
    },
    onSuccess: (_data, enabled) => {
      // 开启即生效（未初始化的链回落=当前全量显示序，开关同时带起代理与接管）；
      // toast 只报「已生效」——干净视图下没有可应用的挂起，弹「尚未生效」是假话。
      if (enabled) toast.success(t("applications.failoverEnabled"));
    },
    onSettled: async () => {
      await Promise.all([
        client.invalidateQueries({ queryKey: ["applicationOverview", appId] }),
        client.invalidateQueries({ queryKey: ["providers", appId] }),
        refresh(),
        client.invalidateQueries({ queryKey: proxyKeys.status }),
        client.invalidateQueries({ queryKey: proxyKeys.takeoverStatus }),
        client.invalidateQueries({ queryKey: ["autoFailoverEnabled", appId] }),
        client.invalidateQueries({ queryKey: ["autoModeStatus", appId] }),
      ]);
    },
    onError,
  });
  return {
    ...query,
    modeState,
    writeBlocked,
    busy:
      apply.isPending ||
      order.isPending ||
      failover.isPending ||
      blockTier.isPending ||
      resetTierErrors.isPending,
    apply: async (change: ApplicationRoutingChange, quitChatgpt?: boolean) => {
      requireWritable();
      return apply.mutateAsync({ change, quitChatgpt });
    },
    setOrder: async (ids: string[]) => {
      requireWritable();
      return order.mutateAsync(ids);
    },
    setFailover: async (enabled: boolean) => {
      requireWritable();
      return failover.mutateAsync(enabled);
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
