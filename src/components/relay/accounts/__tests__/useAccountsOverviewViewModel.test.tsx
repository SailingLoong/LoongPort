import { act, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { StrictMode, useEffect } from "react";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { PreservedView } from "@/components/ui/PreservedView";
import type {
  PlatformPresenceSnapshot,
  PlatformAccountSummary,
} from "../useAccountPlatformPresence";
import type { ServiceAccount } from "../useServiceAccounts";
import { createAccountOverviewPreferencesRepository } from "../accountOverviewPreferences";
import { useAccountsOverviewViewModel } from "../useAccountsOverviewViewModel";

const source = vi.hoisted(() => ({
  presence: {
    data: undefined as PlatformPresenceSnapshot | undefined,
    isPending: false,
    isFetching: false,
    refetch: vi.fn(),
  },
  service: {
    accounts: [] as ServiceAccount[],
    error: null as Error | null,
    isPending: false,
    reload: vi.fn(),
  },
  invoke: vi.fn(),
}));
vi.mock("../useAccountPlatformPresence", () => ({
  useAccountPlatformPresence: () => source.presence,
}));
vi.mock("../useServiceAccounts", () => ({
  useServiceAccounts: () => source.service,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: source.invoke }));
const now = 1800000000000;
const account = (id: string, balance: number | null): PlatformAccountSummary =>
  ({
    source: "workbuddy",
    platform: "workbuddy",
    id,
    label: id,
    snapshot: {
      kind: "workbuddy",
      value: { credits: { totalRemaining: balance }, autoClaimEnabled: false },
    },
  }) as PlatformAccountSummary;
const mm: PlatformAccountSummary = {
  source: "minimax_code",
  platform: "minimax_code",
  id: "mm",
  label: "MiniMax",
};
function repository(initial: unknown = null, fail = false) {
  let value = initial === null ? null : JSON.stringify(initial);
  const storage = {
    getItem: vi.fn(() => value),
    setItem: vi.fn((_key: string, next: string) => {
      if (fail) throw new Error("quota");
      value = next;
    }),
  };
  return {
    repo: createAccountOverviewPreferencesRepository(storage),
    storage,
    stored: () => value,
  };
}
beforeEach(() => {
  vi.clearAllMocks();
  source.presence.data = {
    accounts: [account("private-account", 20), account("zero", 0), mm],
    platforms: ["workbuddy", "minimax_code"],
    unavailable: [],
    connections: [],
  };
  source.presence.isPending = source.presence.isFetching = false;
  source.presence.refetch.mockResolvedValue(undefined);
  source.service.accounts = [];
  source.service.isPending = false;
  source.service.error = null;
  source.service.reload.mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

it("composes original query snapshots, hides only exhausted rows, and retains original bulk scope", () => {
  const { repo, storage } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  expect(result.current.orderedGroups.map((group) => group.id)).toEqual([
    "minimax_code",
    "workbuddy",
  ]);
  const wb = result.current.orderedGroups.find(
    (group) => group.id === "workbuddy",
  )!;
  expect(wb.rows.map((row) => row.accountId)).toEqual(["private-account"]);
  expect(wb.hiddenExhaustedCount).toBe(1);
  expect(result.current.scopeAccounts).toHaveLength(3);
  expect(result.current.counts).toEqual({
    total: 3,
    scope: 3,
    visible: 2,
    hiddenExhausted: 1,
  });
  expect(storage.setItem).not.toHaveBeenCalled();
  expect(source.invoke).not.toHaveBeenCalled();
});
it("tab and group reorder intents consume the same orderedGroups and write exactly once per move", () => {
  const { repo, storage, stored } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  const tabIntent = () =>
    result.current.reorderGroup("workbuddy", "minimax_code");
  const groupIntent = () =>
    result.current.reorderGroup("minimax_code", "workbuddy");
  act(tabIntent);
  expect(result.current.orderedGroups.map((group) => group.id)).toEqual([
    "workbuddy",
    "minimax_code",
  ]);
  act(groupIntent);
  expect(result.current.orderedGroups.map((group) => group.id)).toEqual([
    "minimax_code",
    "workbuddy",
  ]);
  expect(storage.setItem).toHaveBeenCalledTimes(2);
  expect(stored()).not.toMatch(/private-account|selection|generation|token/);
  expect(source.invoke).not.toHaveBeenCalled();
});
it("preserves absent groups in preferences and restores their order when original query data returns", () => {
  const { repo, storage } = repository({
    version: 1,
    groupOrder: ["qoder", "workbuddy", "minimax_code"],
    hideExhausted: true,
  });
  const { result, rerender } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  source.presence.data = {
    ...source.presence.data!,
    accounts: [
      ...source.presence.data!.accounts!,
      { source: "qoder", platform: "qoder", id: "q", label: "Qoder" },
    ],
    platforms: [...source.presence.data!.platforms, "qoder"],
  };
  rerender();
  expect(result.current.orderedGroups.map((group) => group.id)).toEqual([
    "qoder",
    "workbuddy",
    "minimax_code",
  ]);
  expect(storage.setItem).not.toHaveBeenCalled();
});
it("keeps session updates after preference writes fail, without claiming they were persisted", () => {
  const { repo, storage } = repository(null, true);
  const { result, rerender } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  act(() => result.current.reorderGroup("workbuddy", "minimax_code"));
  expect(result.current.orderedGroups[0].id).toBe("workbuddy");
  expect(result.current.preferencePersistence).toBe("sessionOnly");
  rerender();
  expect(storage.setItem).toHaveBeenCalledTimes(1);
});
it("collapse, filter and hiding change presentation only and retain visited manager identities", () => {
  const onOpenDetails = vi.fn();
  const { repo } = repository();
  const { result, rerender } = renderHook(
    ({ activeManager }) =>
      useAccountsOverviewViewModel({
        preferencesRepository: repo,
        activeManager,
        onOpenDetails,
      }),
    {
      initialProps: {
        activeManager: "workbuddy" as "workbuddy" | "minimax_code",
      },
    },
  );
  act(() => {
    result.current.toggleCollapsed("workbuddy");
    result.current.setFilter("minimax_code");
    result.current.setHideExhausted(false);
  });
  expect(
    result.current.orderedGroups.find((group) => group.id === "workbuddy")
      ?.collapsed,
  ).toBe(true);
  expect(result.current.scopeAccounts).toEqual([mm]);
  expect(result.current.managerIds).toContain("workbuddy");
  rerender({ activeManager: "minimax_code" });
  expect(result.current.managerIds).toEqual(["workbuddy", "minimax_code"]);
  expect(onOpenDetails).not.toHaveBeenCalled();
  expect(source.invoke).not.toHaveBeenCalled();
});
it("retaining the VM manager list does not unmount an existing authorization owner on display events", () => {
  const disposed = vi.fn();
  const { repo } = repository();
  function Manager() {
    useEffect(() => () => disposed(), []);
    return <span>Retained owner</span>;
  }
  function Harness() {
    const vm = useAccountsOverviewViewModel({
      preferencesRepository: repo,
      activeManager: "workbuddy",
    });
    return (
      <>
        <button
          onClick={() => {
            vm.toggleCollapsed("workbuddy");
            vm.setFilter("minimax_code");
          }}
        >
          Display only
        </button>
        {vm.managerIds.map((id) => (
          <div key={id}>
            <Manager />
          </div>
        ))}
      </>
    );
  }
  render(<Harness />);
  act(() => screen.getByRole("button", { name: "Display only" }).click());
  expect(screen.getByText("Retained owner")).toBeVisible();
  expect(disposed).not.toHaveBeenCalled();
});
it("sorts group rows without changing source data and ignores redundant/invalid moves", () => {
  const { repo, storage } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  act(() => {
    result.current.setHideExhausted(false);
    result.current.setGroupSort("workbuddy", "balanceDesc");
    result.current.reorderGroup("workbuddy", "workbuddy");
  });
  expect(
    result.current.orderedGroups
      .find((group) => group.id === "workbuddy")
      ?.rows.map((row) => row.accountId),
  ).toEqual(["private-account", "zero"]);
  expect(source.presence.data!.accounts!.map((row) => row.id)).toEqual([
    "private-account",
    "zero",
    "mm",
  ]);
  expect(storage.setItem).toHaveBeenCalledTimes(2);
});
it("keeps cached zero rows unknown when owner reads fail and retains the failed group", () => {
  source.presence.data!.unavailable = ["workbuddy"];
  const { repo } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  const group = result.current.orderedGroups.find(
    (group) => group.id === "workbuddy",
  )!;
  expect(group.rows).toHaveLength(2);
  expect(group.rows.every((row) => row.availability === "unknown")).toBe(true);
  expect(group.freshness).toBe("stale");
});
it("refresh is single-flight over both original query owners and clears waiting after rejection", async () => {
  let resolve!: () => void;
  source.presence.refetch.mockImplementation(
    () =>
      new Promise<void>((done) => {
        resolve = done;
      }),
  );
  source.service.reload.mockRejectedValue(new Error("synthetic read failure"));
  const { repo } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  let waiting!: Promise<void>;
  act(() => {
    waiting = result.current.refresh();
    void result.current.refresh();
  });
  expect(result.current.refreshing).toBe(true);
  expect(source.presence.refetch).toHaveBeenCalledTimes(1);
  expect(source.service.reload).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve();
    await waiting;
  });
  expect(result.current.refreshing).toBe(false);
  expect(result.current.refreshReadFailed).toBe(true);
});

it.each(["presence", "service", "both"])(
  "reports fulfilled QueryObserverResult errors from %s and settles the spinner",
  async (failedOwner) => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const query = (owner: "presence" | "service") =>
      new QueryObserver(client, {
        queryKey: ["overview-refresh-contract", owner],
        queryFn: async () => {
          if (failedOwner === owner || failedOwner === "both") {
            throw new Error("synthetic read failure");
          }
          return { accounts: [] };
        },
      });
    const presence = query("presence");
    const service = query("service");
    source.presence.refetch.mockImplementation(() => presence.refetch());
    source.service.reload.mockImplementation(() => service.refetch());
    const { repo } = repository();
    const { result, unmount } = renderHook(() =>
      useAccountsOverviewViewModel({ preferencesRepository: repo }),
    );
    try {
      let pending!: Promise<void>;
      act(() => {
        pending = result.current.refresh();
      });
      expect(result.current.refreshing).toBe(true);
      await act(async () => {
        await pending;
      });
      expect(result.current.refreshing).toBe(false);
      expect(result.current.refreshReadFailed).toBe(true);
      // These are actual TanStack results: the read errors resolve instead of reject.
      expect(
        failedOwner === "service"
          ? service.getCurrentResult().isError
          : presence.getCurrentResult().isError,
      ).toBe(true);
      expect(source.presence.refetch).toHaveBeenCalledTimes(1);
      expect(source.service.reload).toHaveBeenCalledTimes(1);
      source.presence.refetch.mockResolvedValue({
        isError: false,
        error: null,
      });
      source.service.reload.mockResolvedValue({ isError: false, error: null });
      await act(async () => {
        await result.current.refresh();
      });
      expect(result.current.refreshReadFailed).toBe(false);
      expect(result.current.refreshing).toBe(false);
    } finally {
      unmount();
      client.clear();
    }
  },
);

