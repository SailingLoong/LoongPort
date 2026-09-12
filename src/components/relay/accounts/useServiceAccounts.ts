import { useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { APP_IDS } from "@/config/appConfig";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { relayApi, type RelayRow } from "@/lib/api/relay";
import { vendorApi, type VendorAccountRow } from "@/lib/api/vendor";
import type { AppId } from "@/lib/api/types";
import { PROVIDER_SWITCHED, VENDOR_ACCOUNTS_CHANGED } from "@/lib/api/events";

export type ServiceAccount =
  | { kind: "relay"; id: number; row: RelayRow; apps: Map<AppId, RelayRow> }
  | {
      kind: "vendor";
      id: number;
      row: VendorAccountRow;
      apps: Map<AppId, VendorAccountRow>;
    };

export const serviceAccountsKey = ["serviceAccounts"] as const;

/** Keeps app-specific backend projections intact; the list itself is global. */
export function useServiceAccounts() {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: serviceAccountsKey,
    queryFn: () =>
      Promise.all(
        APP_IDS.map(async (appId) => {
          const [relays, vendors] = await Promise.all([
            relayApi.listRelays(appId),
            vendorApi.list(appId),
          ]);
          return { appId, relays, vendors };
        }),
      ),
  });
  const accounts = useMemo(() => {
    const relays = new Map<
      number,
      Extract<ServiceAccount, { kind: "relay" }>
    >();
    const vendors = new Map<
      number,
      Extract<ServiceAccount, { kind: "vendor" }>
    >();
    for (const snapshot of query.data ?? []) {
      for (const row of snapshot.relays) {
        const account = relays.get(row.id) ?? {
          kind: "relay",
          id: row.id,
          row,
          apps: new Map<AppId, RelayRow>(),
        };
        account.apps.set(snapshot.appId, row);
        relays.set(row.id, account);
      }
      for (const row of snapshot.vendors.accounts) {
        const account = vendors.get(row.id) ?? {
          kind: "vendor",
          id: row.id,
          row,
          apps: new Map<AppId, VendorAccountRow>(),
        };
        account.apps.set(snapshot.appId, row);
        vendors.set(row.id, account);
      }
    }
    return [...relays.values(), ...vendors.values()];
  }, [query.data]);
  const invalidate = () =>
    client.invalidateQueries({ queryKey: serviceAccountsKey });
  useTauriEvent(PROVIDER_SWITCHED, invalidate);
  useTauriEvent(VENDOR_ACCOUNTS_CHANGED, invalidate);
  return {
    accounts,
    isPending: query.isPending,
    error: query.error,
    reload: query.refetch,
  };
}

export function configuredApps(account: ServiceAccount): AppId[] {
  // These are existing configurations reported by the backend, not a frontend
  // registry of supported protocols or an inferred login/current status.
  return APP_IDS.filter((appId) =>
    account.kind === "relay"
      ? Boolean(account.apps.get(appId)?.tiers.length)
      : account.apps
          .get(appId)
          ?.plans.some((plan) => plan.canEditConfig || plan.canSwitch),
  );
}
