import { invoke } from "@tauri-apps/api/core";

export interface ContextSelection {
  installPath: string;
  dataRoot: string;
  keyMode: "standard" | "custom" | "unknown";
}
export interface SourceContext {
  contextId: string;
  contextRevision: string;
  dataRoot: string;
  family: "zai" | "bigmodel";
  version: string;
  build: string;
}
export interface CatalogStatus {
  revision: string;
  profiles: { id: string; family: "zai" | "bigmodel"; label: string | null }[];
  current: string | null;
  pending: boolean;
  nativeUnconfirmed: boolean;
}
export interface RecoveryStatus {
  revision: string;
  pending: boolean;
  nativeUnconfirmed: boolean;
  records: {
    id: string;
    disposition:
      "native-unconfirmed" | "full-before" | "full-after" | "explicit-capture";
    latestCompleted: boolean;
  }[];
}
export interface AccountError {
  code: string;
  remedy: string;
  committed: boolean;
}

/** Static IPC error vocabulary: never expose backend messages, paths or tokens. */
export const accountErrorText = {
  select_context: "Choose and inspect a ZCode installation and data directory.",
  unsupported_platform:
    "This platform does not support the requested account operation. Manage your account in official ZCode and keep existing LoongPort recovery data.",
  unsupported_build:
    "This ZCode build is not verified. Use a verified build before inspecting again.",
  native_gate_pending:
    "Native compatibility verification is still pending. Account capture and switching remain blocked.",
  root_unverified:
    "The selected data directory could not be verified. Review its location in ZCode settings.",
  key_context_unknown:
    "Select standard local verification and inspect again. If the system user cannot be determined, verification remains blocked. Custom secrets are unsupported.",
  custom_key_context:
    "Custom key contexts are not supported. Use ZCode's standard local key context.",
  settings_invalid:
    "ZCode settings could not be verified. Review them in ZCode, quit normally, then inspect again.",
  legacy_selection:
    "This ZCode account selection format is unsupported. Review the personal account selection in ZCode.",
  selection_missing:
    "Select a personal account in ZCode, quit normally, then inspect again.",
  team_unsupported:
    "Team accounts are not supported. Select a personal account in ZCode, quit normally, then inspect again.",
  app_running:
    "Quit ZCode normally and stop its other writers before inspecting again.",
  writer_state_unknown:
    "ZCode's process state could not be verified. Check that ZCode and its other writers have stopped.",
  context_changed:
    "The selected source changed. Inspect it again and review the latest account status.",
  target_scope_mismatch:
    "Choose a saved personal account in the inspected source's account family.",
  vault_unavailable:
    "Unlock the local LoongPort vault, then refresh account status.",
  operation_failed:
    "The account operation could not be verified. Inspect the selected source again and refresh status.",
  committed_recovery_required:
    "The account switch committed, but recovery requires attention. Review local recovery before continuing.",
  pending_or_locked:
    "Quit ZCode normally, then review and preserve any pending local recovery.",
  native_unconfirmed:
    "Confirm the selected recovery record, or sign in officially and explicitly recapture it.",
  confirmation_mismatch:
    "The native session does not match this recovery record. Sign in officially, quit normally, then explicitly recapture this record.",
  recovery_full:
    "Recovery storage is full. Confirm or recapture an older record, then permanently clean up that selected record.",
  resource_limit:
    "The local account store reached its limit. Review local recovery and saved data before continuing.",
  recovery_unconfirmed:
    "This recovery record is not confirmed and cannot be deleted. Confirm or explicitly recapture it first.",
  recovery_changed:
    "Recovery changed elsewhere. Refresh account status and review the record again.",
  catalog_changed:
    "Saved accounts changed elsewhere. Refresh account status and review your choice again.",
  source_changed:
    "The native source changed. Inspect it again before another account action.",
  missing_target:
    "This saved account is no longer available. Refresh account status and choose again.",
  missing_saved_source:
    "Save the current account explicitly before switching to another saved account.",
  recovery_context_mismatch:
    "This recovery record belongs to a different source. Review the selected data directory.",
  unsupported_scope:
    "This account scope is unsupported. Select a personal account in ZCode.",
  unsafe_path:
    "The selected path is unsafe or changed. Review the data directory before inspecting again.",
  storage_failed:
    "The local account store could not be read or written safely. Check local storage and refresh status.",
  native_session_invalid:
    "The native session could not be verified with the standard local key. Sign in through ZCode, quit normally, then inspect again.",
  saved_data_invalid:
    "Saved account data could not be verified. Review the local vault and recovery records before continuing.",
  not_admitted:
    "Account access is not verified. Inspect the selected source again before continuing.",
  recovery_cleanup_required:
    "Recovery has been preserved but cleanup needs attention. Refresh local recovery and review the pending record.",
} as const;
const remedies = new Set([
  "chooseContext",
  "useVerifiedBuild",
  "finishPlatformCheck",
  "reviewDataLocation",
  "useStandardContext",
  "openNativeSettings",
  "quitNativeWriters",
  "verifyWriterState",
  "refreshContext",
  "chooseSavedAccount",
  "unlockVault",
  "reviewRecovery",
  "quitWritersAndReviewRecovery",
  "confirmOrRecapture",
  "captureCurrent",
  "checkLocalStorage",
  "reviewSavedData",
]);
export function safeAccountError(cause: unknown): AccountError {
  const value =
    typeof cause === "object" && cause !== null
      ? (cause as Partial<AccountError>)
      : {};
  const key =
    typeof value.code === "string" && value.code.startsWith("zcode.account.")
      ? value.code.slice("zcode.account.".length)
      : "";
  return {
    code: Object.hasOwn(accountErrorText, key)
      ? `zcode.account.${key}`
      : "zcode.account.operation_failed",
    remedy:
      typeof value.remedy === "string" && remedies.has(value.remedy)
        ? value.remedy
        : "refreshContext",
    committed: value.committed === true,
  };
}
async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (cause) {
    throw safeAccountError(cause);
  }
}
type CaptureOutcome = "saved" | "refreshed";
export const zcodeAccountsApi = {
  inspect: async (source: ContextSelection): Promise<SourceContext> => {
    const result = await call<SourceContext>("inspect_zcode_account_context", {
      source,
    });
    const { contextId, contextRevision, dataRoot, family, version, build } =
      result;
    return { contextId, contextRevision, dataRoot, family, version, build };
  },
  status: async (
    source: ContextSelection,
    contextRevision: string,
  ): Promise<CatalogStatus> => {
    const result = await call<CatalogStatus>("get_zcode_account_status", {
      source,
      contextRevision,
    });
    return {
      revision: result.revision,
      profiles: result.profiles.map(({ id, family, label }) => ({
        id,
        family,
        label,
      })),
      current: result.current,
      pending: result.pending,
      nativeUnconfirmed: result.nativeUnconfirmed,
    };
  },
  capture: (
    source: ContextSelection,
    contextRevision: string,
    catalogRevision: string,
  ) =>
    call<CaptureOutcome>("capture_zcode_current_account", {
      source,
      contextRevision,
      catalogRevision,
    }),
  switch: (
    source: ContextSelection,
    contextRevision: string,
    id: string,
    catalogRevision: string,
  ) =>
    call<"switched" | "refreshed">("switch_zcode_saved_account", {
      source,
      contextRevision,
      id,
      catalogRevision,
    }),
  recoveryStatus: async (): Promise<RecoveryStatus> => {
    const result = await call<RecoveryStatus>("get_zcode_account_recovery");
    return {
      revision: result.revision,
      pending: result.pending,
      nativeUnconfirmed: result.nativeUnconfirmed,
      records: result.records.map(({ id, disposition, latestCompleted }) => ({
        id,
        disposition,
        latestCompleted,
      })),
    };
  },
  archive: (revision: string) =>
    call<"archived" | "nothingPending">("archive_zcode_account_recovery", {
      revision,
    }),
  confirmRecovery: (
    source: ContextSelection,
    contextRevision: string,
    id: string,
    recoveryRevision: string,
  ) =>
    call<void>("confirm_zcode_account_recovery", {
      source,
      contextRevision,
      id,
      recoveryRevision,
    }),
  recapture: (
    source: ContextSelection,
    contextRevision: string,
    id: string,
    recoveryRevision: string,
    catalogRevision: string,
  ) =>
    call<CaptureOutcome>("recapture_zcode_account_recovery", {
      source,
      contextRevision,
      id,
      recoveryRevision,
      catalogRevision,
    }),
  deleteRecovery: (id: string, revision: string) =>
    call<void>("delete_zcode_account_recovery", { id, revision }),
};
