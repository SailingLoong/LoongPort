import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { useRelaySetup } from "@/components/relay/onboarding/useRelaySetup";
import { relayApi } from "@/lib/api/relay";

vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    discoverTiers: vi.fn(),
    getSetupOperation: vi.fn(),
    prepareTiers: vi.fn(),
  },
}));
vi.mock("@/lib/api/relaySetup", () => ({
  isRelayPreparationRejection: () => false,
}));
const discovery = {
  schemaVersion: 1,
  relayId: 7,
  revision: "d1",
  account: {
    identity: "account-7",
    accountLabel: "A",
    siteOrigin: "https://example.test",
  },
  choices: [
    {
      app: "codex",
      groupId: "group-1",
      groupName: "G",
      providerId: "provider-1",
      localConfiguration: "absent",
      compatibility: "unknown",
      keyStatus: "unknown",
    },
  ],
};
const receipt = {
  schemaVersion: 1,
  relayId: 7,
  operationId: "op-7",
  receiptRevision: 1,
  status: "settled",
  accountIdentity: "account-7",
  selectionDigest: "digest",
  createdKeyCount: 0,
  createdAt: 0,
  updatedAt: 0,
  groups: [],
  targets: [],
  selections: [],
};
function Stage({ id = 7 }: { id?: number }) {
  const setup = useRelaySetup(id);
  return (
    <>
      <span>phase:{setup.phase}</span>
      <span>draft:{setup.draft.codex ?? "empty"}</span>
      <span>revision:{setup.receipt?.receiptRevision ?? 0}</span>
      <span>needs:{String(setup.needsReconciliation)}</span>
      <span>error:{setup.error ?? "none"}</span>
      <span>recovery:{String(setup.recovery.isSuccess)}</span>
      <span>operation:{setup.receipt?.operationId ?? "none"}</span>
      <button onClick={setup.newDraft}>new</button>
      <button onClick={() => setup.setDraft({ codex: "group-1" })}>
        choose
      </button>
      <button onClick={setup.reviewSelection}>review</button>
      <button onClick={() => void setup.confirmSelection()}>prepare</button>
    </>
  );
}
function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}
afterEach(() => {
  localStorage.clear();
  vi.resetAllMocks();
});

it("retains the same account draft across Activity hide and reveal", () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  fireEvent.click(screen.getByText("choose"));
  expect(screen.getByText("draft:group-1")).toBeTruthy();
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  expect(screen.getByText("draft:group-1")).toBeTruthy();
});

it("keeps a new review when recovery rereads the same settled receipt", async () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("phase:result");
  fireEvent.click(screen.getByText("new"));
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  expect(screen.getByText("phase:review")).toBeTruthy();
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  expect(screen.getByText("phase:review")).toBeTruthy();
  expect(screen.getByText("draft:group-1")).toBeTruthy();
});

it("clears the previous draft for a different account owner", () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
  const queryClient = client();
  const view = (id: number) => (
    <QueryClientProvider client={queryClient}>
      <Stage id={id} />
    </QueryClientProvider>
  );
  const rendered = render(view(7));
  fireEvent.click(screen.getByText("choose"));
  rendered.rerender(view(8));
  expect(screen.getByText("draft:empty")).toBeTruthy();
});

it("lets a newer recovered receipt supersede a review", async () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation)
    .mockResolvedValueOnce(receipt as any)
    .mockResolvedValue({ ...receipt, receiptRevision: 2 } as any);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("phase:result");
  fireEvent.click(screen.getByText("new"));
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  expect(screen.getByText("phase:review")).toBeTruthy();
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await screen.findByText("revision:2");
  expect(screen.getByText("phase:result")).toBeTruthy();
});

it("does not suppress a recovered receipt that needs reconciliation", async () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation)
    .mockResolvedValueOnce(receipt as any)
    .mockResolvedValue({ ...receipt, status: "needsReconcile" } as any);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("phase:result");
  fireEvent.click(screen.getByText("new"));
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await screen.findByText("needs:true");
  expect(screen.getByText("phase:result")).toBeTruthy();
});

it("accepts a settled receipt when the prior receipt needed reconciliation", async () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation)
    .mockResolvedValueOnce({ ...receipt, status: "needsReconcile" } as any)
    .mockResolvedValue(receipt as any);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("needs:true");
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await screen.findByText("needs:false");
});

it("keeps recovery conflict for a different operation on the same account", async () => {
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation)
    .mockResolvedValueOnce(receipt as any)
    .mockResolvedValue({ ...receipt, operationId: "op-other" } as any);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("phase:result");
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await screen.findByText("error:recoveryConflict");
  expect(screen.getByText("needs:true")).toBeTruthy();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

it("retains a hidden successful prepare even when recovery returns null", async () => {
  const pending = deferred<any>();
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
  vi.mocked(relayApi.prepareTiers)
    .mockReturnValueOnce(pending.promise)
    .mockReturnValue(new Promise(() => {}));
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  const request = vi.mocked(relayApi.prepareTiers).mock.calls[0][0];
  rendered.rerender(view(false));
  await act(async () => {
    pending.resolve({ ...receipt, operationId: request.operationId });
    await pending.promise;
  });
  rendered.rerender(view(true));
  await screen.findByText(`operation:${request.operationId}`);
  fireEvent.click(screen.getByText("prepare"));
  expect(relayApi.prepareTiers).toHaveBeenCalledTimes(1);
  expect(screen.getByText("phase:result")).toBeTruthy();
});

it("keeps a hidden failed prepare uncertain when recovery returns null", async () => {
  const pending = deferred<any>();
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
  vi.mocked(relayApi.prepareTiers).mockReturnValue(pending.promise);
  const queryClient = client();
  const view = (active: boolean) => (
    <QueryClientProvider client={queryClient}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  rendered.rerender(view(false));
  await act(async () => {
    pending.reject(new Error("transport"));
    await pending.promise.catch(() => {});
  });
  rendered.rerender(view(true));
  await screen.findByText("needs:true");
  fireEvent.click(screen.getByText("prepare"));
  expect(relayApi.prepareTiers).toHaveBeenCalledTimes(1);
});

it("does not apply a late account A receipt to account B", async () => {
  const pending = deferred<any>();
  vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
  vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
  vi.mocked(relayApi.prepareTiers).mockReturnValue(pending.promise);
  const queryClient = client();
  const view = (id: number) => (
    <QueryClientProvider client={queryClient}>
      <Stage id={id} />
    </QueryClientProvider>
  );
  const rendered = render(view(7));
  await screen.findByText("recovery:true");
  fireEvent.click(screen.getByText("choose"));
  fireEvent.click(screen.getByText("review"));
  fireEvent.click(screen.getByText("prepare"));
  const request = vi.mocked(relayApi.prepareTiers).mock.calls[0][0];
  rendered.rerender(view(8));
  await act(async () => {
    pending.resolve({ ...receipt, operationId: request.operationId });
    await pending.promise;
  });
  expect(screen.getByText("operation:none")).toBeTruthy();
  expect(screen.getByText("phase:selection")).toBeTruthy();
});
