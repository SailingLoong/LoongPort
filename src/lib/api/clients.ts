import { invoke } from "@tauri-apps/api/core";

export interface ClientConnection {
  clientId: string;
  credentialRef: string;
  routeApp: string;
  models: string[];
}
export interface ClientStatus {
  descriptor: { id: string; name: string; routeApps: string[] };
  connection: ClientConnection | null;
  recoveryRequired: boolean;
  status:
    | "notConfigured"
    | "configured"
    | "recoveryRequired"
    | "configurationChanged";
}
export interface ClientCredential {
  id: string;
  clientId: string;
  routeApp: string;
  allowedModels: string[];
  revoked: boolean;
}
export interface ClientPreview {
  token: string;
  changed: boolean;
  modelCount: number;
}
export const clientsApi = {
  list: () => invoke<ClientStatus[]>("list_client_connections"),
  credentials: () => invoke<ClientCredential[]>("list_client_credentials"),
  createCredential: (clientId: string, routeApp: string, models: string[]) =>
    invoke<ClientCredential>("create_client_credential", {
      clientId,
      routeApp,
      models,
    }),
  revoke: (clientId: string, credentialRef: string) =>
    invoke<void>("revoke_client_credential", { clientId, credentialRef }),
  preview: (request: ClientConnection) =>
    invoke<ClientPreview>("preview_client_connection", { request }),
  apply: (request: ClientConnection, token: string) =>
    invoke<void>("apply_client_connection", { request, token }),
  disconnect: (clientId: string) =>
    invoke<void>("disconnect_client", { clientId }),
  recover: (clientId: string) =>
    invoke<void>("recover_client_connection", { clientId }),
};
