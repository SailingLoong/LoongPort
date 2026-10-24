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
export interface PlatformAccountSummary {
  source: string;
  platform: AccountPlatform;
  id: string;
  label: string;
  identity?: string;
  status?: string;
  app?: AppId;
}
export interface PlatformPresenceSnapshot {
  accounts?: PlatformAccountSummary[];
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
  const accounts: PlatformAccountSummary[] = [];
  const add = (
    source: string,
    platform: AccountPlatform,
    rows: {
      id: string;
      label?: string | null;
      status?: string;
      identity?: string;
      app?: AppId;
    }[],
  ) => {
    accounts.push(
      ...rows.map((row) => ({
        ...row,
        source,
        platform,
        label: row.label || row.id,
      })),
    );
    return rows.length ? [platform] : [];
  };
  const readers: {
    source: string;
    platforms: AccountPlatform[];
    connectionApp?: AppId;
    read: () => Promise<AccountPlatform[]>;
  }[] = [
    ...(["github_copilot", "codex_oauth", "xai_oauth"] as const).map(
      (platform) => ({
        source: platform,
        platforms: [platform],
        read: async () =>
          add(
            platform,
            platform,
            (await authApi.authGetStatus(platform)).accounts.map((row) => ({
              id: row.id,
              label: row.login,
            })),
          ),
      }),
    ),
    {
      source: "workbuddy",
      platforms: ["workbuddy"],
      read: async () =>
        add(
          "workbuddy",
          "workbuddy",
          (await workbuddyApi.list()).map((row) => ({
            id: row.id,
            label: row.label,
            status: row.claimState,
          })),
        ),
    },
    {
      source: "workbuddy-inference",
      platforms: ["workbuddy"],
      read: async () =>
        add(
          "workbuddy-inference",
          "workbuddy",
          (await workbuddyApi.listInferenceAccounts()).map((row) => ({
            id: row.id,
            status: row.state,
            identity: "HTTP",
          })),
        ),
    },
    {
      source: "minimax_code",
      platforms: ["minimax_code", "minimax_code_global"],
      read: async () => {
        const rows = await minimaxCodeApi.list();
        return ["cn", "global"].flatMap((region) =>
          add(
            "minimax_code",
            region === "cn" ? "minimax_code" : "minimax_code_global",
            rows
              .filter((row) => row.region === region)
              .map((row) => ({
                id: row.id,
                label: row.label,
                status: row.status,
                identity: row.region === "cn" ? "CN" : "Global",
              })),
          ),
        );
      },
    },
    {
      source: "trae",
      platforms: ["trae_cn", "trae"],
      read: async () => {
        const rows = (await traeAccountsApi.list()).filter(
          (row) => row.status !== "retired",
        );
        return [true, false].flatMap((cn) =>
          add(
            "trae",
            cn ? "trae_cn" : "trae",
            rows
              .filter((row) => (row.realm === "cn") === cn)
              .map((row) => ({
                id: row.id,
                status: row.status,
                identity: row.realm,
              })),
          ),
        );
      },
    },
    {
      source: "zcode",
      platforms: ["zcode"],
      read: async () =>
        add(
          "zcode",
          "zcode",
          (
            await zcodeAccountsApi.library(
              readZCodeSourcePreference()?.dataRoot || undefined,
            )
          ).profiles.map((row) => ({
            id: row.id,
            label: row.label,
            identity: row.family,
          })),
        ),
    },
    ...PROVIDER_STORE_APP_IDS.map((app) => ({
      source: `provider:${app}`,
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
          ) {
            present.add("qoder");
            add(`provider:${app}`, "qoder", [
              {
                id: provider.id,
                label: provider.name,
                identity:
                  provider.meta?.providerType === "qoder_sdk" ? "Global" : "CN",
              },
            ]);
          }
          // The persisted preset icon is explicit brand metadata, not a URL/name guess.
          if (provider.icon === "openrouter") {
            present.add("openrouter");
            add(`provider:${app}`, "openrouter", [
              { id: provider.id, label: provider.name, app },
            ]);
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
      accounts.push(
        ...(previous?.accounts ?? []).filter(
          (row) => row.source === readers[index].source,
        ),
      );
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
    accounts,
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
