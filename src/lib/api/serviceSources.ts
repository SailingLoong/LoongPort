import { invoke } from "@tauri-apps/api/core";
import type { AppId } from "./types";

export interface ServiceSourceBinding {
  appId: AppId;
  providerId: string;
  model: string | null;
  usageState: "current" | "enabled" | "inactive";
  canSelect: boolean;
}

export interface ServiceSource {
  sourceId: string;
  account: { kind: "relay" | "vendor"; id: number };
  name: string;
  serviceName: string | null;
  accountLabel: string | null;
  configurationName: string | null;
  bindings: ServiceSourceBinding[];
}

export const serviceSourcesApi = {
  list: () => invoke<ServiceSource[]>("get_service_sources"),
  /** Record one inactive binding. Activation remains a separate reviewed operation. */
  bindVendor: (sourceId: string, app: "claude" | "codex") =>
    invoke<boolean>("bind_vendor_service_source", { sourceId, app }),
};
