import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  applicationRoutingApi,
  isApplicationRoutingWriteBlocked,
  type ApplicationRoutingPreparedRequest,
  type ApplicationRoutingChange,
  type ApplicationRouting,
  type ApplicationModeState,
  type ApplicationRoutingRequest,
  type ApplicationRoutingRequestResult,
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

function routingChangeImage(change: ApplicationRoutingChange) {
  return JSON.stringify([
    change.order?.profileName,
    change.order?.providerIds,
    change.selection?.providerId,
    change.selection?.model,
    change.catalogDecision?.choice,
    change.catalogDecision?.revision,
    change.catalogPreviewOnly === true,
  ]);
}

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
  const [originalResult, setOriginalResult] =
    useState<ApplicationRoutingRequestResult | null>(null);
  const original = useRef<{
    change: string;
    prepared: ApplicationRoutingPreparedRequest;
  } | null>(null);
  const applying = useRef(false);
  const [checkingRequest, setCheckingRequest] = useState(false);
  const queryOriginal = async (request: ApplicationRoutingRequest) => {
    try {
      const result = await applicationRoutingApi.queryRequest(appId, request);
      if (
        result.request.id !== request.id ||
        result.request.scope !== request.scope ||
        result.request.draftDigest !== request.draftDigest ||
        result.request.revision !== request.revision
      )
        throw new Error("mode.invalid_save_request");
      setOriginalResult(result);
      return result;
    } catch {
      const result: ApplicationRoutingRequestResult = {
        request,
        status: "unknown",
      };
      setOriginalResult(result);
      return result;
    }
  };
  const prepareRequest = async (change: ApplicationRoutingChange) => {
    requireWritable();
    if (originalResult && originalResult.status !== "completed")
      throw new Error("mode.verification_required");
    setCheckingRequest(true);
    try {
      const prepared = await prepare(change);
      original.current = { change: routingChangeImage(change), prepared };
      await Promise.all([
        refresh(),
        client.invalidateQueries({ queryKey: ["applicationOverview", appId] }),
        client.invalidateQueries({ queryKey: ["orderProfiles", appId] }),
      ]);
    } finally {
      setCheckingRequest(false);
    }
  };
  const prepareCatalogConfirmation = async (
    change: ApplicationRoutingChange,
    result: import("@/lib/api/relay").SwitchTierCommandResult,
  ) => {
    if (
      result.status !== "confirmationRequired" ||
      !result.catalog ||
      !original.current
    )
      return;
    const current = original.current;
    const reviewed = {
      ...change,
      catalogPreviewOnly: false,
      catalogDecision: {
        choice: result.catalog.choice,
        revision: result.catalog.revision,
      },
    };
    const prepared = await prepare(
      reviewed,
      current.prepared.protocol === "native"
        ? current.prepared.request.id
        : undefined,
    );
    const previousRevision =
      current.prepared.protocol === "native"
        ? current.prepared.request.revision
        : current.prepared.review.revision;
    const revision =
      prepared.protocol === "native"
        ? prepared.request.revision
        : prepared.review.revision;
    if (
      prepared.protocol !== current.prepared.protocol ||
      revision !== previousRevision
    )
      throw new Error("mode.verification_required");
    original.current = { change: routingChangeImage(reviewed), prepared };
  };
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
  const prepare = async (
    change: ApplicationRoutingChange,
    requestId: string = crypto.randomUUID(),
  ) => {
    requireWritable();
    const prepared = await applicationRoutingApi.prepareRequest(
      appId,
      change,
      requestId,
    );
    requireWritable();
    const state = client.getQueryData<ApplicationRouting>(key)?.modeState;
    if (
      !prepared ||
      (prepared.protocol !== "native" && prepared.protocol !== "legacy") ||
      (state && prepared.protocol !== state.writeProtocol)
    )
      throw new Error("mode.verification_required");
    return prepared;
  };
  const onError = (error: unknown) =>
    toast.error(t("applications.updateFailed"), {
      description: extractErrorMessage(error),
    });
  const apply = useMutation({
    mutationFn: async ({
      change,
      quitChatgpt,
    }: {
      change: ApplicationRoutingChange;
      quitChatgpt?: boolean;
    }) => {
      requireWritable();
      if (change.catalogPreviewOnly) {
        const result = await applicationRoutingApi.apply(
          appId,
          change,
          quitChatgpt,
        );
        await prepareCatalogConfirmation(change, result);
        return result;
      }
      if (originalResult && originalResult.status !== "completed")
        throw new Error("mode.verification_required");
      const image = routingChangeImage(change);
      if (original.current && original.current.change !== image)
        throw new Error("mode.verification_required");
      if (!original.current) {
        const prepared = await prepare(change);
        original.current = { change: image, prepared };
      }
      const prepared = original.current.prepared;
      if (prepared.protocol === "legacy") {
        try {
          const result = await applicationRoutingApi.apply(
            appId,
            change,
            quitChatgpt,
            undefined,
            prepared.review,
          );
          await prepareCatalogConfirmation(change, result);
          if (result.status !== "confirmationRequired") original.current = null;
          return result;
        } catch (error) {
          // No durable receipt exists. Refresh in onSettled, retain the draft,
          // and require a new explicit user action rather than replaying it.
          original.current = null;
          throw error;
        }
      }
      const request = prepared.request;
      try {
        const result = await applicationRoutingApi.apply(
          appId,
          change,
          quitChatgpt,
          request,
        );
        if (result.status === "requestResolved") {
          if (
            result.result.request.id !== request.id ||
            result.result.request.scope !== request.scope ||
            result.result.request.draftDigest !== request.draftDigest ||
            result.result.request.revision !== request.revision
          )
            throw new Error("mode.invalid_save_request");
          setOriginalResult(result.result);
          if (result.result.status !== "completed")
            throw new Error("mode.verification_required");
          original.current = null;
          return {
            status: "switched" as const,
            providerName: "",
            warnings: [],
            chatgptWasRunning: false,
            chatgptRelaunched: false,
          };
        }
        await prepareCatalogConfirmation(change, result);
        if (result.status !== "confirmationRequired") {
          setOriginalResult({ request, status: "completed" });
          original.current = null;
        }
        return result;
      } catch (error) {
        const result = await queryOriginal(request);
        if (result.status === "completed") {
          original.current = null;
          return {
            status: "switched" as const,
            providerName: "",
            warnings: [],
            chatgptWasRunning: false,
            chatgptRelaunched: false,
          };
        }
        throw error;
      }
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
    writeBlocked:
      writeBlocked ||
      Boolean(originalResult && originalResult.status !== "completed"),
    originalResult,
    prepareRequest,
    recheckRequest: async () => {
      if (!originalResult || checkingRequest) return;
      setCheckingRequest(true);
      try {
        await queryOriginal(originalResult.request);
        await Promise.all([
          refresh(),
          client.invalidateQueries({
            queryKey: ["applicationOverview", appId],
          }),
          client.invalidateQueries({ queryKey: ["orderProfiles", appId] }),
        ]);
      } finally {
        setCheckingRequest(false);
      }
    },
    startNewRequest: () => {
      original.current = null;
      setOriginalResult(null);
    },
    busy:
      checkingRequest ||
      apply.isPending ||
      order.isPending ||
      failover.isPending ||
      blockTier.isPending ||
      resetTierErrors.isPending,
    apply: async (change: ApplicationRoutingChange, quitChatgpt?: boolean) => {
      requireWritable();
      if (applying.current) throw new Error("mode.verification_required");
      applying.current = true;
      try {
        return await apply.mutateAsync({ change, quitChatgpt });
      } finally {
        applying.current = false;
      }
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
