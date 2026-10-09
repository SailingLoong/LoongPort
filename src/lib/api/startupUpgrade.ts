import { invoke } from "@tauri-apps/api/core";

export type UpgradeApp = "claude" | "codex" | "gemini" | "grokbuild";
export interface StartupUpgradeReview {
  status: string;
  checkpointPresent: boolean;
  checkpointId: string | null;
  reviewToken: string | null;
  canAuthenticate: boolean;
  canCheckAndBackup: boolean;
  canStartUpgrade: boolean;
}
export interface UpgradeAppReview {
  appType: UpgradeApp;
  revision: string;
  savedMode: "direct" | "proxy" | null;
  hasPendingOperation: boolean | null;
  pointerConsistent: boolean | null;
  liveStatus: string;
  storedFieldsMatch: boolean | null;
  canRecoverOperation: boolean;
  defaultAction: string;
  defaultTakeover: boolean;
  canCompleteApp: boolean;
  canStartUpgrade: boolean;
}

export const startupUpgradeApi = {
  query: () => invoke<StartupUpgradeReview>("get_startup_upgrade_review"),
  authenticate: (password: string | null) =>
    invoke<StartupUpgradeReview>("authenticate_startup_upgrade", { password }),
  prepare: (expectedReviewToken: string) =>
    invoke<StartupUpgradeReview>("prepare_startup_upgrade_checkpoint", {
      expectedReviewToken,
    }),
  cancel: (expectedReviewToken: string, expectedCheckpointId: string) =>
    invoke<StartupUpgradeReview>("cancel_startup_upgrade_checkpoint", {
      expectedReviewToken,
      expectedCheckpointId,
    }),
  publish: (expectedReviewToken: string, expectedCheckpointId: string) =>
    invoke<StartupUpgradeReview>("publish_startup_upgrade_checkpoint", {
      expectedReviewToken,
      expectedCheckpointId,
    }),
  queryApp: (expectedReviewToken: string, appType: UpgradeApp) =>
    invoke<UpgradeAppReview>("review_startup_upgrade_app", {
      expectedReviewToken,
      appType,
    }),
  recoverApp: (
    expectedReviewToken: string,
    appType: UpgradeApp,
    expectedAppRevision: string,
  ) =>
    invoke<UpgradeAppReview>("recover_startup_upgrade_app", {
      expectedReviewToken,
      appType,
      expectedAppRevision,
    }),
};