it("settles presentation refresh while the retained page is hidden", async () => {
  let resolve!: () => void;
  source.presence.refetch.mockImplementation(
    () =>
      new Promise<void>((done) => {
        resolve = done;
      }),
  );
  const { repo } = repository();
  let current!: ReturnType<typeof useAccountsOverviewViewModel>;
  function Probe() {
    current = useAccountsOverviewViewModel({ preferencesRepository: repo });
    return <span>{current.refreshing ? "Refreshing" : "Ready"}</span>;
  }
  const { rerender } = render(
    <PreservedView active>
      <Probe />
    </PreservedView>,
  );
  let pending!: Promise<void>;
  await act(async () => {
    pending = current.refresh();
    await Promise.resolve();
  });
  expect(screen.getByText("Refreshing")).toBeVisible();
  rerender(
    <PreservedView active={false}>
      <Probe />
    </PreservedView>,
  );
  await act(async () => {
    resolve();
    await pending;
  });
  rerender(
    <PreservedView active>
      <Probe />
    </PreservedView>,
  );
  expect(screen.getByText("Ready")).toBeVisible();
});
it("details resolve the exact current row target, while display controls dispatch no account operation", () => {
  const onOpenDetails = vi.fn();
  const { repo } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({
      preferencesRepository: repo,
      onOpenDetails,
    }),
  );
  const row = result.current.orderedGroups.find(
    (group) => group.id === "workbuddy",
  )!.rows[0];
  act(() => result.current.openDetails(row.rowKey));
  expect(onOpenDetails).toHaveBeenCalledWith({
    platform: "workbuddy",
    id: "private-account",
  });
  expect(source.invoke).not.toHaveBeenCalled();
});
it("de-duplicates only the original WorkBuddy HTTP shadow and converts OpenCode once", () => {
  source.presence.data!.accounts!.push({
    source: "workbuddy-inference",
    platform: "workbuddy",
    id: "private-account",
    label: "HTTP shadow",
    inferenceState: "paused",
  });
  source.service.accounts = [
    {
      kind: "vendor",
      id: 42,
      row: {
        vendorId: "opencode",
        accountLabel: "OpenCode",
        vendorName: "OpenCode",
      },
      apps: new Map(),
    },
  ] as ServiceAccount[];
  const { repo } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  expect(
    result.current.scopeAccounts.filter(
      (row) => row.source === "workbuddy-inference",
    ),
  ).toHaveLength(0);
  expect(
    result.current.orderedGroups.find((group) => group.id === "opencode")?.rows,
  ).toHaveLength(1);
});

