import { describe, expect, it } from "vitest";
import { parse as parseToml } from "smol-toml";
import {
  codexProviderPresets,
  generateThirdPartyConfig,
} from "@/config/codexProviderPresets";

describe("Codex provider context-window presets", () => {
  it("keeps generic third-party configs capability-driven", () => {
    const parsed = parseToml(
      generateThirdPartyConfig(
        "custom",
        "https://example.com/v1",
        "gpt-5.6-sol",
      ),
    ) as Record<string, unknown>;

    expect(parsed.model_context_window).toBeUndefined();
    expect(parsed.model_auto_compact_token_limit).toBeUndefined();
  });

  it("places E-FlowCode's verified million-token opt-in at TOML top level", () => {
    const preset = codexProviderPresets.find(
      (candidate) => candidate.name === "E-FlowCode",
    );
    expect(preset).toBeDefined();

    const parsed = parseToml(preset!.config) as {
      model_context_window?: number;
      model_auto_compact_token_limit?: number;
      model_providers?: {
        custom?: Record<string, unknown>;
      };
    };

    expect(parsed.model_context_window).toBe(1_050_000);
    expect(parsed.model_auto_compact_token_limit).toBe(900_000);
    expect(
      parsed.model_providers?.custom?.model_context_window,
    ).toBeUndefined();
    expect(
      parsed.model_providers?.custom?.model_auto_compact_token_limit,
    ).toBeUndefined();
  });
});
