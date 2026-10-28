import { useQuery } from "@tanstack/react-query";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
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
  providerId: string;
  name: string;
}
/** Vendor setup retains its existing plan owner. Relay uses useRelaySetup's
 * native discovery and durable operation, never a local-tier fallback. */
export function useVendorConfigurationChoices(
  account: Pick<ConnectedService, "rowId" | "name">,
) {
  return useQuery({
    queryKey: ["service-configuration", "vendor", account.rowId],
    queryFn: async () =>
      (
        await Promise.all(
          PROVIDER_STORE_APP_IDS.map(
            async (app): Promise<ServiceConfigurationChoice[]> => {
              const listing = await vendorApi.list(app);
              return (
                listing.accounts
                  .find((row) => row.id === account.rowId)
                  ?.plans.filter((plan) => plan.canSwitch)
                  .map((plan) => ({
                    app,
                    id: plan.planId,
                    providerId: plan.providerId,
                    name: plan.planName,
                  })) ?? []
              );
            },
          ),
        )
      ).flat(),
  });
}
