import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import { useApplicationOverview } from "../useApplicationOverview";
const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  relay: vi.fn(),
  vendor: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { get: mocks.get },
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: { switchTier: mocks.relay } }));
vi.mock("@/lib/api/vendor", () => ({ vendorApi: { switch: mocks.vendor } }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { error: mocks.error, success: mocks.success, warning: vi.fn() },
}));
const target = {
  providerId: "relay-config",
  name: "Example",
  canSelect: true,
  selection: { kind: "relay" },
} as ApplicationConfiguration;
const providers = {};
function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
let client: QueryClient;
beforeEach(() => {
  vi.clearAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  mocks.get.mockResolvedValue({
    configurations: [],
    recentProviderIds: [],
    isAdditive: false,
  });
});
describe("configuration selection", () => {
  it("keeps the backend confirmation boundary and cancellation writes nothing further", async () => {
    mocks.relay.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Example",
    });
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select(target);
    });
    expect(result.current.confirmation).toBe("Example");
    act(() => result.current.cancel());
    expect(mocks.relay).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
    expect(result.current.confirmation).toBeNull();
  });
  it("replays the exact vendor plan only after the explicit confirmation", async () => {
    mocks.vendor
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Example",
      })
      .mockResolvedValueOnce({
        status: "switched",
        warnings: [],
        chatgptWasRunning: false,
        chatgptRelaunched: false,
      });
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select({
        ...target,
        selection: { kind: "vendor", rowId: 3, planId: "standard" },
      });
    });
    act(() => result.current.confirm(false));
    await waitFor(() =>
      expect(mocks.vendor).toHaveBeenLastCalledWith(
        3,
        "standard",
        "codex",
        false,
      ),
    );
    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(mocks.success).toHaveBeenCalledTimes(1);
  });
  it("reports failure without manufacturing a successful selection", async () => {
    mocks.relay.mockRejectedValue(new Error("Unavailable"));
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select(target);
    });
    expect(mocks.error).toHaveBeenCalled();
    expect(mocks.success).not.toHaveBeenCalled();
    expect(result.current.busy).toBe(false);
  });
  it("does not reopen a confirmation after leaving the application", async () => {
    let resolve!: (value: unknown) => void;
    mocks.relay.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const { result, unmount } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    act(() => {
      void result.current.select(target);
    });
    unmount();
    await act(async () =>
      resolve({ status: "confirmationRequired", targetName: "Example" }),
    );
    expect(mocks.relay).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
  });
});
