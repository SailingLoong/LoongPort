import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useManagedAuth } from "@/components/providers/forms/hooks/useManagedAuth";
const api = vi.hoisted(() => ({
  start: vi.fn(),
  poll: vi.fn(),
  cancel: vi.fn(),
  status: vi.fn(),
  open: vi.fn(),
  copy: vi.fn(),
}));
vi.mock("@/lib/api", () => ({
  authApi: {
    authStartLogin: api.start,
    authPollForAccount: api.poll,
    authCancelLogin: api.cancel,
    authGetStatus: api.status,
  },
  settingsApi: { openExternal: api.open },
}));
vi.mock("@/lib/clipboard", () => ({ copyText: api.copy }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn() } }));
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (v: unknown) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
}
function device(id: string) {
  return {
    provider: "codex_oauth",
    device_code: id,
    user_code: `code-${id}`,
    verification_uri: `https://example.invalid/${id}`,
    expires_in: 600,
    interval: 5,
  };
}
async function flush() {
  await act(async () => {
    for (let i = 0; i < 12; i++) await Promise.resolve();
    await vi.advanceTimersByTimeAsync(0);
  });
}
function mount(
  provider: "codex_oauth" | "xai_oauth" | "github_copilot" = "codex_oauth",
  domain?: string,
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useManagedAuth(provider, domain), { wrapper });
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  api.status.mockResolvedValue({ accounts: [], authenticated: false });
  api.copy.mockResolvedValue(undefined);
  api.open.mockResolvedValue(undefined);
  api.cancel.mockResolvedValue(undefined);
  api.poll.mockResolvedValue(null);
});
afterEach(() => {
  vi.useRealTimers();
});
describe("managed login session ownership", () => {
  it("cancel ignores a late start without copying, opening or polling", async () => {
    const start = deferred<any>();
    api.start.mockReturnValue(start.promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    start.resolve(device("A"));
    await flush();
    expect(api.copy).not.toHaveBeenCalled();
    expect(api.open).not.toHaveBeenCalled();
    expect(api.poll).not.toHaveBeenCalled();
    expect(h.result.current.pollingState).toBe("idle");
    h.unmount();
  });
  it("unmount ignores a late start", async () => {
    const start = deferred<any>();
    api.start.mockReturnValue(start.promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    h.unmount();
    start.resolve(device("A"));
    await flush();
    expect(api.open).not.toHaveBeenCalled();
    expect(api.poll).not.toHaveBeenCalled();
  });
  it("cancel during copy prevents a late browser open", async () => {
    const copy = deferred<void>();
    api.start.mockResolvedValue(device("A"));
    api.copy.mockReturnValue(copy.promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    copy.resolve();
    await flush();
    expect(api.open).not.toHaveBeenCalled();
    expect(api.poll).not.toHaveBeenCalled();
    h.unmount();
  });
  it("a cancelled A poll cannot clear B state or B polling timer", async () => {
    const pollA = deferred<any>();
    api.start
      .mockResolvedValueOnce(device("A"))
      .mockResolvedValueOnce(device("B"));
    api.poll.mockImplementation((_p, code) =>
      code === "A" ? pollA.promise : Promise.resolve(null),
    );
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    act(() => h.result.current.startAuth());
    await flush();
    pollA.resolve({ id: "late-A" });
    await flush();
    expect(h.result.current.deviceCode?.device_code).toBe("B");
    expect(h.result.current.pollingState).toBe("polling");
    const before = api.poll.mock.calls.filter((c) => c[1] === "B").length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(8000);
    });
    expect(
      api.poll.mock.calls.filter((c) => c[1] === "B").length,
    ).toBeGreaterThan(before);
    h.unmount();
  });
  it("does not run overlapping poll requests", async () => {
    api.start.mockResolvedValue(device("A"));
    api.poll.mockReturnValue(deferred<any>().promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(24000);
    });
    expect(api.poll).toHaveBeenCalledTimes(1);
    h.unmount();
  });
  it("coalesces repeated start clicks into one active session", async () => {
    api.start.mockReturnValue(deferred<any>().promise);
    const h = mount();
    act(() => {
      h.result.current.startAuth();
      h.result.current.startAuth();
    });
    await flush();
    expect(api.start).toHaveBeenCalledTimes(1);
    h.unmount();
  });
  it("late A start cannot replace an active B session", async () => {
    const a = deferred<any>();
    api.start.mockReturnValueOnce(a.promise).mockResolvedValueOnce(device("B"));
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    act(() => h.result.current.startAuth());
    await flush();
    a.resolve(device("A"));
    await flush();
    expect(h.result.current.deviceCode?.device_code).toBe("B");
    expect(api.open.mock.calls.map((c) => c[0])).toEqual([
      "https://example.invalid/B",
    ]);
    h.unmount();
  });
  it("late A errors cannot replace B state", async () => {
    const a = deferred<any>();
    api.start.mockReturnValueOnce(a.promise).mockResolvedValueOnce(device("B"));
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    act(() => h.result.current.startAuth());
    await flush();
    a.reject(new Error("old A"));
    await flush();
    expect(h.result.current.deviceCode?.device_code).toBe("B");
    expect(h.result.current.error).toBeNull();
    h.unmount();
  });
  it("unmounted A poll cannot affect a reopened B hook", async () => {
    const a = deferred<any>();
    api.start
      .mockResolvedValueOnce(device("A"))
      .mockResolvedValueOnce(device("B"));
    api.poll.mockImplementation((_p, code) =>
      code === "A" ? a.promise : Promise.resolve(null),
    );
    const old = mount();
    act(() => old.result.current.startAuth());
    await flush();
    old.unmount();
    const next = mount();
    act(() => next.result.current.startAuth());
    await flush();
    const statusCalls = api.status.mock.calls.length;
    a.resolve({ id: "old-A" });
    await flush();
    expect(next.result.current.deviceCode?.device_code).toBe("B");
    expect(api.status).toHaveBeenCalledTimes(statusCalls);
    next.unmount();
  });
  it("cancel stops the session after external open resolves", async () => {
    const open = deferred<void>();
    api.start.mockResolvedValue(device("A"));
    api.open.mockReturnValue(open.promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    open.resolve();
    await flush();
    expect(api.poll).not.toHaveBeenCalled();
    expect(h.result.current.isAddingAccount).toBe(false);
    h.unmount();
  });

  it("late A poll rejection cannot overwrite B error or polling", async () => {
    const a = deferred<any>();
    api.start
      .mockResolvedValueOnce(device("A"))
      .mockResolvedValueOnce(device("B"));
    api.poll.mockImplementation((_p, code) =>
      code === "A" ? a.promise : Promise.resolve(null),
    );
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    act(() => h.result.current.startAuth());
    await flush();
    a.reject(new Error("expired A"));
    await flush();
    expect(h.result.current.error).toBeNull();
    expect(h.result.current.deviceCode?.device_code).toBe("B");
    expect(h.result.current.pollingState).toBe("polling");
    h.unmount();
  });
  it("changing provider or domain invalidates a pending start", async () => {
    const a = deferred<any>();
    api.start.mockReturnValue(a.promise);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const h = renderHook(
      ({
        provider,
        domain,
      }: {
        provider: "codex_oauth" | "github_copilot";
        domain: string;
      }) => useManagedAuth(provider, domain),
      {
        wrapper,
        initialProps: {
          provider: "codex_oauth" as "codex_oauth" | "github_copilot",
          domain: "old.invalid",
        },
      },
    );
    act(() => h.result.current.startAuth());
    await flush();
    h.rerender({ provider: "github_copilot", domain: "new.invalid" });
    a.resolve(device("A"));
    await flush();
    expect(api.open).not.toHaveBeenCalled();
    expect(h.result.current.isAddingAccount).toBe(false);
    h.unmount();
  });
  it("cancels exactly the late start device code and provider domain", async () => {
    const a = deferred<any>();
    api.start.mockReturnValue(a.promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    a.resolve(device("A"));
    await flush();
    expect(api.cancel).toHaveBeenCalledExactlyOnceWith(
      "codex_oauth",
      "A",
      undefined,
    );
    h.unmount();
  });
  it("cancels an in-flight poll without invoking logout", async () => {
    api.start.mockResolvedValue(device("A"));
    api.poll.mockReturnValue(deferred<any>().promise);
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    await flush();
    expect(api.cancel).toHaveBeenCalledExactlyOnceWith(
      "codex_oauth",
      "A",
      undefined,
    );
    h.unmount();
    expect(api.cancel).toHaveBeenCalledTimes(1);
  });
  it("reports a failed cancellation instead of claiming backend cancellation", async () => {
    api.start.mockResolvedValue(device("A"));
    api.cancel.mockRejectedValue(new Error("cancel unavailable"));
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    await flush();
    expect(h.result.current.error).toContain("cancel unavailable");
    expect(h.result.current.pollingState).toBe("error");
    h.unmount();
  });
  it.each(["codex_oauth", "xai_oauth", "github_copilot"] as const)(
    "routes %s cancellation to its original provider",
    async (provider) => {
      api.start.mockResolvedValue(device("A"));
      const h = mount(provider, "github.example.invalid");
      act(() => h.result.current.startAuth());
      await flush();
      act(() => h.result.current.cancelAuth());
      await flush();
      expect(api.cancel).toHaveBeenCalledExactlyOnceWith(
        provider,
        "A",
        "github.example.invalid",
      );
      h.unmount();
    },
  );
  it("does not let an old cancellation failure replace B", async () => {
    const cancel = deferred<boolean>();
    api.cancel.mockReturnValue(cancel.promise);
    api.start
      .mockResolvedValueOnce(device("A"))
      .mockResolvedValueOnce(device("B"));
    const h = mount();
    act(() => h.result.current.startAuth());
    await flush();
    act(() => h.result.current.cancelAuth());
    act(() => h.result.current.startAuth());
    await flush();
    cancel.reject(new Error("old cancel unavailable"));
    await flush();
    expect(h.result.current.error).toBeNull();
    expect(h.result.current.pollingState).toBe("polling");
    expect(h.result.current.deviceCode?.device_code).toBe("B");
    api.cancel.mockResolvedValue(true);
    h.unmount();
  });
  it("reconciles status when commit won before cancellation", async () => {
    api.cancel.mockResolvedValue(false);
    api.start.mockResolvedValue(device("A"));
    api.poll.mockReturnValue(deferred<any>().promise);
    const h = mount();
    await flush();
    act(() => h.result.current.startAuth());
    await flush();
    api.status.mockResolvedValue({
      accounts: [{ id: "committed-account" }],
      authenticated: true,
    });
    act(() => h.result.current.cancelAuth());
    await flush();
    expect(h.result.current.accounts).toEqual([{ id: "committed-account" }]);
    expect(h.result.current.isAuthenticated).toBe(true);
    h.unmount();
  });
});
