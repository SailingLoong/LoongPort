import { StrictMode } from "react";
import { act, render, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { useRelaySetupConfigurationViewModel } from "@/components/relay/onboarding/useRelaySetupConfigurationViewModel";

const mocked = vi.hoisted(() => ({ setup: {} as any }));
vi.mock("@/components/relay/onboarding/useRelaySetup", () => ({
  useRelaySetup: () => mocked.setup,
}));
const account = { kind: "relay" as const, rowId: 7, name: "A" };
function viewModel() {
  const client = new QueryClient();
  return renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  ).result.current;
}
it("projects recovery and retry/result facts without manufacturing a connection", () => {
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      isSuccess: true,
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    receipt: {
      operationId: "op-7",
      receiptRevision: 1,
      selections: [],
      groups: [
        { groupId: "g", groupName: "G", state: "ready", origin: "created" },
      ],
      targets: [
        {
          app: "codex",
          groupId: "g",
          configuration: "failed",
          compatibility: "supported",
          connection: { state: "notRequested" },
        },
        {
          app: "claude",
          groupId: "g",
          configuration: "prepared",
          compatibility: "supported",
          connection: { state: "notRequested" },
        },
        {
          app: "gemini",
          groupId: "g",
          configuration: "prepared",
          compatibility: "unsupported",
          connection: { state: "notRequested" },
        },
      ],
    },
  };
  const vm = viewModel();
  expect(vm.resultVisible).toBe(true);
  expect(vm.retryVisible).toBe(true);
  expect(
    vm.resultRows.map((row) => [
      row.app,
      row.result.key,
      row.canConnect,
      row.unsupported,
    ]),
  ).toEqual([
    ["codex", "keyOnly", false, false],
    ["claude", "prepared", true, false],
    ["gemini", "prepared", false, true],
  ]);
  expect(vm.resultRows.every((row) => row.origin.key === "keyCreated")).toBe(
    true,
  );
});
it("does not offer retry while an operation needs reconciliation", () => {
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: {},
    needsReconciliation: true,
    receipt: {
      operationId: "op-7",
      receiptRevision: 1,
      groups: [],
      selections: [],
      targets: [
        {
          app: "codex",
          groupId: "g",
          configuration: "failed",
          compatibility: "supported",
          connection: { state: "unknown" },
        },
      ],
    },
  };
  const vm = viewModel();
  expect(vm.retryVisible).toBe(false);
  expect(vm.resultRows[0].result.key).toBe("connectionUnknown");
  expect(vm.resultRows[0].canConnect).toBe(false);
});
it("does not prepare without acknowledgement or after same-turn revocation", async () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
    cancelDraft: vi.fn(),
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  await act(async () => {
    await hook.result.current.confirmPreparation();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
  act(() => hook.result.current.setAcknowledged(true));
  const old = hook.result.current.confirmPreparation;
  await act(async () => {
    hook.result.current.setAcknowledged(false);
    await old();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
});
it("does not connect without acknowledgement or after same-turn revocation", async () => {
  const confirmConnection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    receipt: {
      operationId: "op-7",
      receiptRevision: 1,
      groups: [],
      selections: [],
      targets: [],
    },
    connectionReview: { app: "codex", confirmation: null },
    confirmConnection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.beginConnection());
  expect(confirmConnection).not.toHaveBeenCalled();
  act(() => hook.result.current.setConnectionAcknowledged(true));
  const old = hook.result.current.beginConnection;
  act(() => {
    hook.result.current.setConnectionAcknowledged(false);
    old();
  });
  expect(confirmConnection).not.toHaveBeenCalled();
});
it("accepts the current preparation acknowledgement exactly once", async () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.setAcknowledged(true));
  const command = hook.result.current.confirmPreparation;
  await act(async () => {
    await command();
    await command();
  });
  expect(confirmSelection).toHaveBeenCalledTimes(1);
});
it("cancelling review retires a retained preparation command in the same turn", async () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
    cancelDraft: vi.fn(),
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.setAcknowledged(true));
  const command = hook.result.current.confirmPreparation;
  await act(async () => {
    hook.result.current.cancelReview();
    await command();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
});
it("a new receipt revision retires the previous preparation acknowledgement", async () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    receipt: {
      operationId: "op",
      receiptRevision: 1,
      targets: [],
      groups: [],
      selections: [],
    },
    confirmSelection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.setAcknowledged(true));
  const command = hook.result.current.confirmPreparation;
  mocked.setup = {
    ...mocked.setup,
    receipt: { ...mocked.setup.receipt, receiptRevision: 2 },
  };
  hook.rerender();
  await act(async () => {
    await command();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
  expect(hook.result.current.acknowledged).toBe(false);
});
it("connects only after acknowledging and opening the current confirmation", () => {
  const confirmConnection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    receipt: {
      operationId: "op",
      receiptRevision: 1,
      targets: [],
      groups: [],
      selections: [],
    },
    connectionReview: {
      app: "codex",
      operationId: "op",
      receiptRevision: 1,
      requestId: "r",
      confirmation: {},
    },
    confirmConnection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.setConnectionAcknowledged(true));
  act(() => hook.result.current.beginConnection());
  expect(hook.result.current.connectionDialog).toBe(true);
  const command = hook.result.current.switchConnection;
  act(() => {
    command(false);
    command(false);
  });
  expect(confirmConnection).toHaveBeenCalledTimes(1);
});
it("does not let a stale preparation acknowledgement setter authorize a new review", () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    receipt: {
      operationId: "op",
      receiptRevision: 1,
      targets: [],
      groups: [],
      selections: [],
    },
    confirmSelection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const oldSetter = hook.result.current.setAcknowledged;
  mocked.setup = {
    ...mocked.setup,
    receipt: { ...mocked.setup.receipt, receiptRevision: 2 },
  };
  hook.rerender();
  act(() => oldSetter(true));
  expect(hook.result.current.acknowledged).toBe(false);
  act(() => {
    void hook.result.current.confirmPreparation();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
});
it("does not let a stale connection acknowledgement setter authorize a new target session", () => {
  const confirmConnection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    receipt: {
      operationId: "op",
      receiptRevision: 1,
      targets: [],
      groups: [],
      selections: [],
    },
    connectionReview: {
      app: "codex",
      operationId: "op",
      receiptRevision: 1,
      requestId: "r1",
      confirmation: null,
    },
    confirmConnection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const oldSetter = hook.result.current.setConnectionAcknowledged;
  mocked.setup = {
    ...mocked.setup,
    connectionReview: { ...mocked.setup.connectionReview, requestId: "r2" },
  };
  hook.rerender();
  act(() => oldSetter(true));
  expect(hook.result.current.connectionAcknowledged).toBe(false);
  act(() => hook.result.current.beginConnection());
  expect(confirmConnection).not.toHaveBeenCalled();
});

it("rejects a stale selection command after a new committed surface", () => {
  const setDraft = vi.fn();
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "selection",
    draft: {},
    discovery: {
      isSuccess: true,
      data: {
        choices: [{ app: "codex", groupId: "g", groupName: "G" }],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    setDraft,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const old = hook.result.current.selectApp;
  mocked.setup = { ...mocked.setup, draft: { codex: "g" } };
  hook.rerender();
  act(() => old("codex", "g"));
  expect(setDraft).not.toHaveBeenCalled();
});
it("rejects a retained retry and connection command after receipt revision changes", () => {
  const reviewConnection = vi.fn();
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "result",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    receipt: {
      operationId: "op",
      receiptRevision: 1,
      groups: [],
      selections: [],
      targets: [
        {
          app: "codex",
          groupId: "g",
          configuration: "prepared",
          compatibility: "supported",
          connection: { state: "notRequested" },
        },
        {
          app: "claude",
          groupId: "g",
          configuration: "failed",
          compatibility: "supported",
          connection: { state: "notRequested" },
        },
      ],
    },
    reviewConnection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const retry = hook.result.current.startRetry;
  const connect = hook.result.current.openConnectionReview;
  mocked.setup = {
    ...mocked.setup,
    receipt: { ...mocked.setup.receipt, receiptRevision: 2 },
  };
  hook.rerender();
  act(() => {
    retry();
    connect("codex");
  });
  expect(hook.result.current.reviewing).toBe(false);
  expect(reviewConnection).not.toHaveBeenCalled();
});
it("a retained acknowledgement setter cannot authorize a review after hide and reveal", () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
  };
  const client = new QueryClient();
  let current!: ReturnType<typeof useRelaySetupConfigurationViewModel>;
  function Stage() {
    current = useRelaySetupConfigurationViewModel(account, "codex", vi.fn());
    return null;
  }
  const view = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <Stage />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  const old = current.setAcknowledged;
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  act(() => old(true));
  expect(current.acknowledged).toBe(false);
  act(() => {
    void current.confirmPreparation();
  });
  expect(confirmSelection).not.toHaveBeenCalled();
});
it("a same-turn refresh synchronously blocks a retained confirmed preparation", async () => {
  let resolve!: () => void;
  const pending = new Promise<void>((done) => {
    resolve = done;
  });
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
      refetch: () => pending,
    },
    recovery: { isSuccess: true, refetch: () => pending },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
  };
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", vi.fn()),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.setAcknowledged(true));
  const confirm = hook.result.current.confirmPreparation;
  const refresh = hook.result.current.refreshAll;
  await act(async () => {
    refresh();
    await confirm();
    resolve();
    await pending;
  });
  expect(confirmSelection).not.toHaveBeenCalled();
});
it("allows a fresh explicit exit while a read-only refresh remains pending", async () => {
  const pending = new Promise<void>(() => {});
  const dismiss = vi.spyOn(serviceOnboardingApi, "dismiss").mockResolvedValue({
    shouldPrompt: false,
    completed: false,
    plazaVisible: false,
  });
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "selection",
    draft: {},
    discovery: {
      isSuccess: true,
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
      refetch: () => pending,
    },
    recovery: { isSuccess: true, refetch: () => pending },
    needsReconciliation: false,
  };
  const onDone = vi.fn();
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", onDone),
    {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  act(() => hook.result.current.refreshAll());
  await act(async () => {
    await hook.result.current.finish();
  });
  expect(dismiss).toHaveBeenCalledTimes(1);
  expect(onDone).toHaveBeenCalledTimes(1);
  dismiss.mockRestore();
});

