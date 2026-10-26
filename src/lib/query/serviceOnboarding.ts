import { useQuery } from "@tanstack/react-query";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";

export const serviceOnboardingKey = ["service-onboarding"] as const;
export function useServiceOnboardingStatus(enabled = true) {
  return useQuery({
    queryKey: serviceOnboardingKey,
    enabled,
    queryFn: serviceOnboardingApi.status,
  });
}
