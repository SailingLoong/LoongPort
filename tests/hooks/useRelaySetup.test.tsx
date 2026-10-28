import React from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import wire from "../fixtures/relay-setup-wire.json";
import {
  useRelaySetup,
  relaySetupOperationKey,
} from "@/components/relay/onboarding/useRelaySetup";
const api = vi.hoisted(() => ({
  discoverTiers: vi.fn(),
  prepareApplicationConnection: vi.fn(),
  applyPreparedTier: vi.fn(),
  prepareTiers: vi.fn(),
  getSetupOperation: vi.fn(),
  reconcileSetupOperation: vi.fn(),
  refresh: vi.fn(),
  switchTier: vi.fn(),
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  api.discoverTiers.mockResolvedValue(wire.discovery);
  api.prepareTiers.mockResolvedValue(wire.receipt);
  api.getSetupOperation.mockResolvedValue(null);
  api.reconcileSetupOperation.mockResolvedValue(wire.receipt);
});
function renderSetup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return renderHook(({ relayId }) => useRelaySetup(relayId), {
    initialProps: { relayId: 7 },
    wrapper: ({ children }) =>
      React.createElement(QueryClientProvider, { client }, children),
  });
}
async function choose(result: ReturnType<typeof renderSetup>["result"]) {
  await waitFor(() => expect(result.current.discovery.data).toBeDefined());
  act(() => result.current.setDraft({ codex: "a" }));
  act(() => result.current.reviewSelection());
}
it("loads remote cold choices without defaulting the first group or preparing anything", async () => {
  const { result } = renderSetup();
  await waitFor(() =>
    expect(result.current.discovery.data?.choices).toHaveLength(3),
  );
  expect(result.current.draft).toEqual({});
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(api.refresh).not.toHaveBeenCalled();
});
it("persists an operation reference before explicit confirmation sends only selected apps", async () => {
  const { result } = renderSetup();
  await choose(result);
  expect(api.prepareTiers).not.toHaveBeenCalled();
  api.prepareTiers.mockImplementation(async (request) => {
    expect(localStorage.getItem(relaySetupOperationKey(7))).toBe(
      request.operationId,
    );
    return { ...wire.receipt, operationId: request.operationId };
  });
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
  expect(api.prepareTiers.mock.calls[0][0]).toMatchObject({
    kind: "start",
    relayId: 7,
    discoveryRevision: "revision-1",
    selections: [{ app: "codex", groupId: "a" }],
  });
  expect(api.switchTier).not.toHaveBeenCalled();
});
it("queries an ambiguous original operation instead of repeating preparation", async () => {
  const { result } = renderSetup();
  await choose(result);
  api.prepareTiers.mockRejectedValueOnce(new Error("fixture IPC lost"));
  api.getSetupOperation.mockImplementation(async (_relayId, operationId) => ({
    ...wire.receipt,
    operationId,
    groups: [{ ...wire.receipt.groups[0], state: "unknown" }],
    status: "needsReconcile",
  }));
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
  expect(api.getSetupOperation).toHaveBeenCalledWith(
    7,
    api.prepareTiers.mock.calls[0][0].operationId,
  );
  expect(result.current.receipt?.groups[0].state).toBe("unknown");
  act(() => result.current.newDraft());
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
});
it("rehydrates a saved receipt through query only, without preparing or switching", async () => {
  localStorage.setItem(relaySetupOperationKey(7), wire.receipt.operationId);
  api.getSetupOperation.mockResolvedValue(wire.receipt);
  const { result } = renderSetup();
  await waitFor(() =>
    expect(result.current.receipt?.operationId).toBe(wire.receipt.operationId),
  );
  expect(api.getSetupOperation).toHaveBeenCalledTimes(1);
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(api.switchTier).not.toHaveBeenCalled();
});
it("never turns an empty selection into a completed operation", async () => {
  const { result } = renderSetup();
  await waitFor(() => expect(result.current.discovery.data).toBeDefined());
  act(() => result.current.reviewSelection());
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBeNull();
});

