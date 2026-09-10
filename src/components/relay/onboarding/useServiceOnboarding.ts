import { useQuery } from "@tanstack/react-query";
import { APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { relayApi } from "@/lib/api/relay";
import { vendorApi } from "@/lib/api/vendor";
export {
  serviceOnboardingKey,
  useServiceOnboardingStatus,
} from "@/lib/query/serviceOnboarding";

export interface ConnectedService {
  kind: "relay" | "vendor";
  rowId: number;
  name: string;
}
export interface ServiceConfigurationChoice {
  app: AppId;
  id: string;
  name: string;
}
export function useServiceConfigurationChoices(account: ConnectedService) {
  return useQuery({
    queryKey: ["service-configuration", account.kind, account.rowId],
    queryFn: async () =>
      (
        await Promise.all(
          APP_IDS.map(async (app): Promise<ServiceConfigurationChoice[]> => {
            if (account.kind === "vendor") {
              const listing = await vendorApi.list(app);
              return (
                listing.accounts
                  .find((row) => row.id === account.rowId)
                  ?.plans.filter((plan) => plan.canSwitch)
                  .map((plan) => ({
                    app,
                    id: plan.planId,
                    name: plan.planName,
                  })) ?? []
              );
            }
            const rows = await relayApi.listRelays(app);
            return (
              rows
                .find((row) => row.id === account.rowId)
                ?.tiers.map((tier) => ({
                  app,
                  id: tier.providerId,
                  name: tier.displayName,
                })) ?? []
            );
          }),
        )
      ).flat(),
  });
}
