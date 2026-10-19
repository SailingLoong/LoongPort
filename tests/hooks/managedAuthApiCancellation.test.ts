import { describe, expect, it, vi } from "vitest";
import { authApi } from "@/lib/api/auth";
const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
describe("managed auth scoped cancellation boundary", () => {
  it.each(["codex_oauth", "xai_oauth", "github_copilot"] as const)(
    "sends only %s's exact pending code",
    async (provider) => {
      await expect(
        authApi.authCancelLogin(provider, "synthetic-A"),
      ).resolves.toBe(true);
      expect(invoke).toHaveBeenLastCalledWith("auth_cancel_login", {
        authProvider: provider,
        deviceCode: "synthetic-A",
        githubDomain: null,
      });
    },
  );
  it("preserves the GitHub Enterprise domain", async () => {
    await authApi.authCancelLogin(
      "github_copilot",
      "synthetic-A",
      "github.example.invalid",
    );
    expect(invoke).toHaveBeenLastCalledWith("auth_cancel_login", {
      authProvider: "github_copilot",
      deviceCode: "synthetic-A",
      githubDomain: "github.example.invalid",
    });
  });
});
