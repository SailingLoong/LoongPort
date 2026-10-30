import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { useQuery } from "@tanstack/react-query";
import { providersApi } from "@/lib/api/providers";
import type { Provider } from "@/types";

function sourceGeneration(provider: Provider): string | undefined {
  const state = (provider.meta as { qoderCampaignState?: unknown } | undefined)
    ?.qoderCampaignState;
  if (!state || typeof state !== "object" || !("generation" in state))
    return undefined;
  return typeof state.generation === "string" ? state.generation : undefined;
}

/** Projects original Provider records and the original providers Query cache. */
export function useQoderAccountPanel() {
  const active = usePreservedViewActive();
  const query = useQuery({
    queryKey: ["providers", "codex"],
    enabled: active,
    queryFn: async () => ({ providers: await providersApi.getAll("codex") }),
  });
  const accounts = Object.values(query.data?.providers ?? {}).filter(
    (provider) =>
      ["qoder_sdk", "qoder_cn", "qoder_cn_device"].includes(
        provider.meta?.providerType ?? "",
      ),
  );
  function details(provider: Provider) {
    const kind = provider.meta?.providerType;
    return {
      generation: sourceGeneration(provider),
      isInternational: kind === "qoder_sdk",
      usesDeviceLogin: kind === "qoder_cn_device",
      identityLabel:
        kind === "qoder_cn_device"
          ? "Qoder CN · 本应用独立授权"
          : kind === "qoder_cn"
            ? "Qoder CN · 独立 PAT"
            : "Qoder 国际版",
    };
  }
  return {
    active,
    accounts,
    details,
    pending: query.isPending,
    error: query.error,
    refreshing: query.isFetching,
    refresh: () => {
      if (active) return query.refetch();
    },
  };
}
