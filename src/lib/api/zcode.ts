import { invoke } from "@tauri-apps/api/core";

export type ZCodeApiType =
  "anthropic-messages" | "openai-chat-completions" | "openai-responses";
export interface ZCodeProvider {
  id: string;
  name: string;
  apiType: string;
  baseUrl: string;
  models: string[];
  hasApiKey: boolean;
  managed: boolean;
}
export interface ZCodeConfig {
  revision: string;
  providers: ZCodeProvider[];
}
export interface ZCodeProviderInput {
  id: string | null;
  revision: string;
  name: string;
  apiType: ZCodeApiType;
  baseUrl: string;
  apiKey: string | null;
  models: string[];
}
export const zcodeApi = {
  read: () => invoke<ZCodeConfig>("get_zcode_config"),
  save: (input: ZCodeProviderInput) =>
    invoke<ZCodeConfig>("save_zcode_provider", { input }),
  remove: (id: string, revision: string) =>
    invoke<ZCodeConfig>("remove_zcode_provider", { id, revision }),
};
