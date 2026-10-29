import { useProvidersQuery } from "@/lib/query/queries";
import { resolveManagedAccountId } from "@/lib/authBinding";

export interface CodexAccountProviderLink {
  id: string;
  name: string;
  presentation: { isCurrent: boolean };
}

/** Readonly associations; a default account or an application selection is not native identity. */
export function useCodexAccountProviderLinks(
  accountId: string,
  enabled: boolean,
) {
  const query = useProvidersQuery("codex", { enabled });
  const state: "loading" | "ready" | "error" = !enabled
    ? "ready"
    : query.isFetching || query.isPending
      ? "loading"
      : query.isError
        ? "error"
        : "ready";
  const providers: CodexAccountProviderLink[] =
    enabled && state === "ready"
      ? Object.values(query.data?.providers ?? {})
          .filter(
            (provider) =>
              provider.presentation?.isOfficial === true &&
              resolveManagedAccountId(provider.meta, "codex_oauth") ===
                accountId,
          )
          .map((provider) => ({
            id: provider.id,
            name: provider.name,
            presentation: {
              isCurrent: provider.presentation?.isCurrent === true,
            },
          }))
      : [];
  return {
    providers,
    state,
    retry: () => (enabled ? query.refetch() : undefined),
  };
}
