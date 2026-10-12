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
  canCompleteUpgrade: boolean;
  archiveRetained: boolean;
  syncPaused: boolean;
}
export interface UpgradeAppReview {
  notUsed: boolean;
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
  directProviderResolution:
    "preserved" | "missing" | "conflict" | "verification_required";
  retainedProviderId: string | null;
  keepFilesProviders: { id: string; name: string }[];
  canChooseProvider: boolean;
  canChooseMode: boolean;
  modeRouteProviders: { id: string; name: string }[];
}

export interface UpgradeSourceAppFacts {
  appType: UpgradeApp;
  savedMode: "direct" | "proxy" | null;
  hasPendingOperation: boolean | null;
  storedFieldsMatch: boolean | null;
  providerResolution:
    "preserved" | "missing" | "conflict" | "verification_required";
  requiresModeChoice: boolean;
  requiresProviderChoice: boolean;
}
export interface UpgradeSourceReview {
  checkpointId: string;
  apps: UpgradeSourceAppFacts[];
}

export interface UpgradeSyncResumeReview {
  checkpointId: string;
  revision: string;
  paused: boolean;
}
export type UpgradeSyncResumeChoice = "independentConfig" | "compatibleDevices";

export const startupUpgradeApi = {
  complete: (expectedReviewToken: string, expectedCheckpointId: string) =>
    invoke<StartupUpgradeReview>("complete_startup_upgrade", {
      expectedReviewToken,
      expectedCheckpointId,
    }),
  querySyncResume: () =>
    invoke<UpgradeSyncResumeReview | null>("get_upgrade_sync_resume_review"),
  confirmSyncResume: (
    expectedCheckpointId: string,
    expectedRevision: string,
    choice: UpgradeSyncResumeChoice,
  ) =>
    invoke<UpgradeSyncResumeReview>("confirm_upgrade_sync_resume", {
      expectedCheckpointId,
      expectedRevision,
      choice,
    }),
  continueRuntime: (expectedReviewToken: string) =>
    invoke<void>("continue_startup_upgrade", { expectedReviewToken }),
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
  reviewOwnership: (expectedReviewToken: string) =>
    invoke<UpgradeSourceReview>("review_startup_upgrade_ownership", {
      expectedReviewToken,
    }),
  queryApp: (expectedReviewToken: string, appType: UpgradeApp) =>
    invoke<UpgradeAppReview>("review_startup_upgrade_app", {
      expectedReviewToken,
      appType,
    }),
  selectProvider: (
    expectedReviewToken: string,
    appType: UpgradeApp,
    expectedAppRevision: string,
    providerId: string,
  ) =>
    invoke<UpgradeAppReview>("select_startup_upgrade_provider", {
      expectedReviewToken,
      appType,
      expectedAppRevision,
      providerId,
    }),
  selectMode: (
    expectedReviewToken: string,
    appType: UpgradeApp,
    expectedAppRevision: string,
    choice: { mode: "direct" | "proxy"; proxyRoute: string | null },
  ) =>
    invoke<UpgradeAppReview>("select_startup_upgrade_mode", {
      expectedReviewToken,
      appType,
      expectedAppRevision,
      choice,
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
