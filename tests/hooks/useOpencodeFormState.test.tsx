import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useOpencodeFormState } from "@/components/providers/forms/hooks/useOpencodeFormState";

const renderOpencodeFormState = (
  initialSettingsConfig: Record<string, unknown>,
) => {
  let settingsConfig = JSON.stringify(initialSettingsConfig);
  const onSettingsConfigChange = vi.fn((nextConfig: string) => {
    settingsConfig = nextConfig;
  });

  const hook = renderHook(() =>
    useOpencodeFormState({
      appId: "opencode",
      initialData: { settingsConfig: initialSettingsConfig },
      onSettingsConfigChange,
      getSettingsConfig: () => settingsConfig,
    }),
  );

  return {
    ...hook,
    onSettingsConfigChange,
    getSettingsConfig: () => settingsConfig,
  };
};

describe("useOpencodeFormState", () => {
  it("hydrates provider headers from options", () => {
    const { result } = renderOpencodeFormState({
      npm: "@ai-sdk/openai-compatible",
      options: {
        headers: {
          "HTTP-Referer": "https://cc-switch.app",
          "X-Title": "CC Switch",
        },
      },
      models: {},
    });

    expect(result.current.opencodeHeaders).toEqual({
      "HTTP-Referer": "https://cc-switch.app",
      "X-Title": "CC Switch",
    });
  });

  it("writes provider headers to options", () => {
    const { result, getSettingsConfig } = renderOpencodeFormState({
      npm: "@ai-sdk/openai-compatible",
      options: {},
      models: {},
    });

    act(() => {
      result.current.handleOpencodeHeadersChange({
        "X-Title": "CC Switch",
      });
    });

    expect(JSON.parse(getSettingsConfig()).options.headers).toEqual({
      "X-Title": "CC Switch",
    });
  });

  it("removes options.headers when all provider headers are removed", () => {
    const { result, getSettingsConfig } = renderOpencodeFormState({
      npm: "@ai-sdk/openai-compatible",
      options: {
        headers: {
          "X-Title": "CC Switch",
        },
      },
      models: {},
    });

    act(() => {
      result.current.handleOpencodeHeadersChange({});
    });

    expect(JSON.parse(getSettingsConfig()).options.headers).toBeUndefined();
  });

  it("preserves legitimate headers whose names start with header-", () => {
    const { result, getSettingsConfig } = renderOpencodeFormState({
      npm: "@ai-sdk/openai-compatible",
      options: {
        headers: {
          "header-version": "v1",
          "X-Title": "Old",
        },
      },
      models: {},
    });

    act(() => {
      result.current.handleOpencodeHeadersChange({
        "header-version": "v1",
        "X-Title": "New",
      });
    });

    expect(JSON.parse(getSettingsConfig()).options.headers).toEqual({
      "header-version": "v1",
      "X-Title": "New",
    });
  });

  it("preserves legitimate options whose names start with option-", () => {
    const { result, getSettingsConfig } = renderOpencodeFormState({
      npm: "@ai-sdk/openai-compatible",
      options: {
        "option-mode": "legacy",
        timeout: 100,
      },
      models: {},
    });

    act(() => {
      result.current.handleOpencodeExtraOptionsChange({
        "option-mode": "legacy",
        timeout: "200",
        "draft-option:123": "",
      });
    });

    expect(JSON.parse(getSettingsConfig()).options).toEqual({
      "option-mode": "legacy",
      timeout: 200,
    });
  });
  it("preserves untouched JSON-looking strings while explicitly parsing edited JSON values", () => {
    const original = {
      flagText: "false",
      numberText: "123",
      nullText: "null",
      objectText: '{"keep":true}',
      timeout: 100,
    };
    const { result, getSettingsConfig } = renderOpencodeFormState({
      options: original,
      models: {},
    });
    act(() =>
      result.current.handleOpencodeExtraOptionsChange({
        ...result.current.opencodeExtraOptions,
        timeout: "200",
        typedBoolean: "false",
        quotedString: '"123"',
        typedObject: '{"keep":true}',
      }),
    );
    expect(JSON.parse(getSettingsConfig()).options).toEqual({
      ...original,
      timeout: 200,
      typedBoolean: false,
      quotedString: "123",
      typedObject: { keep: true },
    });
  });

  it("never lets extra-option rows overwrite dedicated connection fields", () => {
    const original = {
      baseURL: "https://example.invalid/v1",
      apiKey: "synthetic-key",
      headers: { "X-Test": "keep" },
    };
    const { result, getSettingsConfig } = renderOpencodeFormState({
      options: original,
      models: {},
    });
    act(() =>
      result.current.handleOpencodeExtraOptionsChange({
        apiKey: "123",
        " baseURL ": "false",
        headers: "123",
        timeout: "200",
      }),
    );
    expect(JSON.parse(getSettingsConfig()).options).toEqual({
      ...original,
      timeout: 200,
    });
  });
});
