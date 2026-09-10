import { invoke } from "@tauri-apps/api/core";
import type { ProviderPresentation } from "@/types";
import type { AppId } from "./types";

export type ConfigurationSelection =
  | { kind: "relay" }
  | { kind: "vendor"; rowId: number; planId: string }
  | { kind: "provider" };

export interface ApplicationConfiguration {
  providerId: string;
  name: string;
  source: "official" | "relay" | "custom";
  account: { kind: "relay" | "vendor"; id: number } | null;
  serviceName: string | null;
  accountLabel: string | null;
  configurationName: string | null;
  model: string | null;
  presentation: ProviderPresentation;
  selection: ConfigurationSelection;
  canSelect: boolean;
}

export interface ApplicationOverview {
  configurations: ApplicationConfiguration[];
  recentProviderIds: string[];
  isAdditive: boolean;
}

export const applicationOverviewApi = {
  get: (app: AppId) =>
    invoke<ApplicationOverview>("get_application_overview", { app }),
};
