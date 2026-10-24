import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useGeminiConfigState } from "../../src/components/providers/forms/hooks/useGeminiConfigState";

describe("useGeminiConfigState", () => {
  it("preserves raw env while projecting existing parser semantics", () => {
    const { result } = renderHook(() => useGeminiConfigState({}));
    const raw = [
      "# keep this comment",
      "GEMINI_API_KEY=first",
      "GEMINI_API_KEY= final=key ",
      "GOOGLE_GEMINI_BASE_URL= https://example.invalid/v1/ ",
      "GEMINI_MODEL=",
      "CUSTOM_ENV=keep",
      "unfinished line",
    ].join("\n");
    act(() => result.current.handleGeminiEnvChange(raw));
    expect(result.current.geminiEnv).toBe(raw);
    expect(result.current.geminiApiKey).toBe("final=key");
    expect(result.current.geminiBaseUrl).toBe("https://example.invalid/v1/");
    expect(result.current.geminiModel).toBe("");
    expect(result.current.envStringToObj(raw).CUSTOM_ENV).toBe("keep");
  });

  it("alternates field and raw edits without retaining removed fields", () => {
    const { result } = renderHook(() => useGeminiConfigState({}));
    act(() =>
      result.current.handleGeminiEnvChange(
        "CUSTOM_ENV=keep\nGEMINI_API_KEY=initial\nGEMINI_MODEL=old",
      ),
    );
    act(() => result.current.handleGeminiApiKeyChange(" changed "));
    act(() =>
      result.current.handleGeminiBaseUrlChange(" https://example.invalid/// "),
    );
    act(() => result.current.handleGeminiModelChange(" new-model "));
    expect(result.current.envStringToObj(result.current.geminiEnv)).toEqual({
      CUSTOM_ENV: "keep",
      GEMINI_API_KEY: "changed",
      GOOGLE_GEMINI_BASE_URL: "https://example.invalid",
      GEMINI_MODEL: "new-model",
    });
    act(() =>
      result.current.handleGeminiEnvChange("CUSTOM_ENV=keep\nGEMINI_MODEL=raw"),
    );
    expect(result.current.geminiApiKey).toBe("");
    expect(result.current.geminiBaseUrl).toBe("");
    expect(result.current.geminiModel).toBe("raw");
    act(() => result.current.handleGeminiModelChange(""));
    expect(result.current.geminiModel).toBe("");
  });

  it("resets presets and retains extension config validation", () => {
    const { result } = renderHook(() => useGeminiConfigState({}));
    act(() =>
      result.current.resetGeminiConfig(
        { GEMINI_API_KEY: "synthetic", GEMINI_MODEL: "preset", EXTRA: "keep" },
        { custom: true },
      ),
    );
    expect(result.current.geminiApiKey).toBe("synthetic");
    expect(result.current.geminiModel).toBe("preset");
    expect(JSON.parse(result.current.geminiConfig)).toEqual({ custom: true });
    act(() => result.current.handleGeminiConfigChange("{"));
    expect(result.current.geminiConfig).toBe("{");
    expect(result.current.configError).toBe("Invalid JSON format");
    act(() => result.current.resetGeminiConfig({}, {}));
    expect(result.current.geminiEnv).toBe("");
    expect(result.current.geminiApiKey).toBe("");
    expect(result.current.geminiBaseUrl).toBe("");
    expect(result.current.geminiModel).toBe("");
    expect(result.current.configError).toBe("");
  });

  it("loads replacement provider data and clears missing keys", () => {
    const first: { settingsConfig: Record<string, unknown> } = {
      settingsConfig: {
        env: {
          GEMINI_API_KEY: "synthetic",
          GOOGLE_GEMINI_BASE_URL: "https://example.invalid",
          GEMINI_MODEL: "first",
        },
      },
    };
    const { result, rerender } = renderHook(
      ({ initialData }) => useGeminiConfigState({ initialData }),
      { initialProps: { initialData: first } },
    );
    expect(result.current.geminiApiKey).toBe("synthetic");
    rerender({
      initialData: {
        settingsConfig: {
          env: { GEMINI_MODEL: "second" },
        },
      },
    });
    expect(result.current.geminiApiKey).toBe("");
    expect(result.current.geminiBaseUrl).toBe("");
    expect(result.current.geminiModel).toBe("second");
  });
});
