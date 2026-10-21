import { invoke } from "@tauri-apps/api/core";

// Deliberately safe DTOs: credentials and upstream identity never enter the UI.
export type TraeRealm = "cn" | "sg" | "us";
export interface TraeAccountView {
  id: string;
  realm: TraeRealm | "cn";
  status: "stored" | "refreshPending" | "retired";
  // Source adapter eligibility, not live model/credit acceptance. Missing CN metadata fails closed.
  nativeSourceSupported?: boolean;
}
export const TRAE_AUTH_HOST = "https://growsg-normal.trae.ai" as const;
export const traeAuthHost = (realm: TraeRealm) =>
  realm === "cn" ? "https://api.trae.cn" : TRAE_AUTH_HOST;
export interface TraeFlowView {
  flowId: string;
  realm: TraeRealm;
  authHost: string;
}
export interface TraeSelection {
  storagePath: string;
  productPath: string;
  realm: TraeRealm;
}

export const traeAccountsApi = {
  list: () => invoke<TraeAccountView[]>("list_trae_accounts"),
  // Selection alone records paths. Only confirm authorizes the bounded read.
  begin: (selection: TraeSelection) =>
    invoke<TraeFlowView>("begin_trae_authorization", { ...selection }),
  confirm: (flowId: string) =>
    invoke<TraeAccountView>("confirm_trae_authorization", { flowId }),
  cancel: (flowId: string) =>
    invoke<void>("cancel_trae_authorization", { flowId }),
  retire: (id: string) => invoke<void>("retire_trae_account", { id }),
};
