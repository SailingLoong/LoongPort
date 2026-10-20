/** Source identity is independent of the consuming application and display tab. */
export type ServiceEntryIntent =
  | { kind: "account-boundary"; platform: "qoder" }
  | { kind: "provider-preset"; preset: "OpenRouter" }
  | { kind: "vendor"; vendorId: "opencode" };

export function serviceEntryIntent(
  platform: string,
): ServiceEntryIntent | undefined {
  switch (platform) {
    case "qoder":
      return { kind: "account-boundary", platform: "qoder" };
    case "openrouter":
      return { kind: "provider-preset", preset: "OpenRouter" };
    case "opencode":
      return { kind: "vendor", vendorId: "opencode" };
    default:
      return undefined;
  }
}
