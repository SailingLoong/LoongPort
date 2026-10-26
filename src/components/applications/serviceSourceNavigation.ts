import type { AppId } from "@/lib/api";
import type { ServiceAccountReference } from "@/lib/api/serviceSources";
import type { ClientRoute } from "@/components/shell/navigation";

/** Reuse the account page's existing public navigation identities, never numeric-cast native IDs. */
export function serviceSourceDestination(
  account: ServiceAccountReference,
  app: AppId,
): ClientRoute {
  if (account.kind === "relay" || account.kind === "vendor")
    return { view: "services", app, account };
  if (account.kind === "localProvider")
    return { view: "services", app: account.originApp, platform: "openrouter" };
  if (account.kind !== "nativeAccount")
    return { view: "services", app, account };
  const platform =
    account.platform === "minimax_code"
      ? account.region === "global"
        ? "minimax_code_global"
        : "minimax_code"
      : account.platform === "qoder_sdk" || account.platform === "qoder_cn"
        ? "qoder"
        : account.platform === "zcode_start_plan"
          ? "zcode"
          : "workbuddy";
  return { view: "services", app, platform };
}
