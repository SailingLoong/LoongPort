import { useCommonConfigSnippet } from "@/components/providers/forms/hooks/useCommonConfigSnippet";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useCodexCommonConfig } from "@/components/providers/forms/hooks/useCodexCommonConfig";
import { useGeminiCommonConfig } from "@/components/providers/forms/hooks/useGeminiCommonConfig";

const getCommonConfigSnippetMock = vi.fn();
const setCommonConfigSnippetMock = vi.fn();
const extractCommonConfigSnippetMock = vi.fn();
const updateTomlCommonConfigSnippetMock = vi.fn();

vi.mock("@/lib/api", () => ({
  configApi: {
    getCommonConfigSnippet: (...args: unknown[]) =>
      getCommonConfigSnippetMock(...args),
    setCommonConfigSnippet: (...args: unknown[]) =>
      setCommonConfigSnippetMock(...args),
    extractCommonConfigSnippet: (...args: unknown[]) =>
      extractCommonConfigSnippetMock(...args),
    updateTomlCommonConfigSnippet: (...args: unknown[]) =>
      updateTomlCommonConfigSnippetMock(...args),
  },
}));

describe("common config snippet saving", () => {
  beforeEach(() => {
    getCommonConfigSnippetMock.mockResolvedValue("");
    setCommonConfigSnippetMock.mockResolvedValue(undefined);
    extractCommonConfigSnippetMock.mockResolvedValue("");
    updateTomlCommonConfigSnippetMock.mockImplementation(
      async (configToml: string) => configToml,
    );
  });

  it("does not persist an invalid Codex common config snippet", async () => {
    const onConfigChange = vi.fn();
    const { result } = renderHook(() =>
      useCodexCommonConfig({
        codexConfig: 'model = "gpt-5"',
        onConfigChange,
      }),
    );

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    let saved = true;
    await act(async () => {
      saved = await result.current.handleCommonConfigSnippetChange(
        "base_url = https://bad.example/v1",
      );
    });

    expect(saved).toBe(false);
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(onConfigChange).not.toHaveBeenCalled();
    expect(result.current.commonConfigError).toContain("invalid value");
  });

  it("discards stale toggle results when a newer toggle finishes first", async () => {
    getCommonConfigSnippetMock.mockResolvedValue(
      "[tui]\nnotifications = true\n",
    );

    const onConfigChange = vi.fn();
    const { result } = renderHook(() =>
      useCodexCommonConfig({
        codexConfig: 'model = "gpt-5"',
        onConfigChange,
        initialData: { settingsConfig: { config: 'model = "gpt-5"' } },
        initialEnabled: false,
      }),
    );

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    await waitFor(() => expect(result.current.useCommonConfig).toBe(false));

    // 第一次调用（勾选 on 的 merge）挂起，第二次（取消勾选的剥离）立即返回：
    // 模拟后端乱序完成
    let resolveMerge: ((value: string) => void) | undefined;
    updateTomlCommonConfigSnippetMock
      .mockImplementationOnce(
        () =>
          new Promise<string>((resolve) => {
            resolveMerge = resolve;
          }),
      )
      .mockImplementationOnce(async (configToml: string) => configToml);

    await act(async () => {
      const mergePending = result.current.handleCommonConfigToggle(true);
      const removeDone = result.current.handleCommonConfigToggle(false);
      await removeDone;
      // on 的合并结果此时才姗姗来迟——必须被序号守卫丢弃
      resolveMerge?.('model = "gpt-5"\n\n[tui]\nnotifications = true\n');
      await mergePending;
    });

    // 用户最后一次操作是 off：过期的 on 结果不得翻转开关或改写配置
    expect(result.current.useCommonConfig).toBe(false);
    const lastConfig = onConfigChange.mock.calls.at(-1)?.[0] as string;
    expect(lastConfig).not.toContain("[tui]");
  });

  it("discards async merge results when the user edited the config while in flight", async () => {
    getCommonConfigSnippetMock.mockResolvedValue(
      "[tui]\nnotifications = true\n",
    );

    const initialData = { settingsConfig: { config: 'model = "gpt-5"' } };
    const onConfigChange = vi.fn();
    const { result, rerender } = renderHook(
      ({ config }: { config: string }) =>
        useCodexCommonConfig({
          codexConfig: config,
          onConfigChange,
          initialData,
          initialEnabled: false,
        }),
      { initialProps: { config: 'model = "gpt-5"' } },
    );

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    await waitFor(() => expect(result.current.useCommonConfig).toBe(false));

    let resolveMerge: ((value: string) => void) | undefined;
    updateTomlCommonConfigSnippetMock.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          resolveMerge = resolve;
        }),
    );

    let togglePending: Promise<void> = Promise.resolve();
    act(() => {
      togglePending = result.current.handleCommonConfigToggle(true);
    });

    // merge 在飞期间，用户在编辑器里手动改了 config（不经过 hook，
    // 序号不变，只有 codexConfig prop 变化）
    rerender({ config: 'model = "gpt-6-user-edit"' });

    await act(async () => {
      resolveMerge?.('model = "gpt-5"\n\n[tui]\nnotifications = true\n');
      await togglePending;
    });

    // 基于陈旧基线的合并结果必须被丢弃，不得覆盖用户的手动编辑
    expect(onConfigChange).not.toHaveBeenCalled();
    expect(result.current.useCommonConfig).toBe(false);
  });

  it("does not persist an invalid Gemini common config snippet", async () => {
    const onEnvChange = vi.fn();
    const { result } = renderHook(() =>
      useGeminiCommonConfig({
        envValue: "",
        onEnvChange,
        envStringToObj: () => ({}),
        envObjToString: () => "",
      }),
    );

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    let saved = false;
    act(() => {
      saved = result.current.handleCommonConfigSnippetChange(
        JSON.stringify({ GEMINI_MODEL: 123 }),
      );
    });

    expect(saved).toBe(false);
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(onEnvChange).not.toHaveBeenCalled();
    expect(result.current.commonConfigError).toBe(
      "geminiConfig.commonConfigInvalidValues",
    );
  });
});

