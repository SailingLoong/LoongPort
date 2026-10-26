import { invoke } from "@tauri-apps/api/core";

export interface StartupRecoveryView {
  token: string;
  status: "pending" | "completed" | "verification_required";
  canRecover: boolean;
  restartRequired: boolean;
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
  // Only the established public DTO enters the query cache, never extra fields.
  return {
    token: value.token,
    status: value.status as StartupRecoveryView["status"],
    canRecover: value.canRecover,
    restartRequired: value.restartRequired,
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
    // Original wire name and original owner: this is the operation's SOURCE
    // key/recovery passphrase, not a new protection choice or an ordinary unlock.
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
