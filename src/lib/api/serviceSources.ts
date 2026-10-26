import { invoke } from "@tauri-apps/api/core";
import type { AppId } from "./types";

/** Read-only public identity. Native credential generations and tokens never belong here. */
export type ServiceAccountReference =
  | { kind: "relay" | "vendor"; id: number }
  | {
      kind: "localProvider";
      platform: "openrouter";
      id: string;
      originApp: AppId;
      originProviderId: string;
    }
  | {
      kind: "nativeAccount";
      platform: "minimax_code";
      id: string;
      region: "cn" | "global";
    }
  | {
      kind: "nativeAccount";
      platform: "workbuddy" | "qoder_sdk" | "qoder_cn" | "zcode_start_plan";
      id: string;
    };

export interface ServiceSourceBinding {
  appId: AppId;
  providerId: string;
  model: string | null;
  usageState: "current" | "enabled" | "inactive";
  canSelect: boolean;
}

export interface ServiceSource {
  sourceId: string;
  account: ServiceAccountReference;
  name: string;
  serviceName: string | null;
  accountLabel: string | null;
  configurationName: string | null;
  bindings: ServiceSourceBinding[];
  /** Native eligibility facts, not additional stored provider rows. Required for localProvider selection. */
  supportedTargets?: { appId: AppId; model: string; canSelect: boolean }[];
}

export const serviceSourcesApi = {
  list: () => invoke<ServiceSource[]>("get_service_sources"),
  /** Record one inactive binding. Activation remains a separate reviewed operation. */
  bindVendor: (sourceId: string, app: "claude" | "codex") =>
    invoke<boolean>("bind_vendor_service_source", { sourceId, app }),
};