it("fresh commands work under StrictMode effect replay", async () => {
  const dismiss = vi.spyOn(serviceOnboardingApi, "dismiss").mockResolvedValue({
    shouldPrompt: false,
    completed: false,
    plazaVisible: false,
  });
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "selection",
    draft: {},
    discovery: {
      isSuccess: true,
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
  };
  const onDone = vi.fn();
  const client = new QueryClient();
  const hook = renderHook(
    () => useRelaySetupConfigurationViewModel(account, "codex", onDone),
    {
      wrapper: ({ children }) => (
        <StrictMode>
          <QueryClientProvider client={client}>{children}</QueryClientProvider>
        </StrictMode>
      ),
    },
  );
  await act(async () => {
    await hook.result.current.finish();
  });
  expect(dismiss).toHaveBeenCalledTimes(1);
  expect(onDone).toHaveBeenCalledTimes(1);
  dismiss.mockRestore();
});
it("StrictMode reveal rejects old acknowledgement but accepts a fresh review", async () => {
  const confirmSelection = vi.fn().mockResolvedValue(undefined);
  mocked.setup = {
    busy: false,
    canStartOperation: () => true,
    phase: "review",
    draft: {},
    discovery: {
      data: {
        choices: [],
        account: { accountLabel: "A", siteOrigin: "https://example.test" },
      },
    },
    recovery: { isSuccess: true },
    needsReconciliation: false,
    reviewed: { selections: [] },
    confirmSelection,
  };
  const client = new QueryClient();
  let current!: ReturnType<typeof useRelaySetupConfigurationViewModel>;
  function Stage() {
    current = useRelaySetupConfigurationViewModel(account, "codex", vi.fn());
    return null;
  }
  const view = (active: boolean) => (
    <StrictMode>
      <QueryClientProvider client={client}>
        <PreservedView active={active}>
          <Stage />
        </PreservedView>
      </QueryClientProvider>
    </StrictMode>
  );
  const rendered = render(view(true));
  const old = current.setAcknowledged;
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  act(() => old(true));
  expect(current.acknowledged).toBe(false);
  act(() => current.setAcknowledged(true));
  expect(current.acknowledged).toBe(true);
  await act(async () => {
    await current.confirmPreparation();
  });
  expect(confirmSelection).toHaveBeenCalledTimes(1);
});
