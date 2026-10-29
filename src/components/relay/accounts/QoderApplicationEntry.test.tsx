import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Provider } from "@/types";
import type { ServiceSource } from "@/lib/api/serviceSources";
import {
  QoderApplicationEntry,
  qoderApplicationSources,
} from "./QoderApplicationEntry";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const provider = (id = "provider-a", providerType = "qoder_sdk") =>
  ({ id, name: id, settingsConfig: {}, meta: { providerType } }) as Provider;
const source = (id = "provider-a", platform = "qoder_sdk"): ServiceSource => ({
  sourceId: "native:opaque-source",
  account: {
    kind: "nativeAccount",
    platform: platform as "qoder_sdk",
    id: "verified-account",
  },
  name: "Native configuration",
  serviceName: null,
  accountLabel: null,
  configurationName: null,
  bindings: [
    {
      appId: "codex",
      providerId: id,
      model: "verified-model",
      usageState: "inactive",
      canSelect: false,
    },
  ],
});
function mount(row = provider()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const onOpenApp = vi.fn();
  const view = (p: Provider, sourceGeneration?: string) => (
    <QueryClientProvider client={client}>
      <QoderApplicationEntry
        provider={p}
        sourceGeneration={sourceGeneration}
        onOpenApp={onOpenApp}
      />
    </QueryClientProvider>
  );
  return { ...render(view(row)), view, onOpenApp };
}

