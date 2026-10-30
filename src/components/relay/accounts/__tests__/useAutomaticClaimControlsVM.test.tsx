import { useState } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import {
  useAutomaticClaimControlsVM,
  type AutomaticClaimControlsInput,
} from "../useAutomaticClaimControlsVM";
import type { PlatformAccountSummary } from "../useAccountPlatformPresence";
const api = vi.hoisted(() => ({ set: vi.fn(), zread: vi.fn(), zset: vi.fn() }));
vi.mock("@/lib/api/zcodeClaim", () => ({
  zcodeClaimApi: { state: api.zread, setAuto: api.zset },
}));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: { setAutomatic: api.set },
}));
const account = {
  id: "a",
  label: "Synthetic",
  source: "workbuddy",
  platform: "workbuddy",
  snapshot: {
    kind: "workbuddy",
    value: {
      id: "a",
      generation: 2,
      autoClaimRevision: 3,
      autoClaimPolicyVersion: 1,
      autoClaimEnabled: false,
      canSetAutomatic: true,
    },
  },
} as PlatformAccountSummary;
beforeEach(() => {
  vi.resetAllMocks();
  api.set.mockImplementation(async (value, enabled) => ({
    ...value,
    autoClaimEnabled: enabled,
    autoClaimRevision: value.autoClaimRevision + 1,
  }));
});
it("keeps unknown policy pending and explicit uncertain close available", () => {
  const client = new QueryClient();
  const props = {
    accounts: [account],
    scope: "all",
    unavailable: new Set<string>(),
    disabled: false,
    onRefresh: vi.fn(),
    onBusyChange: vi.fn(),
    unconfirmed: { "workbuddy::a": "[2,3,1]" },
    onUnconfirmed: vi.fn(),
  };
  const { result } = renderHook(() => useAutomaticClaimControlsVM(props), {
    wrapper: ({ children }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
  expect(result.current.status).toBe("claimSettings.pending");
  expect(result.current.enableDisabled).toBe(true);
  expect(result.current.disableDisabled).toBe(false);
  expect(result.current.rows[0]).toMatchObject({
    message: "claimSettings.uncertain",
    alert: true,
  });
});
it("does not show completion from an old scope after navigating away and back", async () => {
  const client = new QueryClient();
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const props: AutomaticClaimControlsInput = {
    accounts: [account],
    scope: "all",
    unavailable: new Set(),
    disabled: false,
    onRefresh: async () => ({ data: { accounts: [account], unavailable: [] } }),
    onBusyChange: vi.fn(),
    unconfirmed: {},
    onUnconfirmed: vi.fn(),
  };
  const { result, rerender } = renderHook(
    (input: AutomaticClaimControlsInput) => useAutomaticClaimControlsVM(input),
    {
      initialProps: props,
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  let operation!: Promise<unknown>;
  act(() => {
    operation = result.current.enable();
  });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalled());
  rerender({ ...props, scope: "other", accounts: [] });
  rerender(props);
  await act(async () => {
    resolve({
      ...account.snapshot!.value,
      autoClaimEnabled: true,
      autoClaimRevision: 4,
    });
    await operation;
  });
  expect(result.current.completed).toBeNull();
  expect(result.current.busy).toBe(false);
});

function renderControls(overrides: Partial<AutomaticClaimControlsInput> = {}) {
  const client = new QueryClient();
  const props: AutomaticClaimControlsInput = {
    accounts: [account],
    scope: "all",
    unavailable: new Set(),
    disabled: false,
    onRefresh: async () => ({ data: { accounts: [account], unavailable: [] } }),
    onBusyChange: vi.fn(),
    unconfirmed: {},
    onUnconfirmed: vi.fn(),
    ...overrides,
  };
  return {
    props,
    ...renderHook(
      (input: AutomaticClaimControlsInput) => {
        const [issues, setIssues] = useState<Record<string, string>>({});
        return useAutomaticClaimControlsVM({
          ...input,
          unconfirmed: issues,
          onUnconfirmed: (next) => {
            input.onUnconfirmed(next);
            setIssues(next);
          },
        });
      },
      {
        initialProps: props,
        wrapper: ({ children }) => (
          <QueryClientProvider client={client}>{children}</QueryClientProvider>
        ),
      },
    ),
  };
}

it("keeps a proven not-dispatched operation blocked but retryable, not uncertain", async () => {
  const refresh = vi
    .fn()
    .mockRejectedValue(new Error("Synthetic read failure"));
  const { result, props } = renderControls({ onRefresh: refresh });
  await act(async () => {
    await result.current.enable();
  });
  expect(api.set).not.toHaveBeenCalled();
  expect(props.onUnconfirmed).toHaveBeenCalledWith({});
  expect(result.current.rows[0]).toMatchObject({
    uncertain: false,
    blocked: true,
    message: "claimSettings.readFailed",
  });
  expect(result.current.enableDisabled).toBe(false);
  expect(result.current.completed).toBeNull();
  refresh.mockResolvedValue({ data: { accounts: [account], unavailable: [] } });
  await act(async () => {
    await result.current.enable();
  });
  expect(api.set).toHaveBeenCalledTimes(1);
  expect(result.current.rows[0].blocked).toBe(false);
  expect(result.current.completed?.count).toBe(1);
});

it("does not display an old completion for a replacement owner with the same account key", async () => {
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const { result, props, rerender } = renderControls();
  let operation!: Promise<unknown>;
  act(() => {
    operation = result.current.enable();
  });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalledTimes(1));
  const replacement = {
    ...account,
    snapshot: {
      ...account.snapshot!,
      value: { ...account.snapshot!.value, generation: 9 },
    },
  } as PlatformAccountSummary;
  rerender({ ...props, accounts: [replacement] });
  await act(async () => {
    resolve({
      ...account.snapshot!.value,
      autoClaimEnabled: true,
      autoClaimRevision: 4,
    });
    await operation;
  });
  expect(result.current.completed).toBeNull();
  expect(result.current.busy).toBe(false);
});

it("preserves confirmed completion when only the same owner's policy revision advances", async () => {
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const { result, props, rerender } = renderControls();
  let operation!: Promise<unknown>;
  act(() => {
    operation = result.current.enable();
  });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalledTimes(1));
  const updated = {
    ...account,
    snapshot: {
      ...account.snapshot!,
      value: {
        ...account.snapshot!.value,
        autoClaimEnabled: true,
        autoClaimRevision: 4,
      },
    },
  } as PlatformAccountSummary;
  rerender({ ...props, accounts: [updated] });
  await act(async () => {
    resolve(updated.snapshot!.value);
    await operation;
  });
  expect(result.current.completed?.count).toBe(1);
});

it("keeps a lost write response uncertain and never automatically replays it", async () => {
  api.set.mockRejectedValue(new Error("Synthetic lost response"));
  const { result } = renderControls();
  await act(async () => {
    await result.current.enable();
  });
  expect(result.current.rows[0].uncertain).toBe(true);
  expect(result.current.enableDisabled).toBe(true);
  expect(result.current.disableDisabled).toBe(false);
  await act(async () => {
    await result.current.enable();
  });
  expect(api.set).toHaveBeenCalledTimes(1);
});

it("does not invalidate a supported owner's completion when an unsupported row is refreshed", async () => {
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const unsupported: PlatformAccountSummary = {
    id: "unsupported",
    label: "Synthetic",
    source: "auth",
    platform: "codex_oauth",
  };
  const { result, props, rerender } = renderControls({
    accounts: [account, unsupported],
  });
  let operation!: Promise<unknown>;
  act(() => {
    operation = result.current.enable();
  });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalledTimes(1));
  rerender({ ...props, accounts: [account, { ...unsupported }] });
  await act(async () => {
    resolve({
      ...account.snapshot!.value,
      autoClaimEnabled: true,
      autoClaimRevision: 4,
    });
    await operation;
  });
  expect(result.current.completed?.count).toBe(1);
});

it("refuses same-turn enable replay before uncertain props have rerendered", async () => {
  api.set.mockRejectedValue(new Error("Synthetic lost response"));
  const { result } = renderControls();
  await act(async () => {
    await result.current.enable();
    await result.current.enable();
  });
  expect(api.set).toHaveBeenCalledTimes(1);
  expect(result.current.rows[0].uncertain).toBe(true);
});

it("does not publish completion after unmount but releases the original busy owner", async () => {
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const { result, props, unmount } = renderControls();
  let operation!: Promise<unknown>;
  act(() => {
    operation = result.current.enable();
  });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalledTimes(1));
  unmount();
  await act(async () => {
    resolve({
      ...account.snapshot!.value,
      autoClaimEnabled: true,
      autoClaimRevision: 4,
    });
    await operation;
  });
  expect(props.onBusyChange).toHaveBeenNthCalledWith(1, true);
  expect(props.onBusyChange).toHaveBeenNthCalledWith(2, false);
});