it("StrictMode never persists twice for one explicit presentation move", () => {
  const { repo, storage } = repository();
  const { result } = renderHook(
    () => useAccountsOverviewViewModel({ preferencesRepository: repo }),
    { wrapper: ({ children }) => <StrictMode>{children}</StrictMode> },
  );
  expect(storage.setItem).not.toHaveBeenCalled();
  act(() => result.current.reorderGroup("workbuddy", "minimax_code"));
  expect(storage.setItem).toHaveBeenCalledTimes(1);
});
it("expiry presentation advances automatically without dispatching a refresh or native operation", () => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  const expiring = account("expiring", 20);
  Object.assign(expiring.snapshot!.value, {
    creditExpiryStatus: "known",
    nextKnownCreditExpiry: {
      expiresAtMs: now + 1000,
      remaining: 20,
      unit: "workbuddy:credits",
    },
  });
  source.presence.data = {
    accounts: [expiring],
    platforms: ["workbuddy"],
    unavailable: [],
    connections: [],
  };
  const { repo } = repository();
  const { result } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  expect(result.current.orderedGroups[0].rows[0].earliestKnownExpiryMs).toBe(
    now + 1000,
  );
  act(() => vi.advanceTimersByTime(1001));
  expect(
    result.current.orderedGroups[0].rows[0].earliestKnownExpiryMs,
  ).toBeNull();
  expect(result.current.orderedGroups[0].rows[0].nextExpiry).toBe(
    "到期数据待更新",
  );
  expect(source.presence.refetch).not.toHaveBeenCalled();
  expect(source.invoke).not.toHaveBeenCalled();
});
it("does not open a removed row from an older rendered click target", () => {
  const onOpenDetails = vi.fn();
  const { repo } = repository();
  const { result, rerender } = renderHook(() =>
    useAccountsOverviewViewModel({
      preferencesRepository: repo,
      onOpenDetails,
    }),
  );
  const old = result.current.orderedGroups.find(
    (group) => group.id === "workbuddy",
  )!.rows[0].rowKey;
  source.presence.data = {
    accounts: [mm],
    platforms: ["minimax_code"],
    unavailable: [],
    connections: [],
  };
  rerender();
  act(() => result.current.openDetails(old));
  expect(onOpenDetails).not.toHaveBeenCalled();
});
it("does not clear a failed group's filter, but resets it after a successful owner read confirms removal", () => {
  const { repo, storage } = repository();
  const { result, rerender } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  act(() => result.current.setFilter("workbuddy"));
  source.presence.data = {
    accounts: [mm],
    platforms: ["minimax_code"],
    unavailable: ["workbuddy"],
    connections: [],
  };
  rerender();
  expect(result.current.selectedFilter).toBe("workbuddy");
  source.presence.data = { ...source.presence.data, unavailable: [] };
  rerender();
  expect(result.current.selectedFilter).toBeUndefined();
  expect(storage.setItem).not.toHaveBeenCalled();
});