const frozenHook = (
  app: string,
  readOnly: boolean,
  onChange: (value: string) => void,
  enabled = true,
) => {
  if (app === "claude")
    return useCommonConfigSnippet({
      settingsConfig: "{}",
      onConfigChange: onChange,
      readOnly,
      enabled,
    });
  if (app === "codex")
    return useCodexCommonConfig({
      codexConfig: "",
      onConfigChange: onChange,
      readOnly,
      enabled,
    });
  return useGeminiCommonConfig({
    envValue: "",
    onEnvChange: onChange,
    envStringToObj: () => ({}),
    envObjToString: () => "",
    readOnly,
    enabled,
  });
};

it.each(["claude", "codex", "gemini"])(
  "keeps %s snippets readable but rejects frozen save, clear, extract and merge",
  async (app) => {
    getCommonConfigSnippetMock.mockResolvedValue(
      app === "codex" ? "[tui]\nnotifications = true" : '{"safe":"value"}',
    );
    const onChange = vi.fn();
    const { result } = renderHook(() => frozenHook(app, true, onChange));
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.commonConfigSnippet).toContain(
      app === "codex" ? "notifications" : "safe",
    );
    await act(async () => {
      await result.current.handleCommonConfigSnippetChange("");
      await result.current.handleCommonConfigSnippetChange(
        app === "codex" ? "foo = true" : '{"safe":"changed"}',
      );
      await result.current.handleExtract();
      await result.current.handleCommonConfigToggle(true);
    });
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(extractCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  },
);

it.each(["claude", "codex", "gemini"])(
  "checks current %s admission again after pending extraction resolves",
  async (app) => {
    getCommonConfigSnippetMock.mockResolvedValue("");
    let finish!: (value: string) => void;
    extractCommonConfigSnippetMock.mockReturnValue(
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
    );
    const { result, rerender } = renderHook(
      ({ frozen }) => frozenHook(app, frozen, vi.fn()),
      { initialProps: { frozen: false } },
    );
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.handleExtract();
    });
    rerender({ frozen: true });
    await act(async () => {
      finish(app === "codex" ? "foo = true" : '{"safe":"value"}');
      await pending;
    });
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
  },
);

