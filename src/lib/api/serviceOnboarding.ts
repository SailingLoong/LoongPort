import { invoke } from "@tauri-apps/api/core";

export interface ServiceOnboardingStatus {
  shouldPrompt: boolean;
  completed: boolean;
  plazaVisible: boolean;
}

export const serviceOnboardingApi = {
  status: (): Promise<ServiceOnboardingStatus> =>
    invoke("service_onboarding_status"),
  dismiss: (): Promise<ServiceOnboardingStatus> =>
    invoke("service_onboarding_dismiss"),
  complete: (shareData: boolean): Promise<ServiceOnboardingStatus> =>
    invoke("service_onboarding_complete", { shareData }),
};
