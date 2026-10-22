import { act, fireEvent, render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { UniversalProviderPanel } from "@/components/universal/UniversalProviderPanel";
import { universalProvidersApi } from "@/lib/api";
import type { UniversalProvider, UniversalProvidersMap } from "@/types";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock("@/components/universal/UniversalProviderCard", () => ({
  UniversalProviderCard: ({
    provider,
    onDuplicate,
  }: {
    provider: UniversalProvider;
    onDuplicate: (provider: UniversalProvider) => void;
  }) => (
    <div>
      {provider.name}
      <button onClick={() => onDuplicate(provider)}>Duplicate provider</button>
    </div>
  ),
}));

vi.mock("@/components/universal/UniversalProviderFormModal", () => ({
  UniversalProviderFormModal: () => null,
}));

vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: () => null,
}));

function deferred() {
  let resolve!: (value: UniversalProvidersMap) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<UniversalProvidersMap>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function providers(name: string): UniversalProvidersMap {
  return {
    provider: {
      id: "provider",
      name,
      providerType: "custom",
      models: {},
      baseUrl: "https://provider.example",
      apiKey: "test-key",
      apps: { claude: true, codex: true, gemini: false },
    },
  };
}

describe("UniversalProviderPanel load lifecycle", () => {
  beforeEach(() => {
    vi.spyOn(universalProvidersApi, "upsert").mockResolvedValue(true);
    vi.spyOn(universalProvidersApi, "sync").mockResolvedValue(true);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("does not report a load error after the panel is unmounted", async () => {
    const request = deferred();
    vi.spyOn(universalProvidersApi, "getAll").mockReturnValue(request.promise);
    const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
    const view = render(<UniversalProviderPanel />);

    view.unmount();
    await act(async () => request.reject(new Error("late load failure")));

    expect(toast.error).not.toHaveBeenCalled();
    expect(errorLog).not.toHaveBeenCalled();
  });

  it("keeps loading while the current StrictMode mount is pending", async () => {
    const previous = deferred();
    const current = deferred();
    vi.spyOn(universalProvidersApi, "getAll")
      .mockReturnValueOnce(previous.promise)
      .mockReturnValueOnce(current.promise);
    render(
      <StrictMode>
        <UniversalProviderPanel />
      </StrictMode>,
    );

    await act(async () => previous.resolve(providers("Stale provider")));
    expect(screen.queryByText("Stale provider")).not.toBeInTheDocument();
    expect(screen.queryByText("还没有统一供应商")).not.toBeInTheDocument();

    await act(async () => current.resolve(providers("Current provider")));
    expect(screen.getByText("Current provider")).toBeInTheDocument();
  });

  it("does not let an earlier refresh replace a newer refresh", async () => {
    const previous = deferred();
    const current = deferred();
    vi.spyOn(universalProvidersApi, "getAll")
      .mockResolvedValueOnce(providers("Initial provider"))
      .mockReturnValueOnce(previous.promise)
      .mockReturnValueOnce(current.promise);
    render(<UniversalProviderPanel />);
    const duplicate = await screen.findByRole("button", {
      name: "Duplicate provider",
    });
    await act(async () => {
      fireEvent.click(duplicate);
      fireEvent.click(duplicate);
    });

    await act(async () => current.resolve(providers("Current provider")));
    await act(async () => previous.resolve(providers("Stale provider")));

    expect(screen.getByText("Current provider")).toBeInTheDocument();
    expect(screen.queryByText("Stale provider")).not.toBeInTheDocument();
  });

  it("does not let an earlier load error end a newer load", async () => {
    const previous = deferred();
    const current = deferred();
    vi.spyOn(universalProvidersApi, "getAll")
      .mockReturnValueOnce(previous.promise)
      .mockReturnValueOnce(current.promise);
    const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <StrictMode>
        <UniversalProviderPanel />
      </StrictMode>,
    );

    await act(async () => previous.reject(new Error("stale failure")));
    expect(toast.error).not.toHaveBeenCalled();
    expect(errorLog).not.toHaveBeenCalled();
    expect(screen.queryByText("还没有统一供应商")).not.toBeInTheDocument();

    await act(async () => current.resolve(providers("Current provider")));
    expect(screen.getByText("Current provider")).toBeInTheDocument();
  });

  it("still reports a current load error and finishes loading", async () => {
    const request = deferred();
    const error = new Error("current load failure");
    vi.spyOn(universalProvidersApi, "getAll").mockReturnValue(request.promise);
    const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<UniversalProviderPanel />);

    await act(async () => request.reject(error));

    expect(toast.error).toHaveBeenCalledWith("加载统一供应商失败");
    expect(errorLog).toHaveBeenCalledWith(
      "Failed to load universal providers:",
      error,
    );
    expect(screen.getByText("还没有统一供应商")).toBeInTheDocument();
  });
});
