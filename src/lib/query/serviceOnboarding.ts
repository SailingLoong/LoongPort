import { useQuery } from "@tanstack/react-query";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";

export const serviceOnboardingKey = ["service-onboarding"] as const;
export function useServiceOnboardingStatus() {
  return useQuery({
    queryKey: serviceOnboardingKey,
    queryFn: serviceOnboardingApi.status,
  });
}
