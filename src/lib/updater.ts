import { invoke } from "@tauri-apps/api/core";

export interface UpdateInfo {
  currentVersion: string;
  availableVersion: string;
  notes: string | null;
  pubDate: string | null;
}

export type AppUpdateCheckResult =
  { status: "upToDate" } | { status: "available"; info: UpdateInfo };

export const checkForUpdate = (): Promise<AppUpdateCheckResult> =>
  invoke("check_app_update");

/**
 * 当前被用户「跳过本版本」的版本号（null = 没有跳过过）。
 *
 * 事实 owner 在后端 settings（启动闸门读同一份），前端只读展示；
 * localStorage 时代的旧值只在挂载时一次性迁移。
 */
export const getDismissedUpdateVersion = (): Promise<string | null> =>
  invoke("get_dismissed_update_version");

/** 跳过 / 撤销跳过某个版本（null = 撤销）。 */
export const setDismissedUpdateVersion = (
  version: string | null,
): Promise<boolean> => invoke("set_dismissed_update_version", { version });