it("recovers the account's pending operation after all client UUID storage is lost", async () => {
  api.getSetupOperation.mockResolvedValue({
    ...wire.receipt,
    status: "needsReconcile",
    groups: [{ ...wire.receipt.groups[0], state: "unknown" }],
  });
  const { result } = renderSetup();
  await waitFor(() =>
    expect(result.current.receipt?.operationId).toBe(wire.receipt.operationId),
  );
  expect(api.getSetupOperation).toHaveBeenCalledWith(7);
  expect(api.prepareTiers).not.toHaveBeenCalled();
  act(() => result.current.newDraft());
  expect(result.current.phase).toBe("result");
});
it("does not start while the account recovery query is unresolved", async () => {
  api.getSetupOperation.mockImplementation(() => new Promise(() => {}));
  const { result } = renderSetup();
  await choose(result);
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).not.toHaveBeenCalled();
});
it("does not replay a double confirmation or a failed reconciliation", async () => {
  const { result } = renderSetup();
  await choose(result);
  let resolve!: (value: unknown) => void;
  api.prepareTiers.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  let first!: Promise<void>;
  act(() => {
    first = result.current.confirmSelection();
  });
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve({
      ...wire.receipt,
      status: "needsReconcile",
      groups: [{ ...wire.receipt.groups[0], state: "unknown" }],
    });
    await first;
  });
  api.reconcileSetupOperation.mockRejectedValue(new Error("read failed"));
  await act(() => result.current.reconcile());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
  expect(result.current.receipt?.groups[0].state).toBe("unknown");
});

it.each(["reject", "resolve", "pending"])(
  "isolates a new account from an old %s preparation",
  async (outcome) => {
    const { result, rerender } = renderSetup();
    await choose(result);
    let resolve!: (value: unknown) => void;
    let reject!: (reason: unknown) => void;
    api.prepareTiers.mockImplementationOnce(
      () =>
        new Promise((yes, no) => {
          resolve = yes;
          reject = no;
        }),
    );
    let old!: Promise<void>;
    act(() => {
      old = result.current.confirmSelection();
    });
    expect(api.prepareTiers).toHaveBeenCalledTimes(1);
    api.discoverTiers.mockResolvedValue({
      ...wire.discovery,
      relayId: 8,
      account: { ...wire.discovery.account, identity: "fixture/account8" },
    });
    rerender({ relayId: 8 });
    await waitFor(() => expect(result.current.discovery.data?.relayId).toBe(8));
    if (outcome !== "pending")
      await act(async () => {
        if (outcome === "reject") reject(new Error("old A error"));
        else resolve(wire.receipt);
        await old;
      });
    act(() => result.current.setDraft({ codex: "a" }));
    act(() => result.current.reviewSelection());
    api.prepareTiers.mockImplementationOnce(async (request) => ({
      ...wire.receipt,
      relayId: 8,
      operationId: request.operationId,
      accountIdentity: "fixture/account8",
    }));
    await act(() => result.current.confirmSelection());
    expect(api.prepareTiers).toHaveBeenCalledTimes(2);
    expect(api.prepareTiers.mock.calls[1][0].relayId).toBe(8);
    expect(result.current.receipt?.relayId).toBe(8);
  },
);

