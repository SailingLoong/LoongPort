import { describe, expect, it } from "vitest";
import { parse as parseToml } from "smol-toml";
import {
  extractCodexBaseUrl,
  extractCodexWireApi,
  setCodexBaseUrl,
  setCodexRemoteCompaction,
  setCodexWireApi,
  updateCodexExperimentalBearerToken,
} from "@/utils/providerConfigUtils";

describe("Codex quoted provider table identity", () => {
  it.each([
    ["custom", '["model_providers"."custom"]'],
    ["custom", "[ model_providers . 'custom' ]"],
    ["vendor.gateway", '[model_providers."vendor.gateway"]'],
    ["vendor]gateway#1", "[model_providers.'vendor]gateway#1']"],
    ['vendor"gateway', String.raw`[model_providers."vendor\"gateway"]`],
  ])("edits only %s via its legal header %s", (providerId, header) => {
    const input = [
      `model_provider = ${JSON.stringify(providerId)}`,
      "[model_providers.backup]",
      'base_url = "https://backup.example/v1"',
      'wire_api = "responses"',
      `${header} # preserve the active header`,
      'name = "Gateway"',
      'base_url = "https://active.example/anthropic"',
      'wire_api = "anthropic"',
      'experimental_bearer_token = "synthetic-active-token"',
    ].join("\n");
    const before = parseToml(input) as any;

    expect(extractCodexBaseUrl(input)).toBe("https://active.example/anthropic");
    expect(extractCodexWireApi(input)).toBe("anthropic");

    let output = setCodexBaseUrl(input, "https://updated.example/v1");
    output = setCodexWireApi(output, "responses");
    output = updateCodexExperimentalBearerToken(output, "synthetic-new-token");
    output = setCodexRemoteCompaction(output, true);
    const after = parseToml(output) as any;

    expect(after.model_providers[providerId]).toEqual({
      name: "OpenAI",
      base_url: "https://updated.example/v1",
      wire_api: "responses",
      experimental_bearer_token: "synthetic-new-token",
    });
    expect(after.model_providers.backup).toEqual(before.model_providers.backup);
    expect(output).toContain(`${header} # preserve the active header`);
  });

  it.each(["https://updated.example/v1", ""])(
    "preserves a quoted backup key containing ] when the active URL becomes %j",
    (baseUrl) => {
      const input = [
        'model_provider = "custom"',
        "[model_providers.custom]",
        'base_url = "https://active.example/v1"',
        '[model_providers."backup]name"]',
        'base_url = "https://backup.example/v1"',
        'experimental_bearer_token = "synthetic-backup-token"',
      ].join("\n");
      const before = parseToml(input) as any;

      const output = setCodexBaseUrl(input, baseUrl);
      const after = parseToml(output) as any;

      expect(after.model_providers.custom.base_url).toBe(baseUrl || undefined);
      expect(after.model_providers["backup]name"]).toEqual(
        before.model_providers["backup]name"],
      );
      expect(
        updateCodexExperimentalBearerToken(input, "synthetic-new-token"),
      ).toBe(input);
    },
  );

  it("does not confuse a dotted provider ID with a nested provider table", () => {
    const input = [
      'model_provider = "custom.extra"',
      "[model_providers.custom.extra]",
      'base_url = "https://nested.example/v1"',
      '[model_providers."custom.extra"]',
      'base_url = "https://active.example/v1"',
    ].join("\n");
    const before = parseToml(input) as any;

    const after = parseToml(
      setCodexBaseUrl(input, "https://updated.example/v1"),
    ) as any;

    expect(after.model_providers["custom.extra"].base_url).toBe(
      "https://updated.example/v1",
    );
    expect(after.model_providers.custom).toEqual(before.model_providers.custom);
  });

  it("does not recover a base URL from a quoted MCP table", () => {
    const input = [
      'model_provider = "custom"',
      '["model_providers"."custom"]',
      'name = "Active"',
      '["mcp_servers"."backup]transport"]',
      'base_url = "https://mcp.example/transport"',
    ].join("\n");
    const before = parseToml(input) as any;

    expect(extractCodexBaseUrl(input)).toBeUndefined();
    const after = parseToml(
      setCodexBaseUrl(input, "https://active.example/v1"),
    ) as any;

    expect(after.model_providers.custom.base_url).toBe(
      "https://active.example/v1",
    );
    expect(after.mcp_servers).toEqual(before.mcp_servers);
  });
});
