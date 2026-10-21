import { invoke } from "@tauri-apps/api/core";

/** Safe metadata only; discovered does not mean inference-verified. */
export interface TraeDiscoveredModel {
  id: string;
  name: string;
}
export interface TraeModelDiscoveryRequest {
  requestId: string;
}
export const traeModelsApi = {
  begin: (accountId: string) =>
    invoke<TraeModelDiscoveryRequest>("begin_trae_model_discovery", {
      accountId,
    }),
  list: (requestId: string) =>
    invoke<TraeDiscoveredModel[]>("list_trae_discovered_models", { requestId }),
  cancel: (requestId: string) =>
    invoke<void>("cancel_trae_model_discovery", { requestId }),
};
