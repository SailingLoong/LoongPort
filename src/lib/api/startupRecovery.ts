import { invoke } from "@tauri-apps/api/core";

export interface StartupRecoveryView {
  token: string;
  status: "pending" | "completed" | "verification_required";
  canRecover: boolean;
  restartRequired: boolean;
  inputRequired: boolean;
  inputKind: "none" | "source" | "current";
}
export const startupRecoveryKey = ["startupRecovery"] as const;

function readView(value: unknown): StartupRecoveryView {
  if (
    typeof value !== "object" ||
    value === null ||
    !("token" in value) ||
    typeof value.token !== "string" ||
    !value.token ||
    !("status" in value) ||
    typeof value.status !== "string" ||
    !["pending", "completed", "verification_required"].includes(value.status) ||
    !("canRecover" in value) ||
    typeof value.canRecover !== "boolean" ||
    !("restartRequired" in value) ||
    typeof value.restartRequired !== "boolean"
  )
    throw new Error("startupRecovery.invalidStatus");
  const hasRequired = Object.hasOwn(value, "inputRequired");
  const hasKind = Object.hasOwn(value, "inputKind");
  // Only a completely legacy response retains the original source-input contract.
  // Missing half of a new capability must never become a no-input permission.
  let inputRequired = true;
  let inputKind: StartupRecoveryView["inputKind"] = "source";
  if (hasRequired || hasKind) {
    if (
      !hasRequired ||
      !hasKind ||
      !("inputRequired" in value) ||
      typeof value.inputRequired !== "boolean" ||
      !("inputKind" in value) ||
      typeof value.inputKind !== "string" ||
      !["none", "source", "current"].includes(value.inputKind) ||
      value.inputRequired !== (value.inputKind !== "none") ||
      ((!value.canRecover || value.status === "completed") &&
        value.inputKind !== "none")
    )
      throw new Error("startupRecovery.invalidStatus");
    inputRequired = value.inputRequired;
    inputKind = value.inputKind as StartupRecoveryView["inputKind"];
  }
  // Only the established public DTO enters the query cache, never extra fields.
  return {
    token: value.token,
    status: value.status as StartupRecoveryView["status"],
    canRecover: value.canRecover,
    restartRequired: value.restartRequired,
    inputRequired,
    inputKind,
  };
}
export const startupRecoveryApi = {
  status: async (): Promise<StartupRecoveryView | null> => {
    try {
      return readView(await invoke<unknown>("get_startup_recovery"));
    } catch (error) {
      if (error === "secret.no_pending_operation") return null;
      // Query errors are retained. Never retain native error text or objects.
      throw new Error("startupRecovery.statusFailed");
    }
  },
  recover: async (token: string, sourceText: string) => {
    // Original wire and original owner. The inspected native journal decides
    // source/current/no input; an empty string never selects a protection mode.
    const view = readView(
      await invoke<unknown>("recover_startup_operation", {
        token,
        password: sourceText,
      }),
    );
    if (view.token !== token) throw new Error("startupRecovery.changedResult");
    return view;
  },
  restart: () => invoke<void>("restart_app"),
};
