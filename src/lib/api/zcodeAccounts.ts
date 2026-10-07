import { invoke } from "@tauri-apps/api/core";

export interface ContextSelection {
  installPath: string;
  dataRoot: string;
  keyMode: "standard" | "custom" | "unknown";
}
export interface LatestVersion {
  version: string | null;
  checkedAt: number;
  error: "network" | "invalidManifest" | "unsupportedPlatform" | null;
}
export interface MetadataDiscovery {
  dataRoot: string;
  sourceBasis: "osAccountHome" | "bootstrapDataBaseDir";
  candidates: {
    installPath: string;
    version: string | null;
    build: string | null;
    verifiedBuild: boolean;
  }[];
  latestStatus: "notQueried";
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
  profiles: {
    id: string;
    family: "zai" | "bigmodel";
    label: string | null;
    sourceVerified: boolean;
  }[];
  current: string | null;
  pending: boolean;
  nativeUnconfirmed: boolean;
}
export interface CapturePreview {
  id: string;
  label: string | null;
  family: "zai" | "bigmodel";
  duplicate: boolean;
  previewId: string;
}
export interface BundlePreview {
  previewId: string;
  rows: {
    index: number;
    id: string | null;
    label: string | null;
    family: "zai" | "bigmodel" | null;
    duplicate: boolean;
    ambiguous: boolean;
    error: "invalidEntry" | "incompatibleCredentials" | null;
  }[];
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
export interface SwitchOperationStatus {
  requestId: string;
  phase:
    | "accepted"
    | "exited"
    | "transactionUncertain"
    | "committed"
    | "restartVerified"
    | "restartFailed"
    | "failed";
  target: string;
  refreshed: boolean;
  restartRequested: boolean;
}
type SwitchRequest = {
  requestId: string;
  sourceKey: string;
  contextRevision: string;
  target: string;
  catalogRevision: string;
};
const requestKey = "loongport:zcode-switch-request-v1";
const sourceKey = (source: ContextSelection) =>
  JSON.stringify([source.installPath, source.dataRoot, source.keyMode]);
function requests(): SwitchRequest[] {
  try {
    const value = JSON.parse(localStorage.getItem(requestKey) ?? "null");
    const items = Array.isArray(value) ? value : value ? [value] : [];
    if (
      items.length > 16 ||
      !items.every(
        (item) =>
          item &&
          [
            "requestId",
            "sourceKey",
            "contextRevision",
            "target",
            "catalogRevision",
          ].every((key) => typeof item[key] === "string"),
      )
    )
      throw new Error("invalid local request pointers");
    return items;
  } catch {
    throw {
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    };
  }
}
function lastRequest(source: ContextSelection): SwitchRequest | null {
  return (
    requests().find((request) => request.sourceKey === sourceKey(source)) ??
    null
  );
}
function rememberRequest(request: SwitchRequest) {
  const items = requests().filter(
    (item) => item.sourceKey !== request.sourceKey,
  );
  if (items.length >= 16)
    throw {
      code: "zcode.account.resource_limit",
      remedy: "reviewSavedData",
      committed: false,
    };
  try {
    localStorage.setItem(requestKey, JSON.stringify([...items, request]));
  } catch {
    throw {
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    };
  }
}
function forgetRequest(id: string) {
  try {
    localStorage.setItem(
      requestKey,
      JSON.stringify(requests().filter((request) => request.requestId !== id)),
    );
  } catch {
    /* Original result remains queryable in the encrypted vault. */
  }
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
  official_unavailable:
    "The official service could not complete this step. Check the current login status before continuing.",
  key_result_unknown:
    "The official Key may already exist. Query the original project result before creating another.",
  key_cleanup_pending:
    "The local Key operation record needs attention. Query its original result before creating a Key.",
  request_not_sent:
    "The write was not sent. Review the current account and project before confirming again.",
  login_changed:
    "This login operation changed or expired. Query its original result before starting again.",
  save_result_unknown:
    "Saving may already have completed. Query the original result; closing this dialog does not undo a submitted save.",
  operation_failed:
    "The account operation could not be verified. Inspect the selected source again and refresh status.",
  committed_recovery_required:
    "The account switch committed, but recovery requires attention. Review local recovery before continuing.",
  committed_restart_failed:
    "The local account switch committed, but reopening the exact ZCode source could not be verified. Open that source normally and review its account status. Online sign-in remains unverified.",
  operation_already_known:
    "This switch request already exists. Query the original operation and refresh before another confirmation.",
  committed_result_unknown:
    "The local switch committed, but its final result is incomplete. Query the original operation; do not repeat the switch automatically.",
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
  source_unverified:
    "This imported account has unverified source scope. Sign in with the same personal account in official ZCode, then explicitly save and update the current account in LoongPort.",
  bundle_authentication:
    "The account bundle could not be authenticated. Check its password and obtain an intact export.",
  bundle_invalid:
    "This account bundle is unsupported or malformed. Choose an intact .zsb export in the supported format (up to 10 MiB and 50 accounts).",
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
  "queryOriginal",
  "retryKeyConsent",
  "retrySave",
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
  operationStatus: (
    source: ContextSelection,
    contextRevision: string,
    requestId: string,
  ): Promise<SwitchOperationStatus | null> =>
    call("get_zcode_switch_operation", { source, contextRevision, requestId }),
  queryLastOperation: async (
    source: ContextSelection,
    contextRevision: string,
  ): Promise<SwitchOperationStatus | null> => {
    const request = lastRequest(source);
    if (!request || request.sourceKey !== sourceKey(source)) return null;
    return zcodeAccountsApi.operationStatus(
      source,
      contextRevision,
      request.requestId,
    );
  },
  openForLogin: (source: ContextSelection): Promise<void> =>
    call("open_zcode_for_account_login", { source }),
  previewBundle: async (
    source: ContextSelection,
    contextRevision: string,
    catalogRevision: string,
    file: number[],
    password: string,
  ): Promise<BundlePreview> => {
    const result = await call<BundlePreview>("preview_zcode_account_bundle", {
      source,
      contextRevision,
      catalogRevision,
      file,
      password,
    });
    return {
      previewId: result.previewId,
      rows: result.rows.map(
        ({ index, id, label, family, duplicate, ambiguous, error }) => ({
          index,
          id,
          label,
          family,
          duplicate,
          ambiguous,
          error,
        }),
      ),
    };
  },
  importBundle: (
    source: ContextSelection,
    contextRevision: string,
    catalogRevision: string,
    previewId: string,
    selected: { index: number; updateDuplicate: boolean }[],
  ): Promise<("saved" | "refreshed" | "kept")[]> =>
    call("import_zcode_account_bundle", {
      source,
      contextRevision,
      catalogRevision,
      previewId,
      selected,
    }),
  cancelBundle: (previewId: string): Promise<void> =>
    call("cancel_zcode_bundle_preview", { previewId }),
  latestVersion: async (): Promise<LatestVersion> => {
    const { version, checkedAt, error } = await call<LatestVersion>(
      "query_zcode_latest_version",
    );
    return { version, checkedAt, error };
  },
  discover: async (installPath?: string): Promise<MetadataDiscovery> => {
    const result = await call<MetadataDiscovery>("discover_zcode_metadata", {
      installPath: installPath ?? null,
    });
    return {
      dataRoot: result.dataRoot,
      sourceBasis: result.sourceBasis,
      latestStatus: result.latestStatus,
      candidates: result.candidates.map(
        ({ installPath, version, build, verifiedBuild }) => ({
          installPath,
          version,
          build,
          verifiedBuild,
        }),
      ),
    };
  },
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
      profiles: result.profiles.map(
        ({ id, family, label, sourceVerified }) => ({
          id,
          family,
          label,
          sourceVerified: sourceVerified === true,
        }),
      ),
      current: result.current,
      pending: result.pending,
      nativeUnconfirmed: result.nativeUnconfirmed,
    };
  },
  previewCapture: async (
    source: ContextSelection,
    contextRevision: string,
    catalogRevision: string,
  ): Promise<CapturePreview> => {
    const result = await call<CapturePreview>("preview_zcode_current_account", {
      source,
      contextRevision,
      catalogRevision,
    });
    const { id, label, family, duplicate, previewId } = result;
    return { id, label, family, duplicate, previewId };
  },
  commitCapture: (
    source: ContextSelection,
    contextRevision: string,
    catalogRevision: string,
    previewId: string,
    updateDuplicate: boolean,
  ) =>
    call<"saved" | "refreshed" | "kept">("save_zcode_account_preview", {
      source,
      contextRevision,
      catalogRevision,
      previewId,
      updateDuplicate,
    }),
  cancelCapture: (previewId: string) =>
    call<void>("cancel_zcode_account_preview", { previewId }),
  switch: async (
    source: ContextSelection,
    contextRevision: string,
    id: string,
    catalogRevision: string,
  ): Promise<"switched" | "refreshed"> => {
    let previous = lastRequest(source);
    if (previous?.sourceKey === sourceKey(source)) {
      // Resolve the original physical operation before allocating another identity.
      // A lost reply must never become a second quit/write/restart sequence.
      const original = await zcodeAccountsApi.operationStatus(
        source,
        contextRevision,
        previous.requestId,
      );
      if (
        original &&
        !["failed", "restartVerified"].includes(original.phase) &&
        !(original.phase === "committed" && !original.restartRequested)
      ) {
        throw {
          code:
            original.phase === "restartFailed"
              ? "zcode.account.committed_restart_failed"
              : original.phase === "committed"
                ? "zcode.account.committed_result_unknown"
                : "zcode.account.operation_already_known",
          remedy: "queryOriginal",
          committed: ["committed", "restartFailed"].includes(original.phase),
        };
      }
      if (
        original &&
        original.phase !== "failed" &&
        previous.contextRevision === contextRevision &&
        previous.target === id &&
        previous.catalogRevision === catalogRevision
      )
        return original.refreshed ? "refreshed" : "switched";
      if (original?.phase === "failed") previous = null;
      if (
        previous &&
        !original &&
        (previous.target !== id ||
          previous.catalogRevision !== catalogRevision ||
          previous.contextRevision !== contextRevision)
      )
        throw {
          code: "zcode.account.operation_already_known",
          remedy: "queryOriginal",
          committed: false,
        };
    }
    const request =
      lastRequest(source) &&
      previous &&
      previous.sourceKey === sourceKey(source) &&
      previous.contextRevision === contextRevision &&
      previous.target === id &&
      previous.catalogRevision === catalogRevision
        ? previous
        : {
            requestId: crypto.randomUUID(),
            sourceKey: sourceKey(source),
            contextRevision,
            target: id,
            catalogRevision,
          };
    rememberRequest(request);
    try {
      const result = await call<"switched" | "refreshed">(
        "switch_zcode_saved_account",
        {
          source,
          contextRevision,
          id,
          catalogRevision,
          requestId: request.requestId,
        },
      );
      return result;
    } catch (cause) {
      let status: SwitchOperationStatus | null = null;
      try {
        status = await zcodeAccountsApi.operationStatus(
          source,
          contextRevision,
          request.requestId,
        );
      } catch {
        /* Keep the original identity and error; never replay here. */
      }
      if (
        status?.target === id &&
        (status.phase === "restartVerified" ||
          (status.phase === "committed" && !status.restartRequested))
      )
        return status.refreshed ? "refreshed" : "switched";
      if (status?.phase === "restartFailed")
        throw {
          code: "zcode.account.committed_restart_failed",
          remedy: "openNativeSettings",
          committed: true,
        };
      if (status?.phase === "committed")
        throw {
          code: "zcode.account.committed_result_unknown",
          remedy: "queryOriginal",
          committed: true,
        };
      if (status?.phase === "failed") forgetRequest(request.requestId);
      throw safeAccountError(cause);
    }
  },
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
