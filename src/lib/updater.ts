import { invoke } from "@tauri-apps/api/core";

export interface UpdateInfo {
  currentVersion: string;
  availableVersion: string;
  notes?: string;
  pubDate?: string;
}

export type AppUpdateCheckResult =
  | { status: "upToDate" }
  | { status: "available"; info: UpdateInfo };

export const checkForUpdate = (): Promise<AppUpdateCheckResult> =>
  invoke("check_app_update");
