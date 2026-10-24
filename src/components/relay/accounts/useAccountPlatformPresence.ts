import { useQuery, useQueryClient } from "@tanstack/react-query";
import { readZCodeSourcePreference } from "@/lib/zcodeSourcePreference";
import { authApi } from "@/lib/api/auth";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { minimaxCodeApi } from "@/lib/api/minimaxCode";
import { traeAccountsApi } from "@/lib/api/traeAccounts";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
import { providersApi } from "@/lib/api/providers";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { PROVIDER_SWITCHED, VENDOR_ACCOUNTS_CHANGED } from "@/lib/api/events";
import type { AccountPlatform } from "./accountPlatforms";
import type { AppId } from "@/lib/api/types";

export interface PlatformConnection {
  id: string;
  name: string;
  app: AppId;
  platform: "openrouter";
}
export interface PlatformPresenceSnapshot {
  platforms: AccountPlatform[];
  unavailable: AccountPlatform[];
  connections: PlatformConnection[];
}
export const accountPlatformPresenceKey = ["accountPlatformPresence"] as const;

/** Local read-only projection for navigation; never owns credentials or account policy. */
export async function readAccountPlatformPresence(
  previous?: PlatformPresenceSnapshot,
): Promise<PlatformPresenceSnapshot> {
  const connections: PlatformConnection[] = [];
  const readers: {
    platforms: AccountPlatform[];
    connectionApp?: AppId;
    read: () => Promise<AccountPlatform[]>;
  }[] = [
    ...(["github_copilot", "codex_oauth", "xai_oauth"] as const).map(
      (platform) => ({
        platforms: [platform],
        read: async () =>
          (await authApi.authGetStatus(platform)).accounts.length
            ? [platform]
            : [],
      }),
    ),
    {
      platforms: ["workbuddy"],
      read: async () =>
        (await workbuddyApi.list()).length ? ["workbuddy"] : [],
    },
    {
      platforms: ["workbuddy"],
      read: async () =>
        (await workbuddyApi.listInferenceAccounts()).length
          ? ["workbuddy"]
          : [],
    },
    {
      platforms: ["minimax_code", "minimax_code_global"],
      read: async () => [
        ...new Set(
          (await minimaxCodeApi.list())
            .filter(
              (account) =>
                account.region === "cn" || account.region === "global",
            )
            .map((account): AccountPlatform =>
              account.region === "cn" ? "minimax_code" : "minimax_code_global",
            ),
        ),
      ],
    },
    {
      platforms: ["trae_cn", "trae"],
      read: async () => [
        ...new Set(
          (await traeAccountsApi.list())
            .filter((account) => account.status !== "retired")
            .map((account): AccountPlatform =>
              account.realm === "cn" ? "trae_cn" : "trae",
            ),
        ),
      ],
    },
    {
      platforms: ["zcode"],
      read: async () =>
        (
          await zcodeAccountsApi.library(
            readZCodeSourcePreference()?.dataRoot || undefined,
          )
        ).profiles.length
          ? ["zcode"]
          : [],
    },
    ...PROVIDER_STORE_APP_IDS.map((app) => ({
      platforms: (app === "codex"
        ? ["qoder", "openrouter"]
        : ["openrouter"]) as AccountPlatform[],
      connectionApp: app,
      read: async () => {
        const present = new Set<AccountPlatform>();
        for (const provider of Object.values(await providersApi.getAll(app))) {
          if (
            app === "codex" &&
            ["qoder_sdk", "qoder_cn", "qoder_cn_device"].includes(
              provider.meta?.providerType ?? "",
            )
          )
            present.add("qoder");
          // The persisted preset icon is explicit brand metadata, not a URL/name guess.
          if (provider.icon === "openrouter") {
            present.add("openrouter");
            connections.push({
              id: provider.id,
              name: provider.name,
              app,
              platform: "openrouter",
            });
          }
        }
        return [...present];
      },
    })),
  ];
  const results = await Promise.allSettled(
    readers.map((reader) => reader.read()),
  );
  const platforms = new Set<AccountPlatform>();
  const unavailable = new Set<AccountPlatform>();
  const unavailableApps = new Set<AppId>();
  results.forEach((result, index) => {
    if (result.status === "fulfilled")
      result.value.forEach((platform) => platforms.add(platform));
    else {
      readers[index].platforms.forEach((platform) => unavailable.add(platform));
      const app = readers[index].connectionApp;
      if (app) unavailableApps.add(app);
    }
  });
  // Failed reads are not proof an imported platform disappeared. Keep its
  // presentation mounted (and its in-progress native dialogs alive) until a
  // successful inventory read actually confirms absence.
  for (const platform of previous?.platforms ?? []) {
    if (platform !== "openrouter" && unavailable.has(platform))
      platforms.add(platform);
  }
  for (const connection of previous?.connections ?? []) {
    if (
      unavailableApps.has(connection.app) &&
      !connections.some(
        (current) =>
          current.app === connection.app && current.id === connection.id,
      )
    )
      connections.push(connection);
  }
  // Connection presence follows its exact app owner, never an unrelated failure.
  if (connections.length) platforms.add("openrouter");
  else platforms.delete("openrouter");
  return {
    platforms: [...platforms],
    unavailable: [...unavailable],
    connections,
  };
}

export function useAccountPlatformPresence() {
  const active = usePreservedViewActive();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: accountPlatformPresenceKey,
    queryFn: () =>
      readAccountPlatformPresence(
        client.getQueryData<PlatformPresenceSnapshot>(
          accountPlatformPresenceKey,
        ),
      ),
    enabled: active,
    // Native mutations remain authoritative; periodically read local inventories only.
    refetchInterval: active ? 15_000 : false,
  });
  useTauriEvent(PROVIDER_SWITCHED, async () => {
    await query.refetch();
  });
  useTauriEvent(VENDOR_ACCOUNTS_CHANGED, async () => {
    await query.refetch();
  });
  return query;
}
