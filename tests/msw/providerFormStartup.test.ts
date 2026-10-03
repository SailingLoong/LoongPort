import { invoke } from "@tauri-apps/api/core";
import { expect, it } from "vitest";

it("resolves provider form startup commands in the native test fixture", async () => {
  await expect(
    invoke("get_common_config_snippet", { appType: "claude" }),
  ).resolves.toBeNull();
  await expect(invoke("preset_referral_urls")).resolves.toEqual({});
  await expect(invoke("get_claude_desktop_default_routes")).resolves.toEqual([
    {
      routeId: "claude-sonnet-5",
      envKey: "ANTHROPIC_DEFAULT_SONNET_MODEL",
      supports1m: true,
    },
    {
      routeId: "claude-opus-5",
      envKey: "ANTHROPIC_DEFAULT_OPUS_MODEL",
      supports1m: true,
    },
    {
      routeId: "claude-haiku-4-5",
      envKey: "ANTHROPIC_DEFAULT_HAIKU_MODEL",
      supports1m: true,
    },
    {
      routeId: "claude-fable-5",
      envKey: "ANTHROPIC_DEFAULT_FABLE_MODEL",
      supports1m: true,
    },
  ]);
  await expect(
    invoke("auth_get_status", { authProvider: "codex_oauth" }),
  ).resolves.toEqual({
    provider: "codex_oauth",
    authenticated: false,
    default_account_id: null,
    accounts: [],
  });
});