it("does not let a delayed recovery snapshot downgrade a newer preparation receipt", async () => {
  const { result } = renderSetup();
  await choose(result);
  let oldResolve!: (value: unknown) => void;
  api.getSetupOperation.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        oldResolve = resolve;
      }),
  );
  let oldRead!: Promise<unknown>;
  act(() => {
    oldRead = result.current.recovery.refetch();
  });
  let operationId = "";
  api.prepareTiers.mockImplementationOnce(async (request) => {
    operationId = request.operationId;
    return {
      ...wire.receipt,
      operationId,
      receiptRevision: 5,
      status: "settled",
      targets: [{ ...wire.receipt.targets[0], configuration: "prepared" }],
    };
  });
  await act(() => result.current.confirmSelection());
  expect(result.current.receipt?.receiptRevision).toBe(5);
  await act(async () => {
    oldResolve({ ...wire.receipt, operationId, receiptRevision: 1 });
    await oldRead;
  });
  expect(result.current.receipt?.receiptRevision).toBe(5);
  expect(result.current.receipt?.targets[0].configuration).toBe("prepared");
});

it("allows review again after a matching typed preflight rejection, without inferring it from null", async () => {
  const { result } = renderSetup();
  await choose(result);
  api.prepareTiers.mockImplementationOnce(async (request) => {
    throw {
      schemaVersion: 1,
      outcome: "rejectedBeforeOperation",
      operationId: request.operationId,
      reasonCode: "selectionChanged",
    };
  });
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
  expect(result.current.needsReconciliation).toBe(false);
  expect(result.current.phase).toBe("selection");
  act(() => result.current.reviewSelection());
  api.prepareTiers.mockImplementationOnce(async (request) => ({
    ...wire.receipt,
    operationId: request.operationId,
  }));
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(2);
});
it("does not trust a rejected-before-operation object for a different operation", async () => {
  const { result } = renderSetup();
  await choose(result);
  api.prepareTiers.mockRejectedValueOnce({
    schemaVersion: 1,
    outcome: "rejectedBeforeOperation",
    operationId: "another-operation",
    reasonCode: "selectionChanged",
  });
  await act(() => result.current.confirmSelection());
  expect(result.current.needsReconciliation).toBe(true);
  act(() => result.current.newDraft());
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).toHaveBeenCalledTimes(1);
});

it("treats cached settled data as display-only until this owner's native recovery finishes", async () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: Infinity },
    },
  });
  client.setQueryData(["relay-setup-recovery", 7], {
    receipt: null,
    version: 0,
  });
  let resolve!: (value: unknown) => void;
  api.getSetupOperation.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { result } = renderHook(() => useRelaySetup(7), {
    wrapper: ({ children }) =>
      React.createElement(QueryClientProvider, { client }, children),
  });
  await waitFor(() => expect(result.current.discovery.data).toBeDefined());
  act(() => result.current.setDraft({ codex: "a" }));
  act(() => result.current.reviewSelection());
  await act(() => result.current.confirmSelection());
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(api.getSetupOperation).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve({
      ...wire.receipt,
      status: "needsReconcile",
      groups: [{ ...wire.receipt.groups[0], state: "unknown" }],
    });
  });
  await waitFor(() =>
    expect(result.current.receipt?.groups[0].state).toBe("unknown"),
  );
});

