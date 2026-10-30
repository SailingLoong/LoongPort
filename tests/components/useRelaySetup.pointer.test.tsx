import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  cleanup,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  useRelaySetup,
  relaySetupOperationKey,
} from "@/components/relay/onboarding/useRelaySetup";
import { relayApi } from "@/lib/api/relay";
import { PreservedView } from "@/components/ui/PreservedView";
vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    discoverTiers: vi.fn(),
    getSetupOperation: vi.fn(),
    reconcileSetupOperation: vi.fn(),
    prepareTiers: vi.fn(),
  },
}));
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.resetAllMocks();
  vi.restoreAllMocks();
});
const discovery = {
  revision: "d1",
  account: { accountLabel: "A", siteOrigin: "https://example.test" },
  choices: [{ app: "codex", groupId: "group-1", groupName: "G" }],
};
function Stage({ id = 7 }: { id?: number }) {
  const s = useRelaySetup(id);
  return (
    <>
      <span>phase:{s.phase}</span>
      <span>needs:{String(s.needsReconciliation)}</span>
      <span>error:{s.error ?? "none"}</span>
      <span>recovery:{String(s.recovery.isSuccess)}</span>
      <button onClick={() => s.setDraft({ codex: "group-1" })}>choose</button>
      <button onClick={s.reviewSelection}>review</button>
      <button onClick={() => void s.confirmSelection()}>prepare</button>
      <button onClick={() => void s.reconcile()}>reconcile</button>
    </>
  );
}
it("preserves a remembered original operation as uncertain when UUID and account lookup are null", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "op-pending");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  const lookup = vi
    .spyOn(relayApi, "getSetupOperation")
    .mockResolvedValue(null);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  expect(lookup).toHaveBeenCalledWith(7, "op-pending");
  expect(screen.getByText("needs:true")).toBeTruthy();
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
});
it("A→B→A retains the pending UUID and does not start a second prepare on null recovery", async () => {
  const pending = new Promise<any>(() => {});
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  const lookup = vi
    .spyOn(relayApi, "getSetupOperation")
    .mockResolvedValue(null);
  const prepare = vi.spyOn(relayApi, "prepareTiers").mockReturnValue(pending);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = (id: number) => (
    <QueryClientProvider client={client}>
      <Stage id={id} />
    </QueryClientProvider>
  );
  const v = render(view(7));
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).toHaveBeenCalledTimes(1);
  const original = localStorage.getItem(relaySetupOperationKey(7));
  expect(original).toBeTruthy();
  v.rerender(view(8));
  await screen.findByText("recovery:true");
  v.rerender(view(7));
  await screen.findByText("recovery:true");
  expect(lookup).toHaveBeenCalledWith(7, original);
  expect(screen.getByText("needs:true")).toBeTruthy();
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).toHaveBeenCalledTimes(1);
});
it("reconciles the exact original UUID after its journal appears", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "op-pending");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const receipt = {
    relayId: 7,
    operationId: "op-pending",
    receiptRevision: 1,
    status: "settled",
    groups: [],
    targets: [],
    selections: [],
  };
  const reconcile = vi
    .spyOn(relayApi, "reconcileSetupOperation")
    .mockResolvedValue(receipt as any);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("needs:true");
  fireEvent.click(screen.getByText("reconcile"));
  await screen.findByText("needs:false");
  expect(reconcile).toHaveBeenCalledWith("op-pending");
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-pending");
});
it("does not let the stored UUID hide a different account journal operation", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "op-pending");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  const receipt = {
    relayId: 7,
    operationId: "op-pending",
    receiptRevision: 1,
    status: "settled",
    groups: [],
    targets: [],
    selections: [],
  };
  vi.spyOn(relayApi, "getSetupOperation").mockImplementation(
    async (_id, op) =>
      (op ? receipt : { ...receipt, operationId: "op-other" }) as any,
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("error:recoveryConflict");
  expect(screen.getByText("needs:true")).toBeTruthy();
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-pending");
});
it("does not remove a newer pointer when requesting a new draft", async () => {
  const receipt = {
    relayId: 7,
    operationId: "op-old",
    receiptRevision: 1,
    status: "settled",
    groups: [],
    targets: [],
    selections: [],
  };
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(receipt as any);
  function Page() {
    const s = useRelaySetup(7);
    return (
      <>
        <span>phase:{s.phase}</span>
        <span>needs:{String(s.needsReconciliation)}</span>
        <span>error:{s.error ?? "none"}</span>
        <button onClick={s.newDraft}>new</button>
      </>
    );
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Page />
    </QueryClientProvider>,
  );
  await screen.findByText("phase:result");
  localStorage.setItem(relaySetupOperationKey(7), "op-new");
  fireEvent.click(screen.getByText("new"));
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-new");
  expect(screen.getByText("needs:true")).toBeTruthy();
  expect(screen.getByText("error:recoveryConflict")).toBeTruthy();
});
it("fails closed if the pointer cannot be persisted before dispatch", async () => {
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("denied");
  });
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.getByText("error:operationStorageUnavailable")).toBeTruthy();
});
it("a pointer installed after recovery but before prepare blocks a new UUID", async () => {
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  localStorage.setItem(relaySetupOperationKey(7), "op-new");
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.getByText("needs:true")).toBeTruthy();
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-new");
});
it("pre-creation rejection plus same UUID null removes only the matching pointer and allows retry", async () => {
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockImplementationOnce(async (req: any) => {
      throw {
        schemaVersion: 1,
        outcome: "rejectedBeforeOperation",
        operationId: req.operationId,
        reasonCode: "rejected",
      };
    })
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  await screen.findByText("error:rejected");
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBeNull();
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).toHaveBeenCalledTimes(2);
});
it("pre-creation rejection cannot delete a replacement pointer", async () => {
  let reject!: (x: any) => void;
  const pending = new Promise<any>((_, r) => (reject = r));
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const prepare = vi.spyOn(relayApi, "prepareTiers").mockReturnValue(pending);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  const request = prepare.mock.calls[0][0];
  localStorage.setItem(relaySetupOperationKey(7), "op-new");
  await act(async () => {
    reject({
      schemaVersion: 1,
      outcome: "rejectedBeforeOperation",
      operationId: request.operationId,
      reasonCode: "rejected",
    });
    await pending.catch(() => {});
  });
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-new");
  expect(screen.getByText("needs:true")).toBeTruthy();
});
it("an empty corrupt stored pointer cannot authorize a new operation", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(null);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
});
it("a UUID lookup error preserves uncertainty and blocks new preparation", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "op-pending");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockImplementation(
    async (_id, op) => {
      if (op) throw new Error("offline");
      return null;
    },
  );
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await screen.findByText("error:recoveryFailed");
  expect(screen.getByText("needs:true")).toBeTruthy();
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
});
it("a newer pointer written while recovery is in flight is preserved and blocks preparation", async () => {
  localStorage.setItem(relaySetupOperationKey(7), "op-old");
  let resolve!: (value: null) => void;
  const pending = new Promise<null>((r) => (resolve = r));
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockReturnValue(pending);
  const prepare = vi
    .spyOn(relayApi, "prepareTiers")
    .mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Stage />
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(relayApi.getSetupOperation).toHaveBeenCalledTimes(2),
  );
  localStorage.setItem(relaySetupOperationKey(7), "op-new");
  await act(async () => {
    resolve(null);
    await pending;
  });
  await screen.findByText("error:recoveryConflict");
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-new");
  expect(screen.getByText("needs:true")).toBeTruthy();
  fireEvent.click(screen.getByText("prepare"));
  expect(prepare).not.toHaveBeenCalled();
});