const zaccount = {
  id: "z",
  label: "Synthetic",
  source: "zcode",
  platform: "zcode",
  snapshot: {
    kind: "zcode",
    dataRoot: "synthetic-root",
    value: {
      enabled: false,
      participants: [],
      consentRevision: "context:1",
      busy: false,
      records: {},
    },
  },
} as PlatformAccountSummary;

it.each(["receipt", "foreign-consent", "foreign-root"])(
  "accepts only its own ZCode confirmation after refresh: %s",
  async (change) => {
    let resolve!: (value: unknown) => void;
    api.zread.mockResolvedValue(zaccount.snapshot!.value);
    api.zset.mockImplementation(
      () =>
        new Promise((r) => {
          resolve = r;
        }),
    );
    const receipt = {
      ...zaccount.snapshot!.value,
      enabled: true,
      participants: ["z"],
      consentRevision: "context:2",
    };
    const updated = {
      ...zaccount,
      snapshot: {
        ...zaccount.snapshot!,
        dataRoot:
          change === "foreign-root" ? "replacement-root" : "synthetic-root",
        value: {
          ...receipt,
          consentRevision:
            change === "foreign-consent" ? "context:9" : "context:2",
        },
      },
    } as PlatformAccountSummary;
    const { result, props, rerender } = renderControls({
      accounts: [zaccount],
      onRefresh: async () => ({
        data: { accounts: [zaccount], unavailable: [] },
      }),
    });
    let operation!: Promise<unknown>;
    act(() => {
      operation = result.current.enable();
    });
    await vi.waitFor(() => expect(api.zset).toHaveBeenCalledTimes(1));
    rerender({ ...props, accounts: [updated] });
    await act(async () => {
      resolve(receipt);
      await operation;
    });
    if (change === "receipt") expect(result.current.completed?.count).toBe(1);
    else expect(result.current.completed).toBeNull();
  },
);