it.each(["claude", "codex", "gemini"])(
  "preserves %s legacy storage when a delayed load loses write admission",
  async (app) => {
    const key =
      app === "claude"
        ? "cc-switch:common-config-snippet"
        : `cc-switch:${app}-common-config-snippet`;
    localStorage.setItem(
      key,
      app === "codex" ? "foo = true" : '{"safe":"legacy"}',
    );
    let finish!: (value: string) => void;
    getCommonConfigSnippetMock.mockReturnValue(
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
    );
    const { rerender } = renderHook(
      ({ frozen }) => frozenHook(app, frozen, vi.fn()),
      { initialProps: { frozen: false } },
    );
    const previous = finish;
    rerender({ frozen: true });
    await act(async () => {
      previous("");
      finish("");
    });
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(localStorage.getItem(key)).not.toBeNull();
    localStorage.removeItem(key);
  },
);

it.each(["claude", "codex", "gemini"])(
  "refuses previously captured %s save callbacks after freeze",
  async (app) => {
    getCommonConfigSnippetMock.mockResolvedValue("");
    const { result, rerender } = renderHook(
      ({ frozen }) => frozenHook(app, frozen, vi.fn()),
      { initialProps: { frozen: false } },
    );
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    const save = result.current.handleCommonConfigSnippetChange;
    rerender({ frozen: true });
    await act(async () => {
      await save("");
    });
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
  },
);

it.each(["claude", "codex", "gemini"])(
  "keeps the %s legacy migration on admitted source17",
  async (app) => {
    const key =
      app === "claude"
        ? "cc-switch:common-config-snippet"
        : `cc-switch:${app}-common-config-snippet`;
    const value = app === "codex" ? "foo = true" : '{"safe":"legacy"}';
    localStorage.setItem(key, value);
    getCommonConfigSnippetMock.mockResolvedValue("");
    setCommonConfigSnippetMock.mockResolvedValue(undefined);
    renderHook(() => frozenHook(app, false, vi.fn()));
    await waitFor(() =>
      expect(setCommonConfigSnippetMock).toHaveBeenCalledWith(app, value),
    );
    await waitFor(() => expect(localStorage.getItem(key)).toBeNull());
  },
);

it.each(["codex", "gemini"])(
  "does not load or migrate inactive %s hooks in another application form",
  async (app) => {
    const key = `cc-switch:${app}-common-config-snippet`;
    localStorage.setItem(key, '{"safe":"legacy"}');
    const { result } = renderHook(() => frozenHook(app, false, vi.fn(), false));
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(getCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(setCommonConfigSnippetMock).not.toHaveBeenCalled();
    expect(localStorage.getItem(key)).not.toBeNull();
    localStorage.removeItem(key);
  },
);

it.each(["claude", "codex", "gemini"])(
  "waits for %s legacy reload before initializing a newly admitted draft",
  async (app) => {
    const key =
      app === "claude"
        ? "cc-switch:common-config-snippet"
        : `cc-switch:${app}-common-config-snippet`;
    if (app === "codex")
      updateTomlCommonConfigSnippetMock.mockImplementation(
        async (_config: string, snippet: string) => snippet,
      );
    const legacy =
      app === "codex" ? "legacy_shared = true" : '{"LEGACY_SHARED":"retained"}';
    localStorage.setItem(key, legacy);
    let finish!: (value: string) => void;
    getCommonConfigSnippetMock.mockResolvedValueOnce("").mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const onChange = vi.fn();
    const { result, rerender } = renderHook(
      ({ frozen }) => {
        if (app === "gemini")
          return useGeminiCommonConfig({
            envValue: "",
            onEnvChange: onChange,
            envStringToObj: () => ({}),
            envObjToString: (value) => JSON.stringify(value),
            readOnly: frozen,
          });
        return frozenHook(app, frozen, onChange);
      },
      { initialProps: { frozen: true } },
    );
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    rerender({ frozen: false });
    await act(async () => {
      finish("");
    });
    await waitFor(() =>
      expect(setCommonConfigSnippetMock).toHaveBeenCalledWith(app, legacy),
    );
    await waitFor(() =>
      expect(
        onChange.mock.calls.some(([value]) =>
          String(value).toLowerCase().includes("legacy_shared"),
        ),
      ).toBe(true),
    );
    localStorage.removeItem(key);
  },
);