describe("Qoder original application projection", () => {
  it("uses the exact original Codex binding rather than equating account and Provider IDs", () => {
    const verified = source();
    expect(
      qoderApplicationSources(provider(), [source("other"), verified]),
    ).toEqual([verified]);
  });
  it("leaves unknown providers, unbound accounts and cross-region identities empty", () => {
    expect(
      qoderApplicationSources(provider("provider-a", "unknown"), [source()]),
    ).toEqual([]);
    expect(qoderApplicationSources(provider(), [])).toEqual([]);
    expect(
      qoderApplicationSources(provider(), [source("provider-a", "qoder_cn")]),
    ).toEqual([]);
  });
  it("maps device Providers to the backend CN namespace without inventing identity", () => {
    const verified = source("provider-a", "qoder_cn");
    expect(
      qoderApplicationSources(provider("provider-a", "qoder_cn_device"), [
        verified,
      ]),
    ).toEqual([verified]);
  });
  it("projects only the selected Codex Provider binding without mutating the source", () => {
    const verified = source();
    // Defensive synthetic DTO: the backend currently rejects duplicate app bindings.
    verified.bindings.push(
      {
        appId: "codex",
        providerId: "other-provider",
        model: "other-model",
        usageState: "enabled",
        canSelect: true,
      },
      {
        appId: "claude",
        providerId: "projection",
        model: "claude-model",
        usageState: "enabled",
        canSelect: true,
      },
      {
        appId: "claude",
        providerId: "provider-a",
        model: "same-id-other-app",
        usageState: "enabled",
        canSelect: true,
      },
    );
    const before = structuredClone(verified);
    const result = qoderApplicationSources(provider(), [verified]);
    expect(result).toEqual([{ ...before, bindings: [before.bindings[0]] }]);
    expect(verified).toEqual(before);
  });
  it("renders the native model and delegates explicit navigation only, without writes or identity establishment", async () => {
    vi.mocked(invoke).mockReset().mockResolvedValue([source()]);
    const { onOpenApp } = mount();
    fireEvent.click(
      await screen.findByRole("button", { name: /verified-model/ }),
    );
    expect(onOpenApp).toHaveBeenCalledExactlyOnceWith("codex");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "get_service_sources",
    ]);
  });
  it("does not display a previous account's late response after switching Providers", async () => {
    let resolveOld!: (value: ServiceSource[]) => void;
    vi.mocked(invoke)
      .mockReset()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve;
          }),
      )
      .mockResolvedValueOnce([source("provider-b")]);
    const result = mount();
    result.rerender(result.view(provider("provider-b")));
    await screen.findByRole("button", { name: /verified-model/ });
    await act(async () =>
      resolveOld([
        {
          ...source(),
          bindings: [{ ...source().bindings[0], model: "late-old-model" }],
        },
      ]),
    );
    expect(screen.queryByText("late-old-model")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /verified-model/ }),
    ).toBeInTheDocument();
  });
  it("isolates source revision changes and exposes unknown/error states without cached bindings", async () => {
    vi.mocked(invoke)
      .mockReset()
      .mockResolvedValueOnce([source()])
      .mockRejectedValueOnce(new Error("unavailable"));
    const result = mount();
    await screen.findByRole("button", { name: /verified-model/ });
    result.rerender(result.view(provider(), "new-generation"));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(
      screen.queryByRole("button", { name: /verified-model/ }),
    ).not.toBeInTheDocument();
  });
  it("shows no consumer action when the backend has no verified binding", async () => {
    vi.mocked(invoke).mockReset().mockResolvedValue([]);
    mount();
    await screen.findByText(/暂无后端确认的应用绑定/);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
  it("shows and navigates only the selected Provider in a multi-binding source", async () => {
    const verified = source();
    verified.bindings.push(
      {
        appId: "codex",
        providerId: "other-provider",
        model: "other-model",
        usageState: "enabled",
        canSelect: true,
      },
      {
        appId: "claude",
        providerId: "projection",
        model: "claude-model",
        usageState: "enabled",
        canSelect: true,
      },
    );
    vi.mocked(invoke).mockReset().mockResolvedValue([verified]);
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const { onOpenApp } = mount();
      const button = await screen.findByRole("button", {
        name: /verified-model/,
      });
      expect(
        screen.getByRole("region", { name: "当前配置用于哪些应用" }),
      ).toBeInTheDocument();
      expect(
        screen.getByText(/仅显示当前 Codex 供应商配置/),
      ).toBeInTheDocument();
      expect(screen.queryByText("other-model")).not.toBeInTheDocument();
      expect(screen.queryByText("claude-model")).not.toBeInTheDocument();
      expect(screen.getAllByRole("button")).toHaveLength(1);
      expect(onOpenApp).not.toHaveBeenCalled();
      expect(errors).not.toHaveBeenCalled();
      fireEvent.click(button);
      expect(onOpenApp).toHaveBeenCalledExactlyOnceWith("codex");
      expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
        "get_service_sources",
      ]);
    } finally {
      errors.mockRestore();
    }
  });
  it("switches between two verified Provider sources without exposing the other model", async () => {
    const a = { ...source("provider-a"), sourceId: "source-a" };
    const b = {
      ...source("provider-b"),
      sourceId: "source-b",
      bindings: [{ ...source("provider-b").bindings[0], model: "model-b" }],
    };
    vi.mocked(invoke).mockReset().mockResolvedValue([a, b]);
    const result = mount();
    await screen.findByRole("button", { name: /verified-model/ });
    expect(screen.queryByText("model-b")).not.toBeInTheDocument();
    result.rerender(result.view(provider("provider-b")));
    await screen.findByRole("button", { name: /model-b/ });
    expect(screen.queryByText("verified-model")).not.toBeInTheDocument();
  });
  it("does not duplicate React keys for same-app different-Provider synthetic bindings", async () => {
    const verified = source();
    verified.bindings.push({
      appId: "codex",
      providerId: "other-provider",
      model: "foreign",
      usageState: "inactive",
      canSelect: false,
    });
    vi.mocked(invoke).mockReset().mockResolvedValue([verified]);
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      mount();
      await screen.findByRole("button", { name: /verified-model/ });
      expect(
        errors.mock.calls.some((call) => String(call[0]).includes("same key")),
      ).toBe(false);
    } finally {
      errors.mockRestore();
    }
  });
  it("shows no consumer action for an unknown Provider type", async () => {
    vi.mocked(invoke).mockReset().mockResolvedValue([source()]);
    mount(provider("provider-a", "unknown"));
    await screen.findByText(/暂无后端确认的应用绑定/);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
  it("fails closed when a synthetic DTO repeats the same binding identity", async () => {
    const verified = source();
    verified.bindings.push({
      ...verified.bindings[0],
      model: "conflicting-model",
    });
    expect(qoderApplicationSources(provider(), [verified])).toEqual([]);
    vi.mocked(invoke).mockReset().mockResolvedValue([verified]);
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      mount();
      await screen.findByText(/暂无后端确认的应用绑定/);
      expect(screen.queryByRole("button")).not.toBeInTheDocument();
      expect(
        errors.mock.calls.some((call) => String(call[0]).includes("same key")),
      ).toBe(false);
    } finally {
      errors.mockRestore();
    }
  });
});
