/** Navigation identities only; never account IDs, credentials or pool priorities. */
export const accountPlatforms = [
  { id: "github_copilot", name: "GitHub Copilot", logo: "githubcopilot.svg" },
  { id: "codex_oauth", name: "ChatGPT", logo: "chatgpt.svg" },
  {
    id: "minimax_code",
    name: "MiniMax Code",
    edition: "国内版",
    logo: "minimax.svg",
  },
  {
    id: "minimax_code_global",
    name: "MiniMax Code",
    edition: "国际版",
    logo: "minimax.svg",
  },
  { id: "xai_oauth", name: "xAI", logo: "xai.svg" },
  { id: "workbuddy", name: "WorkBuddy", logo: "workbuddy.svg" },
  { id: "trae_cn", name: "TRAE", edition: "中国版", logo: "trae.png" },
  { id: "trae", name: "TRAE", edition: "国际版", logo: "trae.png" },
  { id: "zcode", name: "ZCode", logo: "zcode.svg" },
  { id: "qoder", name: "Qoder", logo: "qoder.svg" },
  { id: "openrouter", name: "OpenRouter", logo: "openrouter.svg" },
  { id: "opencode", name: "OpenCode", logo: "opencode.svg" },
] as const;
export type AccountPlatform = (typeof accountPlatforms)[number]["id"];
export const OPEN_ACCOUNT_MANAGER = "loongport:open-account-manager";
export function isAccountPlatform(value: unknown): value is AccountPlatform {
  return accountPlatforms.some((platform) => platform.id === value);
}
export function accountPlatformLabel(
  platform: (typeof accountPlatforms)[number],
) {
  return `${platform.name}${"edition" in platform ? ` ${platform.edition}` : ""}`;
}
