import type { AppId } from "@/lib/api/types";
import type { SubscriptionWindow } from "@/lib/api/applicationRouting";
import type { ProviderWindowStatsEntry } from "@/lib/api/usage";
import type { AccountActionTarget } from "../RelaySection";
import type { ServiceAccount } from "./useServiceAccounts";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";

export type ServiceAccountScope = "opencode" | "relay-and-vendor";
export type ServiceAccountSelection = {
  kind: ServiceAccount["kind"];
  id: number;
  appId: AppId;
};
export function inServiceAccountScope(
  account: ServiceAccount,
  scope?: ServiceAccountScope,
) {
  const opencode =
    account.kind === "vendor" && account.row.vendorId === "opencode";
  return scope === "opencode"
    ? opencode
    : scope === "relay-and-vendor"
      ? !opencode
      : true;
}
export function serviceAccountSnapshot(
  account: ServiceAccount | undefined,
  appId: AppId,
): AccountActionTarget | null {
  if (!account) return null;
  if (account.kind === "relay") {
    const row = account.apps.get(appId);
    return row ? { kind: "relay", row, appId } : null;
  }
  const row = account.apps.get(appId);
  return row ? { kind: "vendor", row, appId } : null;
}
export function serviceAccountName(account: ServiceAccount) {
  return account.kind === "relay"
    ? account.row.siteName
    : account.row.vendorName;
}

/** App context is reported inventory only; it never changes provider configuration. */
export function projectServiceAccounts(
  accounts: ServiceAccount[],
  appId: AppId,
) {
  return accounts.flatMap((account) => {
    const contextApp = account.apps.has(appId)
      ? appId
      : account.apps.keys().next().value;
    if (!contextApp) return [];
    const actionTarget = serviceAccountSnapshot(account, contextApp);
    if (!actionTarget) return [];
    return [
      {
        key: `${account.kind}:${account.id}`,
        name: serviceAccountName(account),
        account,
        contextApp,
        actionTarget,
        apps: configuredApps(account),
      },
    ];
  });
}

export type SubscriptionWindowRow = SubscriptionWindow & {
  key: string;
  usedLimit: string;
};
/** Preserve window reset semantics: these are not credit-batch expiry timestamps. */
export function projectSubscriptionWindows(
  tiers: { tiers: { subscriptionWindows: SubscriptionWindow[] }[] }[],
): SubscriptionWindowRow[] {
  const windows = new Map<string, SubscriptionWindowRow>();
  for (const row of tiers)
    for (const tier of row.tiers)
      for (const window of tier.subscriptionWindows) {
        const key = JSON.stringify(window);
        if (!windows.has(key))
          windows.set(key, {
            ...window,
            key,
            usedLimit: `${window.usedUsd == null ? "—" : window.usedUsd.toFixed(2)} / ${window.limitUsd.toFixed(2)}`,
          });
      }
  return [...windows.values()].sort(
    (a, b) =>
      (a.resetAt ?? Number.MAX_SAFE_INTEGER) -
      (b.resetAt ?? Number.MAX_SAFE_INTEGER),
  );
}
export function projectAccountUsage(
  entries: { providerId: string; displayName: string }[],
  data: ProviderWindowStatsEntry[],
) {
  const byId = new Map(data.map((entry) => [entry.providerId, entry]));
  const rows = entries
    .map((entry) => ({
      ...entry,
      cost7: byId.get(entry.providerId)?.stats.costUsd7d ?? 0,
      cost30: byId.get(entry.providerId)?.stats.costUsd30d ?? 0,
    }))
    .filter((row) => row.cost30 > 0);
  if (!rows.length) return null;
  const total7 = rows.reduce((sum, row) => sum + row.cost7, 0),
    total30 = rows.reduce((sum, row) => sum + row.cost30, 0),
    max = Math.max(...rows.map((row) => row.cost30));
  return {
    total7,
    total30,
    rows: rows
      .sort((a, b) => b.cost30 - a.cost30)
      .map((row) => ({
        ...row,
        barWidth: `${Math.max(4, Math.round((row.cost30 / max) * 100))}%`,
      })),
  };
}

export function configuredApps(account: ServiceAccount): AppId[] {
  // These are existing configurations reported by the backend, not a frontend
  // registry of supported protocols or an inferred login/current status.
  return PROVIDER_STORE_APP_IDS.filter((appId) =>
    account.kind === "relay"
      ? Boolean(account.apps.get(appId)?.tiers.length)
      : account.apps
          .get(appId)
          ?.plans.some((plan) => plan.canEditConfig || plan.canSwitch),
  );
}
