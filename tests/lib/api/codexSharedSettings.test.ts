import { describe, expect, it, vi } from "vitest";
import * as configApi from "@/lib/api/config";
import { invoke } from "@tauri-apps/api/core";
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue({ app: "codex" }),
}));

describe("Codex shared settings wire contract", () => {
  it("uses independent commands and the shared scope without provider identity", async () => {
    expect(configApi.getCodexSharedSettings).toBeTypeOf("function");
    const request = {
      id: "shared-1",
      scope: "codexSharedSettings" as const,
      draftDigest: "digest",
      revision: "revision",
    };
    await configApi.getCodexSharedSettings();
    expect(invoke).toHaveBeenLastCalledWith("get_codex_shared_settings");
    await configApi.previewCodexSharedSettings(
      'approval_policy = "never"',
      request.id,
    );
    expect(invoke).toHaveBeenLastCalledWith("preview_codex_shared_settings", {
      config: 'approval_policy = "never"',
      requestId: request.id,
    });
    await configApi.saveCodexSharedSettings(
      'approval_policy = "never"',
      request,
    );
    expect(invoke).toHaveBeenLastCalledWith("save_codex_shared_settings", {
      config: 'approval_policy = "never"',
      request,
    });
    await configApi.getCodexSharedSettingsSaveResult(request);
    expect(invoke).toHaveBeenLastCalledWith(
      "get_codex_shared_settings_save_result",
      { request },
    );
  });
});
