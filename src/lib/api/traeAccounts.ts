import { invoke } from "@tauri-apps/api/core";

// Deliberately safe DTOs: credentials and upstream identity never enter the UI.
export type TraeRealm = "sg" | "us";
export interface TraeAccountView {
  id: string;
  realm: TraeRealm | "cn";
  status: "stored" | "refreshPending" | "retired";
}
export const TRAE_AUTH_HOST = "https://growsg-normal.trae.ai" as const;
export interface TraeFlowView {
  flowId: string;
  realm: TraeRealm;
  authHost: typeof TRAE_AUTH_HOST;
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
