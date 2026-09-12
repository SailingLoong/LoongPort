import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { proxyApi } from "@/lib/api/proxy";
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
  const onError = (error: unknown) =>
    toast.error(t("applications.updateFailed"), {
      description: extractErrorMessage(error),
    });
  const order = useMutation({
    mutationFn: (ids: string[]) => applicationRoutingApi.setOrder(appId, ids),
    onSuccess: refresh,
    onError,
  });
  const failover = useMutation({
    mutationFn: async (enabled: boolean) => {
      if (enabled) {
        // Read fresh state at click time; the view's cached status may be stale.
        const [status, takeover] = await Promise.all([
          proxyApi.getProxyStatus(),
          proxyApi.getProxyTakeoverStatus(),
        ]);
        if (!status.running) await proxyApi.startProxyServer();
        if (!takeover[appId as keyof typeof takeover])
          await proxyApi.setProxyTakeoverForApp(appId, true);
      }
      await applicationRoutingApi.setFailover(appId, enabled);
    },
    onSuccess: async () => {
      await Promise.all([
        refresh(),
        client.invalidateQueries({ queryKey: proxyKeys.status }),
        client.invalidateQueries({ queryKey: proxyKeys.takeoverStatus }),
        client.invalidateQueries({ queryKey: ["autoFailoverEnabled", appId] }),
        client.invalidateQueries({ queryKey: ["autoModeStatus", appId] }),
      ]);
    },
    onError,
  });
  const model = useMutation({
    mutationFn: (model: string | null) =>
      applicationRoutingApi.setModel(appId, model),
    onSuccess: refresh,
    onError,
  });
  return {
    ...query,
    busy: order.isPending || failover.isPending || model.isPending,
    setModel: model.mutateAsync,
    setOrder: order.mutateAsync,
    setFailover: failover.mutateAsync,
  };
}