it("never resurrects a ZCode completion after a foreign consent token was observed", async () => {
  const receipt = {
    ...zaccount.snapshot!.value,
    enabled: true,
    participants: ["z"],
    consentRevision: "context:2",
  };
  api.zread.mockResolvedValue(zaccount.snapshot!.value);
  api.zset.mockResolvedValue(receipt);
  const { result, props, rerender } = renderControls({
    accounts: [zaccount],
    onRefresh: async () => ({
      data: { accounts: [zaccount], unavailable: [] },
    }),
  });
  await act(async () => {
    await result.current.enable();
  });
  const updated = {
    ...zaccount,
    snapshot: { ...zaccount.snapshot!, value: receipt },
  } as PlatformAccountSummary;
  rerender({ ...props, accounts: [updated] });
  expect(result.current.completed?.count).toBe(1);
  const foreign = {
    ...updated,
    snapshot: {
      ...updated.snapshot!,
      value: { ...receipt, consentRevision: "context:9" },
    },
  } as PlatformAccountSummary;
  rerender({ ...props, accounts: [foreign] });
  expect(result.current.completed).toBeNull();
  rerender({ ...props, accounts: [updated] });
  expect(result.current.completed).toBeNull();
});

it("retains the not-dispatched notice when a refreshed ZCode consent revision replaces the clicked one", async () => {
  const fresh = {
    ...zaccount,
    snapshot: {
      ...zaccount.snapshot!,
      value: { ...zaccount.snapshot!.value, consentRevision: "context:2" },
    },
  } as PlatformAccountSummary;
  const { result, props, rerender } = renderControls({
    accounts: [zaccount],
    onRefresh: async () => ({ data: { accounts: [fresh], unavailable: [] } }),
  });
  await act(async () => {
    await result.current.enable();
  });
  expect(api.zset).not.toHaveBeenCalled();
  expect(result.current.rows[0]).toMatchObject({
    uncertain: false,
    blocked: true,
    message: "claimSettings.notDispatched",
  });
  rerender({ ...props, accounts: [fresh] });
  expect(result.current.rows[0]).toMatchObject({
    uncertain: false,
    blocked: true,
    message: "claimSettings.notDispatched",
  });
  expect(result.current.enableDisabled).toBe(false);
  const otherRoot = {
    ...fresh,
    snapshot: { ...fresh.snapshot!, dataRoot: "replacement-root" },
  } as PlatformAccountSummary;
  rerender({ ...props, accounts: [otherRoot] });
  expect(result.current.rows[0].blocked).toBe(false);
});