it("keeps a new review after confirmed pointer removal and Activity recovery", async () => {
  const receipt = {
    relayId: 7,
    operationId: "op-old",
    receiptRevision: 1,
    status: "settled",
    groups: [],
    targets: [],
    selections: [],
  };
  localStorage.setItem(relaySetupOperationKey(7), "op-old");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  const lookup = vi
    .spyOn(relayApi, "getSetupOperation")
    .mockResolvedValue(receipt as any);
  function Page() {
    const s = useRelaySetup(7);
    return (
      <>
        <span>phase:{s.phase}</span>
        <span>needs:{String(s.needsReconciliation)}</span>
        <span>error:{s.error ?? "none"}</span>
        <button onClick={s.newDraft}>new</button>
        <button onClick={() => s.setDraft({ codex: "group-1" })}>choose</button>
        <button onClick={s.reviewSelection}>review</button>
      </>
    );
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <Page />
      </PreservedView>
    </QueryClientProvider>
  );
  const v = render(view(true));
  await screen.findByText("phase:result");
  await screen.findByText("needs:false");
  fireEvent.click(screen.getByText("new"));
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  expect(screen.getByText("phase:review")).toBeTruthy();
  v.rerender(view(false));
  v.rerender(view(true));
  await waitFor(() =>
    expect(lookup.mock.calls.length).toBeGreaterThanOrEqual(3),
  );
  await act(async () => {});
  expect(screen.getByText("phase:review")).toBeTruthy();
  expect(screen.getByText("needs:false")).toBeTruthy();
  expect(screen.getByText("error:none")).toBeTruthy();
});
it("does not remove a pointer for a receipt still requiring reconciliation", async () => {
  const receipt = {
    relayId: 7,
    operationId: "op-pending",
    receiptRevision: 1,
    status: "needsReconcile",
    groups: [],
    targets: [],
    selections: [],
  };
  localStorage.setItem(relaySetupOperationKey(7), "op-pending");
  vi.spyOn(relayApi, "discoverTiers").mockResolvedValue(discovery as any);
  vi.spyOn(relayApi, "getSetupOperation").mockResolvedValue(receipt as any);
  function Page() {
    const s = useRelaySetup(7);
    return (
      <>
        <span>needs:{String(s.needsReconciliation)}</span>
        <button onClick={s.newDraft}>new</button>
      </>
    );
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Page />
    </QueryClientProvider>,
  );
  await screen.findByText("needs:true");
  fireEvent.click(screen.getByText("new"));
  expect(localStorage.getItem(relaySetupOperationKey(7))).toBe("op-pending");
});
