/** View destinations only. Credentials, account state and application routing retain their original owners. */
export const accountPlatforms = [
  { id: "github_copilot", name: "GitHub Copilot" },
  { id: "codex_oauth", name: "ChatGPT" },
  { id: "minimax_code", name: "MiniMax Code" },
  { id: "xai_oauth", name: "xAI" },
  { id: "workbuddy", name: "WorkBuddy" },
  { id: "trae", name: "TRAE" },
  { id: "zcode", name: "ZCode" },
] as const;
export type AccountPlatform = (typeof accountPlatforms)[number]["id"];
export const OPEN_ACCOUNT_MANAGER = "loongport:open-account-manager";
export function isAccountPlatform(value: unknown): value is AccountPlatform {
  return accountPlatforms.some((platform) => platform.id === value);
}