const preparedReceipt = () => ({
  ...wire.receipt,
  status: "settled",
  receiptRevision: 8,
  groups: [{ ...wire.receipt.groups[0], state: "ready" }],
  targets: [
    {
      ...wire.receipt.targets[0],
      configuration: "prepared",
      connection: { state: "notRequested" },
    },
  ],
});
it("only an explicit second confirmation publishes the reviewed prepared app", async () => {
  const ready = preparedReceipt();
  api.getSetupOperation.mockResolvedValue(ready);
  const { result } = renderSetup();
  await waitFor(() => expect(result.current.receipt?.receiptRevision).toBe(8));
  expect(api.prepareApplicationConnection).not.toHaveBeenCalled();
  expect(api.applyPreparedTier).not.toHaveBeenCalled();
  const reviewed = {
    ...ready,
    receiptRevision: 9,
    targets: [
      {
        ...ready.targets[0],
        connection: { state: "confirmationRequired", requestId: "routing-1" },
      },
    ],
  };
  api.prepareApplicationConnection.mockResolvedValue({
    receipt: reviewed,
    confirmation: null,
  });
  await act(() => result.current.reviewConnection("codex"));
  expect(api.prepareApplicationConnection).toHaveBeenCalledWith(
    ready.operationId,
    "codex",
    8,
    undefined,
  );
  expect(api.applyPreparedTier).not.toHaveBeenCalled();
  api.applyPreparedTier.mockResolvedValue({
    receipt: {
      ...reviewed,
      receiptRevision: 10,
      targets: [
        {
          ...reviewed.targets[0],
          connection: {
            state: "applied",
            writeAcknowledged: true,
            observedCurrent: true,
          },
        },
      ],
    },
    result: { status: "switched" },
  });
  await act(() => result.current.confirmConnection(false));
  expect(api.applyPreparedTier).toHaveBeenCalledWith(
    ready.operationId,
    "codex",
    9,
    false,
  );
  expect(result.current.receipt?.targets[0].connection.state).toBe("applied");
  expect(api.switchTier).not.toHaveBeenCalled();
});
it("an unknown application reply queries the same attempt and never replaces it", async () => {
  const ready = preparedReceipt();
  api.getSetupOperation.mockResolvedValue(ready);
  const { result } = renderSetup();
  await waitFor(() => expect(result.current.receipt?.receiptRevision).toBe(8));
  const reviewed = {
    ...ready,
    receiptRevision: 9,
    targets: [
      {
        ...ready.targets[0],
        connection: { state: "confirmationRequired", requestId: "routing-old" },
      },
    ],
  };
  api.prepareApplicationConnection.mockResolvedValue({
    receipt: reviewed,
    confirmation: null,
  });
  await act(() => result.current.reviewConnection("codex"));
  api.applyPreparedTier.mockRejectedValue(new Error("lost reply"));
  api.getSetupOperation.mockResolvedValue({
    ...reviewed,
    receiptRevision: 10,
    status: "needsReconcile",
    targets: [
      {
        ...reviewed.targets[0],
        connection: { state: "unknown", requestId: "routing-old" },
      },
    ],
  });
  await act(() => result.current.confirmConnection(false));
  await act(() => result.current.reviewConnection("codex"));
  await act(() => result.current.confirmConnection(true));
  expect(api.applyPreparedTier).toHaveBeenCalledTimes(1);
  expect(api.prepareApplicationConnection).toHaveBeenCalledTimes(1);
  expect(api.getSetupOperation).toHaveBeenLastCalledWith(7, ready.operationId);
  expect(result.current.receipt?.targets[0].connection.requestId).toBe(
    "routing-old",
  );
});

it("invalidates the displayed application review when reconciliation observes another window's request", async () => {
  const ready = preparedReceipt();
  api.getSetupOperation.mockResolvedValue(ready);
  const { result } = renderSetup();
  await waitFor(() => expect(result.current.receipt?.receiptRevision).toBe(8));
  const reviewed = {
    ...ready,
    receiptRevision: 9,
    targets: [
      {
        ...ready.targets[0],
        connection: {
          state: "confirmationRequired",
          requestId: "review-window-a",
        },
      },
    ],
  };
  api.prepareApplicationConnection.mockResolvedValue({
    receipt: reviewed,
    confirmation: null,
  });
  await act(() => result.current.reviewConnection("codex"));
  api.reconcileSetupOperation.mockResolvedValue({
    ...reviewed,
    receiptRevision: 10,
    targets: [
      {
        ...reviewed.targets[0],
        connection: {
          state: "confirmationRequired",
          requestId: "review-window-b",
        },
      },
    ],
  });
  api.applyPreparedTier.mockResolvedValue({ receipt: reviewed, result: null });
  await act(() => result.current.reconcile());
  await act(() => result.current.confirmConnection(false));
  expect(api.applyPreparedTier).not.toHaveBeenCalled();
  expect(result.current.connectionReview).toBeNull();
});
