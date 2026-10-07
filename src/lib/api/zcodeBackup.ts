import { invoke } from "@tauri-apps/api/core";
import { safeAccountError, type AccountError } from "./zcodeAccounts";

export interface BundleExportRequest {
  requestId: string;
  dataRoot?: string;
  catalogRevision: string;
  profileIds: string[];
  destination: string;
  password: string;
  passwordConfirmation: string;
}
export interface BundleExportResult {
  requestId: string;
  status: "working" | "saved" | "failed" | "unknown";
  destination: string | null;
  count: number | null;
  error: AccountError | null;
}

// Only authenticated completion evidence may expose a destination or account count.
function publicResult(value: unknown, requestId: string): BundleExportResult {
  const result = value as Partial<BundleExportResult> | null;
  if (
    !result ||
    result.requestId !== requestId ||
    !["working", "saved", "failed", "unknown"].includes(result.status ?? "") ||
    (result.status === "saved" &&
      (typeof result.destination !== "string" ||
        !result.destination.trim() ||
        !Number.isInteger(result.count) ||
        (result.count ?? 0) < 1 ||
        (result.count ?? 0) > 50 ||
        result.error != null))
  ) {
    throw {
      code: "zcode.account.saved_data_invalid",
      remedy: "queryOriginal",
      committed: false,
    };
  }
  return {
    requestId,
    status: result.status as BundleExportResult["status"],
    destination: result.status === "saved" ? result.destination! : null,
    count: result.status === "saved" ? result.count! : null,
    error: result.error
      ? { ...safeAccountError(result.error), remedy: "queryOriginal" }
      : null,
  };
}

async function call(
  command: string,
  args: Record<string, unknown>,
  requestId: string,
) {
  try {
    return publicResult(await invoke<unknown>(command, args), requestId);
  } catch (cause) {
    // A rejected IPC reply is never permission to replay the export write.
    throw { ...safeAccountError(cause), remedy: "queryOriginal" };
  }
}

export const zcodeBackupApi = {
  exportBundle: (request: BundleExportRequest): Promise<BundleExportResult> =>
    call(
      "export_zcode_account_bundle",
      {
        input: {
          requestId: request.requestId,
          ...(request.dataRoot === undefined
            ? {}
            : { dataRoot: request.dataRoot }),
          catalogRevision: request.catalogRevision,
          profileIds: [...request.profileIds],
          destination: request.destination,
          password: request.password,
          passwordConfirmation: request.passwordConfirmation,
        },
      },
      request.requestId,
    ),
  result: (requestId: string): Promise<BundleExportResult> =>
    call("get_zcode_bundle_export_result", { requestId }, requestId),
};