it("keeps an OpenCode filter unknown while its separate original query is pending", () => {
  source.service.accounts = [
    {
      kind: "vendor",
      id: 42,
      row: {
        vendorId: "opencode",
        accountLabel: "OpenCode",
        vendorName: "OpenCode",
      },
      apps: new Map(),
    },
  ] as ServiceAccount[];
  const { repo } = repository();
  const { result, rerender } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  act(() => result.current.setFilter("opencode"));
  source.service.accounts = [];
  source.service.isPending = true;
  rerender();
  expect(result.current.selectedFilter).toBe("opencode");
  expect(
    result.current.orderedGroups.find((group) => group.id === "opencode")
      ?.freshness,
  ).toBe("unknown");
  source.service.isPending = false;
  rerender();
  expect(result.current.selectedFilter).toBeUndefined();
});

it("cancels the old expiry clock when the original query account changes", () => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  const expiring = {
    ...account("old", 20),
    snapshot: {
      kind: "workbuddy",
      value: {
        credits: { totalRemaining: 20 },
        creditExpiryStatus: "known",
        nextKnownCreditExpiry: {
          expiresAtMs: now + 1000,
          remaining: 42,
          unit: "points",
        },
      },
    },
  } as PlatformAccountSummary;
  source.presence.data = {
    ...source.presence.data!,
    accounts: [expiring],
    platforms: ["workbuddy"],
  };
  const { repo } = repository();
  const { result, rerender, unmount } = renderHook(() =>
    useAccountsOverviewViewModel({ preferencesRepository: repo }),
  );
  expect(result.current.orderedGroups[0].rows[0].nextExpiry).toMatch(
    /剩余 42 积分/,
  );
  source.presence.data = {
    ...source.presence.data,
    accounts: [account("new", 20)],
  };
  rerender();
  expect(vi.getTimerCount()).toBe(0);
  act(() => vi.advanceTimersByTime(1001));
  expect(result.current.orderedGroups[0].rows[0].nextExpiry).toBe(
    "到期时间未提供",
  );
  expect(source.presence.refetch).not.toHaveBeenCalled();
  unmount();
});
