import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { useApplicationOverview } from "../useApplicationOverview";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
const m = vi.hoisted(() => ({
  invoke: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  apply: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: m.invoke }));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { get: async () => ({ configurations: [] }) },
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { apply: m.apply },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { success: m.success, error: m.error, warning: vi.fn() },
}));
const providers = {};
const target = {
  providerId: "vendor-3",
  name: "Account",
  canSelect: true,
  selection: { kind: "vendor", rowId: 3, planId: "standard" },
} as ApplicationConfiguration;
const id = "11fdd126-d40c-452f-8fb1-22597dfd5250";
function receipt(
  operationId = id,
  status = "completed",
  providerId = "vendor-3",
) {
  return {
    operationId,
    rowId: 3,
    plan: "standard",
    app: "codex",
    providerId,
    receiptRevision: 1,
    status,
    operationState: status,
    completedConfigurationRevision: status === "completed" ? "rev-1" : null,
  };
}
let client: QueryClient;
function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
function setup() {
  return renderHook(() => useApplicationOverview("codex", providers, vi.fn()), {
    wrapper,
  });
}
beforeEach(() => {
  vi.resetAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});
it("prepares before writing and verifies the original operation after a switched reply", async () => {
  let operation = "";
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { status: "none" }
        : receipt(operation);
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return {
        status: "switched",
        providerName: "Account",
        chatgptWasRunning: false,
        chatgptRelaunched: false,
        warnings: [],
      };
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(m.invoke.mock.calls.map((c) => c[0])).toEqual([
    "vendor_query_connection",
    "vendor_prepare_connection",
    "vendor_switch",
    "vendor_query_connection",
  ]);
  expect(m.invoke.mock.calls[2][1]).toMatchObject({
    operationId: operation,
    expectedRevision: 1,
  });
  expect(m.success).toHaveBeenCalledTimes(1);
});
it("never repeats apply when the reply is lost and an explicit retry remains unknown", async () => {
  let operation = "";
  let reads = 0;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection") {
      reads++;
      return reads === 1
        ? { status: "none" }
        : { status: "unknown", operationId: operation, app: "codex" };
    }
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch") throw new Error("lost response");
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  await act(async () => {
    await result.current.select(target);
  });
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_prepare_connection"),
  ).toHaveLength(1);
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch"),
  ).toHaveLength(1);
  expect(
    m.invoke.mock.calls
      .filter((c) => c[0] === "vendor_query_connection")
      .at(-1)?.[1].operationId,
  ).toBe(operation);
  expect(m.success).not.toHaveBeenCalled();
});
it("blocks unresolved recovery and does not mistake another provider receipt for success", async () => {
  m.invoke.mockResolvedValue({
    ...receipt(id, "unknown", "foreign"),
    operationState: "applying",
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(m.invoke.mock.calls.map((c) => c[0])).toEqual([
    "vendor_query_connection",
    "vendor_preview_new_connection_intent",
  ]);
  expect(m.success).not.toHaveBeenCalled();
});
it("cancels before prepare if the app recovery read is still pending", async () => {
  let resolve!: (value: unknown) => void;
  m.invoke.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { result } = setup();
  let work!: Promise<void>;
  act(() => {
    work = result.current.select(target);
  });
  act(() => result.current.cancel());
  await act(async () => {
    resolve({ status: "none" });
    await work;
  });
  expect(m.invoke.mock.calls.map((c) => c[0])).toEqual([
    "vendor_query_connection",
  ]);
  expect(m.success).not.toHaveBeenCalled();
});
it("does not apply from a retained callback after switching the visible application", async () => {
  m.invoke.mockResolvedValue({ status: "none" });
  const { result, rerender } = renderHook(
    ({ app }: { app: "codex" | "claude" }) =>
      useApplicationOverview(app, providers, vi.fn()),
    { wrapper, initialProps: { app: "codex" } },
  );
  const oldSelect = result.current.select;
  rerender({ app: "claude" });
  await act(async () => {
    await oldSelect(target);
  });
  expect(m.invoke).not.toHaveBeenCalled();
});
it("keeps a completed receipt with no configuration revision uncertain", async () => {
  let operation = "";
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { status: "none" }
        : { ...receipt(operation), completedConfigurationRevision: null };
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return { status: "unknown", operationId: operation };
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(m.success).not.toHaveBeenCalled();
  expect(m.error).toHaveBeenCalled();
});
it("recovers a lost reply on retry using the same UUID without another write", async () => {
  let operation = "";
  let read = 0;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection") {
      read++;
      return read === 1
        ? { status: "none" }
        : read === 2
          ? { status: "unknown", operationId: operation, app: "codex" }
          : receipt(operation);
    }
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch") throw new Error("lost response");
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  await act(async () => {
    await result.current.select(target);
  });
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch"),
  ).toHaveLength(1);
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_prepare_connection"),
  ).toHaveLength(1);
  expect(m.invoke.mock.calls.at(-1)?.[1].operationId).toBe(operation);
});
it("does not route vendor catalog decisions through generic apply", async () => {
  const { result } = setup();
  await act(async () => {
    await result.current.select(target, undefined, undefined, {
      choice: "manage",
      revision: "rev",
    });
  });
  expect(m.apply).not.toHaveBeenCalled();
  expect(m.invoke).not.toHaveBeenCalled();
  expect(m.error).toHaveBeenCalled();
});
it("does not apply from a retained callback after an application ABA change", async () => {
  m.invoke.mockResolvedValue({ status: "none" });
  const { result, rerender } = renderHook(
    ({ app }: { app: "codex" | "claude" }) =>
      useApplicationOverview(app, providers, vi.fn()),
    { wrapper, initialProps: { app: "codex" } },
  );
  const oldSelect = result.current.select;
  rerender({ app: "claude" });
  rerender({ app: "codex" });
  await act(async () => {
    await oldSelect(target);
  });
  expect(m.invoke).not.toHaveBeenCalled();
});
it("retains the unresolved native operation across a hook remount without a new prepare", async () => {
  m.invoke.mockResolvedValue({
    ...receipt(id, "unknown"),
    operationState: "applying",
  });
  const first = setup();
  first.unmount();
  const next = setup();
  await act(async () => {
    await next.result.current.select(target);
  });
  expect(m.invoke.mock.calls.map((c) => c[0])).toEqual([
    "vendor_query_connection",
    "vendor_preview_new_connection_intent",
  ]);
  expect(m.success).not.toHaveBeenCalled();
});
it("shows a backend quit confirmation and cancellation does not dispatch again", async () => {
  let operation = "";
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection") return { status: "none" };
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return {
        status: "confirmationRequired",
        targetName: "Account",
        quitRequired: true,
      };
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(result.current.confirmation).toBe("Account");
  expect(result.current.quitRequired).toBe(true);
  const staleConfirm = result.current.confirm;
  act(() => result.current.cancel());
  act(() => staleConfirm(true));
  expect(m.error).not.toHaveBeenCalled();
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch"),
  ).toHaveLength(1);
  expect(m.success).not.toHaveBeenCalled();
});
it("queries the durable receipt even when the original switched promise settles after cancel", async () => {
  let operation = "";
  let finish!: (value: unknown) => void;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { status: "none" }
        : receipt(operation);
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return new Promise((done) => {
        finish = done;
      });
  });
  const { result } = setup();
  let work!: Promise<void>;
  await act(async () => {
    work = result.current.select(target);
  });
  act(() => result.current.cancel());
  await act(async () => {
    finish({
      status: "switched",
      providerName: "Account",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
      warnings: [],
    });
    await work;
  });
  expect(m.invoke.mock.calls.at(-1)?.[0]).toBe("vendor_query_connection");
  expect(m.success).not.toHaveBeenCalled();
});
it("continues an explicit quit confirmation with the original prepared UUID and revision", async () => {
  let operation = "";
  let writes = 0;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { status: "none" }
        : receipt(operation);
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch") {
      writes++;
      return writes === 1
        ? {
            status: "confirmationRequired",
            targetName: "Account",
            quitRequired: true,
          }
        : {
            status: "switched",
            providerName: "Account",
            chatgptWasRunning: true,
            chatgptRelaunched: true,
            warnings: [],
          };
    }
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  await act(async () => {
    result.current.confirm(true);
  });
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_prepare_connection"),
  ).toHaveLength(1);
  const switches = m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch");
  expect(switches).toHaveLength(2);
  expect(switches[1][1]).toMatchObject({
    operationId: operation,
    expectedRevision: 1,
    quitChatgpt: true,
  });
  expect(m.success).toHaveBeenCalledTimes(1);
});
it("binds a changed catalog decision to a fresh operation only after reviewing the displayed image", async () => {
  const rev = "a".repeat(64),
    observed = "b".repeat(64),
    later = "c".repeat(64);
  const catalog = {
    ownership: "unclaimedExternal",
    choice: "keep",
    canManage: true,
    canRestore: false,
    revision: rev,
    beforePointer: "/old",
    afterPointer: "/old",
  };
  let operation = "";
  let writes = 0,
    observations = 0;
  const preparedIds: string[] = [];
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { status: "none" }
        : receipt(operation);
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      preparedIds.push(operation);
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch") {
      writes++;
      return writes === 1
        ? {
            status: "confirmationRequired",
            targetName: "Account",
            quitRequired: false,
            catalog,
          }
        : {
            status: "switched",
            providerName: "Account",
            chatgptWasRunning: false,
            chatgptRelaunched: false,
            warnings: [],
          };
    }
    if (command === "vendor_preview_connection_catalog")
      return {
        status: "confirmationRequired",
        targetName: "Account",
        quitRequired: false,
        catalog: { ...catalog, choice: "manage", afterPointer: "/managed" },
      };
    if (command === "vendor_preview_connection_review") {
      observations++;
      return {
        status: "reviewRequired",
        app: "codex",
        operationId: operation,
        receiptRevision: 1,
        observedRevision: observations === 1 ? observed : later,
      };
    }
    if (command === "vendor_review_connection")
      return { ...receipt(operation, "failedBeforeWrite"), receiptRevision: 2 };
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(
    m.invoke.mock.calls.filter(
      (c) => c[0] === "vendor_preview_connection_review",
    ),
  ).toHaveLength(1);
  await act(async () => {
    await result.current.previewCatalog({ choice: "manage", revision: rev });
  });
  expect(preparedIds).toHaveLength(1);
  await act(async () => {
    result.current.confirm(false, { choice: "manage", revision: rev });
  });
  expect(preparedIds).toHaveLength(2);
  expect(preparedIds[0]).not.toBe(preparedIds[1]);
  expect(
    m.invoke.mock.calls.filter(
      (c) => c[0] === "vendor_review_connection",
    )[0]?.[1],
  ).toMatchObject({
    operationId: preparedIds[0],
    expectedRevision: 1,
    observedRevision: later,
  });
  expect(
    m.invoke.mock.calls.filter(
      (c) => c[0] === "vendor_prepare_connection",
    )[1]?.[1].catalogDecision,
  ).toEqual({ choice: "manage", revision: rev });
  expect(m.apply).not.toHaveBeenCalled();
  expect(m.success).toHaveBeenCalledTimes(1);
});
it("retains a catalog prompt after an initial review preview failure so an explicit preview can retry", async () => {
  const rev = "a".repeat(64),
    observed = "b".repeat(64);
  const catalog = {
    ownership: "unclaimedExternal",
    choice: "keep",
    canManage: true,
    canRestore: false,
    revision: rev,
    beforePointer: "/old",
    afterPointer: "/old",
  };
  let operation = "",
    reviews = 0;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection") return { status: "none" };
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (
      command === "vendor_switch" ||
      command === "vendor_preview_connection_catalog"
    )
      return {
        status: "confirmationRequired",
        targetName: "Account",
        quitRequired: false,
        catalog,
      };
    if (command === "vendor_preview_connection_review") {
      reviews++;
      if (reviews === 1) throw new Error("read failed");
      return {
        status: "reviewRequired",
        app: "codex",
        operationId: operation,
        receiptRevision: 1,
        observedRevision: observed,
      };
    }
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(result.current.confirmation).toBe("Account");
  await act(async () => {
    await result.current.previewCatalog({ choice: "keep", revision: rev });
  });
  expect(reviews).toBe(2);
  expect(
    m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch"),
  ).toHaveLength(1);
});
it.each(["rejected", "foreign", "cancelled"])(
  "never prepares the replacement when catalog retirement is %s",
  async (mode) => {
    const rev = "a".repeat(64),
      observed = "b".repeat(64);
    const catalog = {
      ownership: "unclaimedExternal",
      choice: "keep",
      canManage: true,
      canRestore: false,
      revision: rev,
      beforePointer: "/old",
      afterPointer: "/old",
    };
    let operation = "";
    let finish!: (value: unknown) => void;
    m.invoke.mockImplementation(async (command, input) => {
      if (command === "vendor_query_connection") return { status: "none" };
      if (command === "vendor_prepare_connection") {
        operation = input.operationId;
        return receipt(operation, "prepared");
      }
      if (command === "vendor_switch")
        return {
          status: "confirmationRequired",
          targetName: "Account",
          quitRequired: false,
          catalog,
        };
      if (command === "vendor_preview_connection_review")
        return {
          status: "reviewRequired",
          app: "codex",
          operationId: operation,
          receiptRevision: 1,
          observedRevision: observed,
        };
      if (command === "vendor_review_connection") {
        if (mode === "rejected") throw new Error("application changed");
        if (mode === "foreign")
          return {
            ...receipt(operation, "failedBeforeWrite", "foreign"),
            receiptRevision: 2,
          };
        return new Promise((done) => {
          finish = done;
        });
      }
    });
    const { result } = setup();
    await act(async () => {
      await result.current.select(target);
    });
    await act(async () => {
      result.current.confirm(false, { choice: "keep", revision: rev });
    });
    if (mode === "cancelled") {
      act(() => result.current.cancel());
      await act(async () => {
        finish({
          ...receipt(operation, "failedBeforeWrite"),
          receiptRevision: 2,
        });
      });
    }
    expect(
      m.invoke.mock.calls.filter((c) => c[0] === "vendor_prepare_connection"),
    ).toHaveLength(1);
    expect(
      m.invoke.mock.calls.filter((c) => c[0] === "vendor_switch"),
    ).toHaveLength(1);
    expect(m.success).not.toHaveBeenCalled();
  },
);
it("does not let a stale cancel close a newer application confirmation after ABA navigation", async () => {
  let operation = "";
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection") return { status: "none" };
    if (command === "vendor_prepare_connection") {
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return {
        status: "confirmationRequired",
        targetName: "Account",
        quitRequired: true,
      };
  });
  const { result, rerender } = renderHook(
    ({ app }: { app: "codex" | "claude" }) =>
      useApplicationOverview(app, providers, vi.fn()),
    { wrapper, initialProps: { app: "codex" } },
  );
  const staleCancel = result.current.cancel;
  rerender({ app: "claude" });
  rerender({ app: "codex" });
  await act(async () => {
    await result.current.select(target);
  });
  expect(result.current.confirmation).toBe("Account");
  act(() => staleCancel());
  expect(result.current.confirmation).toBe("Account");
});
it("previews an uncertain prior intent and writes a new operation only after explicit review confirmation", async () => {
  let operation = id,
    prepares = 0;
  m.invoke.mockImplementation(async (command, input) => {
    if (command === "vendor_query_connection")
      return input.operationId === null
        ? { ...receipt(id, "unknown"), operationState: "applying" }
        : receipt(operation);
    if (command === "vendor_preview_new_connection_intent")
      return {
        status: "newIntentReviewRequired",
        app: "codex",
        operationId: id,
        receiptRevision: 2,
        originalRowId: 3,
        originalPlan: "standard",
        targetRowId: 3,
        targetPlan: "standard",
        targetProviderId: "vendor-3",
        reviewToken: "a".repeat(64),
      };
    if (command === "vendor_confirm_new_connection_intent")
      return { status: "newIntentPermitted", app: "codex" };
    if (command === "vendor_prepare_connection") {
      prepares++;
      operation = input.operationId;
      return receipt(operation, "prepared");
    }
    if (command === "vendor_switch")
      return {
        status: "switched",
        providerName: "Account",
        chatgptWasRunning: false,
        chatgptRelaunched: false,
        warnings: [],
      };
  });
  const { result } = setup();
  await act(async () => {
    await result.current.select(target);
  });
  expect(result.current.vendorRecovery?.target.name).toBe("Account");
  expect(prepares).toBe(0);
  await act(async () => {
    await result.current.confirmVendorRecovery();
  });
  expect(prepares).toBe(1);
  expect(operation).not.toBe(id);
  expect(m.success).toHaveBeenCalledTimes(1);
  expect(
    m.invoke.mock.calls.filter(
      (c) => c[0] === "vendor_confirm_new_connection_intent",
    )[0]?.[1].reviewToken,
  ).toBe("a".repeat(64));
});
it.each(["cancel", "stale", "duplicate", "rejected", "foreign"])(
  "keeps new-intent approval one-shot and bound when %s",
  async (mode) => {
    let operation = id,
      prepares = 0;
    m.invoke.mockImplementation(async (command, input) => {
      if (command === "vendor_query_connection")
        return input.operationId === null
          ? { ...receipt(id, "unknown"), operationState: "applying" }
          : receipt(operation);
      if (command === "vendor_preview_new_connection_intent")
        return {
          status: "newIntentReviewRequired",
          app: "codex",
          operationId: id,
          receiptRevision: 2,
          originalRowId: 3,
          originalPlan: "standard",
          targetRowId: 3,
          targetPlan: "standard",
          targetProviderId: mode === "foreign" ? "foreign" : "vendor-3",
          reviewToken: "a".repeat(64),
        };
      if (command === "vendor_confirm_new_connection_intent") {
        if (mode === "rejected") throw new Error("changed");
        return { status: "newIntentPermitted", app: "codex" };
      }
      if (command === "vendor_prepare_connection") {
        prepares++;
        operation = input.operationId;
        return receipt(operation, "prepared");
      }
      if (command === "vendor_switch")
        return {
          status: "switched",
          providerName: "Account",
          chatgptWasRunning: false,
          chatgptRelaunched: false,
          warnings: [],
        };
    });
    const { result } = setup();
    await act(async () => {
      await result.current.select(target);
    });
    const oldConfirm = result.current.confirmVendorRecovery;
    if (mode === "cancel" || mode === "stale")
      act(() => result.current.cancelVendorRecovery());
    if (mode === "stale")
      await act(async () => {
        await result.current.select(target);
      });
    await act(async () => {
      await oldConfirm();
      if (mode === "duplicate") await oldConfirm();
    });
    expect(prepares).toBe(mode === "duplicate" ? 1 : 0);
    expect(
      m.invoke.mock.calls.filter(
        (c) => c[0] === "vendor_confirm_new_connection_intent",
      ),
    ).toHaveLength(mode === "duplicate" || mode === "rejected" ? 1 : 0);
  },
);
it.each(["failedBeforeWrite", "reviewed"])(
  "starts an explicit new intent after the native receipt is already retired as %s",
  async (state) => {
    let operation = "";
    let writes = 0;
    m.invoke.mockImplementation(async (command, input) => {
      if (command === "vendor_query_connection")
        return input.operationId === null
          ? receipt(id, state)
          : receipt(operation);
      if (command === "vendor_prepare_connection") {
        operation = input.operationId;
        return receipt(operation, "prepared");
      }
      if (command === "vendor_switch") {
        writes++;
        return {
          status: "switched",
          providerName: "Account",
          chatgptWasRunning: false,
          chatgptRelaunched: false,
          warnings: [],
        };
      }
    });
    const { result } = setup();
    await act(async () => {
      await result.current.select(target);
    });
    expect(writes).toBe(1);
    expect(operation).not.toBe(id);
    expect(
      m.invoke.mock.calls.some(
        (c) => c[0] === "vendor_preview_new_connection_intent",
      ),
    ).toBe(false);
  },
);
