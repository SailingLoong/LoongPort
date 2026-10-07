import { invoke } from "@tauri-apps/api/core";
import { safeAccountError, type AccountError } from "./zcodeAccounts";

export type LoginFamily = "bigmodel" | "zai";
export interface LoginProgress {
  flowId: string;
  phase:
    | "waiting"
    | "preparing"
    | "keyRequired"
    | "review"
    | "saved"
    | "cancelled"
    | "expired"
    | "failed";
  family: LoginFamily;
  // expiresAt is Unix seconds, as returned by the official init response.
  authorization: {
    url: string;
    expiresAt: number;
    pollIntervalSec: number;
  } | null;
  account: {
    id: string;
    label: string | null;
    duplicate: boolean;
    identitySource: "officialLogin";
  } | null;
  connections: {
    start: "ready" | "unknown" | "unavailable";
    coding: "ready" | "unknown" | "unavailable";
    needsKey: boolean;
  } | null;
  project: {
    organizationId: string;
    organizationName: string | null;
    projectId: string;
    projectName: string | null;
  } | null;
  keyCreated: boolean;
  keyMayExist: boolean;
  keyManagementUrl: string;
  error: AccountError | null;
  saved: { id: string; outcome: "saved" | "refreshed" | "kept" } | null;
}

// Explicit projection keeps private backend implementation fields out of UI state.
function publicProgress(value: LoginProgress): LoginProgress {
  const { authorization, account, connections, project, saved } = value;
  const error = value.error ? safeAccountError(value.error) : null;
  if (
    error &&
    error.remedy !== "retryKeyConsent" &&
    error.remedy !== "retrySave"
  )
    error.remedy = "queryOriginal";
  return {
    flowId: value.flowId,
    phase: value.phase,
    family: value.family,
    authorization: authorization && {
      url: authorization.url,
      expiresAt: authorization.expiresAt,
      pollIntervalSec: authorization.pollIntervalSec,
    },
    account: account && {
      id: account.id,
      label: account.label,
      duplicate: account.duplicate,
      identitySource: account.identitySource,
    },
    connections: connections && {
      start: connections.start,
      coding: connections.coding,
      needsKey: connections.needsKey,
    },
    project: project && {
      organizationId: project.organizationId,
      organizationName: project.organizationName,
      projectId: project.projectId,
      projectName: project.projectName,
    },
    keyCreated: value.keyCreated,
    keyMayExist: value.keyMayExist,
    keyManagementUrl: value.keyManagementUrl,
    error,
    saved: saved && { id: saved.id, outcome: saved.outcome },
  };
}

async function call(
  command: string,
  args: Record<string, unknown>,
): Promise<LoginProgress> {
  try {
    return publicProgress(await invoke<LoginProgress>(command, args));
  } catch (cause) {
    throw { ...safeAccountError(cause), remedy: "queryOriginal" };
  }
}

export const zcodeLoginApi = {
  begin: (family: LoginFamily, dataRoot?: string) =>
    call("begin_zcode_official_login", {
      family,
      ...(dataRoot === undefined ? {} : { dataRoot }),
    }),
  progress: (flowId: string) => call("get_zcode_login_progress", { flowId }),
  confirmKey: (flowId: string, organizationId: string, projectId: string) =>
    call("confirm_zcode_login_key", { flowId, organizationId, projectId }),
  declineKey: (flowId: string) => call("decline_zcode_login_key", { flowId }),
  save: (flowId: string, updateDuplicate: boolean) =>
    call("save_zcode_login_account", { flowId, updateDuplicate }),
  cancel: (flowId: string) => call("cancel_zcode_official_login", { flowId }),
};
