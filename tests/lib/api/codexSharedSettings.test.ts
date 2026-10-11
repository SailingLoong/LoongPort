import { describe, expect, it, vi } from "vitest";
import * as configApi from "@/lib/api/config";
import { invoke } from "@tauri-apps/api/core";
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue({ app: "codex" }),
}));

describe("Codex shared settings wire contract", () => {
  it("uses explicit legacy import commands and preserves original key path arrays", async () => {
    expect(configApi.getCodexSharedSettingsImport).toBeTypeOf("function");
    expect(configApi.previewCodexSharedSettingsImport).toBeTypeOf("function");
    await configApi.getCodexSharedSettingsImport();
    expect(invoke).toHaveBeenLastCalledWith("get_codex_shared_settings_import");
    const fields = [
      ["history", "max.bytes"],
      ["tools.web", "search"],
    ];
    await configApi.previewCodexSharedSettingsImport(fields, "import-1");
    expect(invoke).toHaveBeenLastCalledWith(
      "preview_codex_shared_settings_import",
      {
        fields,
        requestId: "import-1",
      },
    );
  });
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

describe("Claude shared settings wire contract", () => {
  it("uses all six Claude commands and echoes exact JSON text and receipt", async () => {
    const request = {
      id: "claude-shared-1",
      scope: "claudeSharedSettings" as const,
      draftDigest: "digest",
      revision: "revision",
    };
    const config =
      '{"limits":{"count":9007199254740993},"ui":{"literal.key":true}}';
    await configApi.getClaudeSharedSettings();
    expect(invoke).toHaveBeenLastCalledWith("get_claude_shared_settings");
    await configApi.previewClaudeSharedSettings(config, request.id);
    expect(invoke).toHaveBeenLastCalledWith("preview_claude_shared_settings", {
      config,
      requestId: request.id,
    });
    await configApi.saveClaudeSharedSettings(config, request);
    expect(invoke).toHaveBeenLastCalledWith("save_claude_shared_settings", {
      config,
      request,
    });
    await configApi.getClaudeSharedSettingsSaveResult(request);
    expect(invoke).toHaveBeenLastCalledWith(
      "get_claude_shared_settings_save_result",
      { request },
    );
    await configApi.getClaudeSharedSettingsImport();
    expect(invoke).toHaveBeenLastCalledWith(
      "get_claude_shared_settings_import",
    );
    const fields = [["ui", "literal.key"]];
    await configApi.previewClaudeSharedSettingsImport(fields, request.id);
    expect(invoke).toHaveBeenLastCalledWith(
      "preview_claude_shared_settings_import",
      { fields, requestId: request.id },
    );
  });
});

describe("Gemini shared settings wire contract", () => {
  it("echoes two original-format texts and the exact Gemini receipt across all six commands", async () => {
    const request = {
      id: "gemini-1",
      scope: "geminiSharedSettings" as const,
      draftDigest: "digest",
      revision: "revision",
    };
    const config = {
      env: '# shared\nGEMINI_CLI_THEME="dark"\n',
      settings: '{ "ui": { "theme": "dark" }, "large": 9007199254740993 }\n',
    };
    const api = configApi.getSharedSettingsApi("gemini");
    await api.get();
    expect(invoke).toHaveBeenLastCalledWith("get_gemini_shared_settings");
    await api.preview(config, request.id);
    expect(invoke).toHaveBeenLastCalledWith("preview_gemini_shared_settings", {
      config,
      requestId: request.id,
    });
    await api.save(config, request);
    expect(invoke).toHaveBeenLastCalledWith("save_gemini_shared_settings", {
      config,
      request,
    });
    await api.result(request);
    expect(invoke).toHaveBeenLastCalledWith(
      "get_gemini_shared_settings_save_result",
      { request },
    );
    await api.getImport();
    expect(invoke).toHaveBeenLastCalledWith(
      "get_gemini_shared_settings_import",
    );
    const fields = [["GEMINI_CLI_THEME"]];
    await api.previewImport(fields, request.id);
    expect(invoke).toHaveBeenLastCalledWith(
      "preview_gemini_shared_settings_import",
      { fields, requestId: request.id },
    );
  });
});
