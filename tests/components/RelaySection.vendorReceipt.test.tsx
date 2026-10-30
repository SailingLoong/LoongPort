import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { RelaySection } from "@/components/relay/RelaySection";
import { PreservedView } from "@/components/ui/PreservedView";
import { vendorApi } from "@/lib/api/vendor";
import { relayApi } from "@/lib/api";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";

const mock = vi.hoisted(() => ({
  invoke: vi.fn(),
  confirm: null as any,
  review: null as any,
  actions: null as any,
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("sonner", () => ({
  toast: { success: mock.success, error: mock.error, warning: mock.warning },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: () => {} }));
vi.mock("@/hooks/useStreamCheck", () => ({
  useStreamCheck: () => ({ checkProvider: vi.fn(), isChecking: () => false }),
}));
vi.mock("@/components/relay/useTierEditGuard", () => ({
  useTierEditGuard: () => ({ requestEdit: vi.fn(), editDialogs: null }),
}));
vi.mock("@/components/relay/VendorBlock", () => ({
  VendorBlock: ({ vendor }: any) => {
    mock.actions = vendor;
    return (
      <button onClick={() => vendor.onUse(1, vendor.accounts[0].plans[0])}>
        Use vendor
      </button>
    );
  },
}));
vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: (props: any) => {
    mock.confirm = props;
    return null;
  },
}));
vi.mock("@/components/relay/RelayTierList", () => ({
  RelayTierList: () => null,
}));
vi.mock(
  "@/components/relay/model-verification/TierVerificationProvider",
  () => ({ TierVerificationProvider: ({ children }: any) => children }),
);
vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: (props: any) => {
    if (props.title === "loongport.onboarding.reapply") mock.review = props;
    return null;
  },
}));
const plan = {
  planId: "plan",
  planName: "Plan",
  providerId: "provider",
  canSwitch: true,
  isCurrent: false,
  userEdited: false,
  canEditConfig: true,
};
const row = {
  id: 1,
  vendorId: "deepseek",
  vendorName: "Vendor",
  accountLabel: "Account",
  status: "ready",
  plans: [plan],
};
const snapshot = {
  configurationRevision: "rev1",
  isAdditive: false,
  configurations: [
    {
      selection: { kind: "vendor", rowId: 1, planId: "plan" },
      providerId: "provider",
      presentation: { isCurrent: true },
    },
  ],
};
const full = (
  operationId: string,
  status: "prepared" | "completed" = "prepared",
) =>
  ({
    status,
    operationState: status,
    rowId: 1,
    plan: "plan",
    app: "codex",
    providerId: "provider",
    operationId,
    receiptRevision: 3,
    completedConfigurationRevision: status === "completed" ? "rev1" : null,
  }) as const;
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: any) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return render(<RelaySection appId="codex" onOpenAddHub={() => {}} />, {
    wrapper,
  });
}
beforeEach(() => {
  vi.restoreAllMocks();
  mock.invoke.mockReset();
  mock.confirm = null;
  mock.review = null;
  mock.actions = null;
  mock.success.mockReset();
  mock.error.mockReset();
  mock.warning.mockReset();
  vi.spyOn(relayApi, "listRelays").mockResolvedValue([]);
  vi.spyOn(vendorApi, "list").mockResolvedValue({
    supported: true,
    accounts: [row],
  } as any);
  vi.spyOn(applicationOverviewApi, "state").mockResolvedValue(snapshot as any);
  vi.spyOn(applicationRoutingApi, "apply").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    catalog: { choice: "keep", revision: "r" },
  } as any);
});
afterEach(cleanup);

it("uses one operation ID through prepare, dispatch and authoritative query after a lost switch reply", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockImplementation(async (_app, id) =>
      id ? (full(id, "completed") as any) : { status: "none" },
    );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockRejectedValue(new Error("reply lost"));
  const legacy = vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  const input = prepare.mock.calls[0][0];
  expect(dispatch).toHaveBeenCalledWith({
    rowId: 1,
    plan: "plan",
    app: "codex",
    operationId: input.operationId,
    expectedRevision: 3,
    quitChatgpt: null,
  });
  expect(query).toHaveBeenCalledWith("codex", input.operationId);
  await waitFor(() =>
    expect(mock.success).toHaveBeenCalledWith(
      "loongport.onboarding.results.success",
    ),
  );
  expect(legacy).not.toHaveBeenCalled();
});

it.each([
  { rowId: 2 },
  { plan: "other" },
  { app: "claude" },
  { providerId: "other" },
  { operationId: "other" },
])("never dispatches a mismatched prepared target %o", async (mismatch) => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({
    status: "none",
  } as any);
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => ({ ...full(input.operationId), ...mismatch }) as any,
  );
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({ status: "unknown" });
  vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  expect(dispatch).not.toHaveBeenCalled();
});

it("does not route a vendor catalog callback through generic apply, even after cancellation", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({
    status: "none",
  } as any);
  vi.spyOn(vendorApi, "prepareConnection").mockResolvedValue({
    status: "unknown",
  } as any);
  vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    catalog: { choice: "keep", revision: "r" },
  } as any);
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() =>
    expect(
      Boolean(mock.confirm?.targetName) || mock.error.mock.calls.length > 0,
    ).toBe(true),
  );
  const old = mock.confirm;
  act(() => old.onPreviewCatalog({ choice: "keep", revision: "r" }));
  await act(async () => {});
  act(() => {
    old.onCancel();
    old.onSwitch(false, { choice: "keep", revision: "r" });
  });
  expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
});

it("queries the original operation after a lost prepare and never creates a replacement on another click", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockImplementation(async (_app, id) =>
      id
        ? ({ status: "unknown", operationId: id, app: "codex" } as any)
        : { status: "none" },
    );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockRejectedValue(new Error("lost prepare reply"));
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({ status: "unknown" });
  vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalledTimes(1));
  const id = prepare.mock.calls[0][0].operationId;
  expect(query).toHaveBeenLastCalledWith("codex", id);
  fireEvent.click(screen.getByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalledTimes(2));
  expect(query).toHaveBeenLastCalledWith("codex", id);
  expect(prepare).toHaveBeenCalledTimes(1);
  expect(dispatch).not.toHaveBeenCalled();
});

it("does not dispatch the old app after the pending read resolves following an app change", async () => {
  let resolve!: (value: any) => void;
  const query = vi.spyOn(vendorApi, "queryConnection").mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const prepare = vi.spyOn(vendorApi, "prepareConnection");
  vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  const view = setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(query).toHaveBeenCalled());
  view.rerender(<RelaySection appId="claude" onOpenAddHub={() => {}} />);
  await act(async () => {
    resolve({ status: "none" });
  });
  expect(prepare).not.toHaveBeenCalled();
  expect(mock.success).not.toHaveBeenCalled();
});

it("rejects a retained row callback after its plan changes", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockResolvedValue({ status: "none" } as any);
  vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  const view = setup();
  await screen.findByText("Use vendor");
  const old = mock.actions.onUse;
  const changed = { ...row, plans: [{ ...plan, providerId: "different" }] };
  view.rerender(
    <RelaySection
      appId="codex"
      onOpenAddHub={() => {}}
      accountSnapshot={{ kind: "vendor", row: changed as any, appId: "codex" }}
    />,
  );
  act(() => old(1, plan));
  expect(query).not.toHaveBeenCalled();
  expect(mock.success).not.toHaveBeenCalled();
});

it("retires a retained vendor action when its preserved view is hidden", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockResolvedValue({ status: "none" } as any);
  const legacy = vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: any) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const view = render(
    <PreservedView active>
      <RelaySection appId="codex" onOpenAddHub={() => {}} />
    </PreservedView>,
    { wrapper },
  );
  await screen.findByText("Use vendor");
  const old = mock.actions.onUse;
  view.rerender(
    <PreservedView active={false}>
      <RelaySection appId="codex" onOpenAddHub={() => {}} />
    </PreservedView>,
  );
  act(() => old(1, plan));
  expect(query).not.toHaveBeenCalled();
  expect(legacy).not.toHaveBeenCalled();
});

it("after verified completed A permits a separately selected B without replaying A", async () => {
  const planB = {
    ...plan,
    planId: "plan-b",
    planName: "Plan B",
    providerId: "provider-b",
  };
  vi.mocked(vendorApi.list).mockResolvedValue({
    supported: true,
    accounts: [{ ...row, plans: [plan, planB] }],
  } as any);
  const byId = new Map<string, any>();
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id ? byId.get(id) : { status: "none" },
  );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => {
      const receipt = {
        ...full(input.operationId),
        plan: input.plan,
        providerId: input.plan === "plan-b" ? "provider-b" : "provider",
      };
      byId.set(input.operationId, {
        ...receipt,
        status: "completed",
        operationState: "completed",
        completedConfigurationRevision: "rev1",
      });
      return receipt as any;
    });
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({ status: "unknown" });
  setup();
  await screen.findByText("Use vendor");
  act(() => mock.actions.onUse(1, plan));
  await waitFor(() =>
    expect(applicationOverviewApi.state).toHaveBeenCalledTimes(1),
  );
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  act(() => mock.actions.onUse(1, planB));
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(2));
  expect(prepare.mock.calls.map(([input]) => input.plan)).toEqual([
    "plan",
    "plan-b",
  ]);
  expect(prepare.mock.calls[0][0].operationId).not.toEqual(
    prepare.mock.calls[1][0].operationId,
  );
});

it("a remounted view may start B after the app query reports terminal completed A", async () => {
  const planB = {
    ...plan,
    planId: "plan-b",
    planName: "Plan B",
    providerId: "provider-b",
  };
  vi.mocked(vendorApi.list).mockResolvedValue({
    supported: true,
    accounts: [{ ...row, plans: [plan, planB] }],
  } as any);
  const latest = full("op-a", "completed");
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id
      ? ({
          ...full(id, "completed"),
          plan: "plan-b",
          providerId: "provider-b",
        } as any)
      : (latest as any),
  );
  const prepare = vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) =>
      ({
        ...full(input.operationId),
        plan: "plan-b",
        providerId: "provider-b",
      }) as any,
  );
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({ status: "unknown" });
  setup();
  await screen.findByText("Use vendor");
  act(() => mock.actions.onUse(1, planB));
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  expect(prepare.mock.calls[0][0]).toMatchObject({
    rowId: 1,
    plan: "plan-b",
    app: "codex",
  });
});

it("does not revive a retained row action after target A to B to A", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockResolvedValue({ status: "none" } as any);
  const legacy = vi.spyOn(vendorApi, "switch").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
  } as any);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: any) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const make = (item: any) => (
    <RelaySection
      appId="codex"
      onOpenAddHub={() => {}}
      accountSnapshot={{ kind: "vendor", row: item, appId: "codex" }}
    />
  );
  const view = render(make(row), { wrapper });
  await screen.findByText("Use vendor");
  const old = mock.actions.onUse;
  view.rerender(make({ ...row, id: 2 }));
  view.rerender(make(row));
  act(() => old(1, plan));
  expect(query).not.toHaveBeenCalled();
  expect(legacy).not.toHaveBeenCalled();
});

it("rejects a retained vendor action when the vendor-plan surface is no longer rendered", async () => {
  const query = vi
    .spyOn(vendorApi, "queryConnection")
    .mockResolvedValue({ status: "none" } as any);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: any) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const view = render(<RelaySection appId="codex" onOpenAddHub={() => {}} />, {
    wrapper,
  });
  await screen.findByText("Use vendor");
  const old = mock.actions.onUse;
  view.rerender(
    <RelaySection appId="codex" onOpenAddHub={() => {}} accountActionsOnly />,
  );
  act(() => old(1, plan));
  expect(query).not.toHaveBeenCalled();
});

it.each([
  { rowId: 2 },
  { plan: "other" },
  { app: "claude" },
  { providerId: "other" },
  { operationId: "other" },
])("does not accept a mismatched completed query %o", async (mismatch) => {
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id
      ? ({ ...full(id, "completed"), ...mismatch } as any)
      : { status: "none" },
  );
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({ status: "unknown" });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  expect(dispatch).toHaveBeenCalledTimes(1);
  expect(applicationOverviewApi.state).not.toHaveBeenCalled();
  expect(mock.success).not.toHaveBeenCalled();
});

it("does not treat a redacted unresolved app query as absence", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({
    status: "unknown",
  });
  const prepare = vi.spyOn(vendorApi, "prepareConnection");
  const dispatch = vi.spyOn(vendorApi, "switchPreparedConnection");
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  expect(prepare).not.toHaveBeenCalled();
  expect(dispatch).not.toHaveBeenCalled();
  expect(mock.success).not.toHaveBeenCalled();
});

it("real vendor adapter does not dispatch when prepared wire data includes an unknown protection field", async () => {
  mock.invoke.mockImplementation(async (command: string, payload: any) => {
    if (command === "vendor_query_connection")
      return payload.operationId === null
        ? { status: "none" }
        : { status: "unknown" };
    if (command === "vendor_prepare_connection")
      return { ...full(payload.operationId), quitRequired: true };
    if (command === "vendor_switch") return { status: "completed" };
    throw new Error(`unexpected command ${command}`);
  });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  expect(
    mock.invoke.mock.calls.some(
      ([command]) => command === "vendor_prepare_connection",
    ),
  ).toBe(true);
  expect(
    mock.invoke.mock.calls.some(([command]) => command === "vendor_switch"),
  ).toBe(false);
  expect(mock.success).not.toHaveBeenCalled();
});

it("continues an explicit quit confirmation with the same prepared operation exactly once", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id ? (full(id, "completed") as any) : { status: "none" },
  );
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: true,
    })
    .mockResolvedValue({
      status: "switched",
      providerName: "Plan",
      chatgptWasRunning: true,
      chatgptRelaunched: true,
      warnings: [],
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.targetName).toBe("Plan"));
  const confirm = mock.confirm;
  expect(mock.success).not.toHaveBeenCalled();
  act(() => {
    confirm.onSwitch(true);
    confirm.onSwitch(true);
  });
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(2));
  expect(dispatch.mock.calls[1][0]).toEqual({
    ...dispatch.mock.calls[0][0],
    quitChatgpt: true,
  });
  await waitFor(() => expect(mock.success).toHaveBeenCalledTimes(1));
});

const catalog = {
  ownership: "none",
  choice: "keep",
  canManage: true,
  canRestore: false,
  revision: "catalog-1",
  beforePointer: null,
  afterPointer: null,
} as any;
it("previews catalog through the bound operation then retires and prepares the confirmed decision", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id ? (full(id, "completed") as any) : { status: "none" },
  );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog,
    })
    .mockResolvedValue({
      status: "switched",
      providerName: "Plan",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
      warnings: [],
    });
  const preview = vi
    .spyOn(vendorApi, "previewConnectionCatalog")
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog: { ...catalog, choice: "manage", revision: "catalog-2" },
    });
  const review = vi
    .spyOn(vendorApi, "previewConnectionReview")
    .mockImplementation(async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "observed-1",
    }));
  const retire = vi.spyOn(vendorApi, "reviewConnection").mockImplementation(
    async (input) =>
      ({
        ...full(input.operationId),
        receiptRevision: 4,
        status: "failedBeforeWrite",
        operationState: "failedBeforeWrite",
      }) as any,
  );
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() =>
    expect(mock.confirm?.catalog?.revision).toBe("catalog-1"),
  );
  const old = mock.confirm;
  act(() => old.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }));
  await waitFor(() =>
    expect(mock.confirm?.catalog?.revision).toBe("catalog-2"),
  );
  expect(preview).toHaveBeenCalledWith({
    app: "codex",
    operationId: prepare.mock.calls[0][0].operationId,
    catalogDecision: { choice: "manage", revision: "catalog-1" },
    quitChatgpt: null,
  });
  expect(retire).not.toHaveBeenCalled();
  expect(dispatch).toHaveBeenCalledTimes(1);
  act(() => {
    old.onSwitch(false, { choice: "keep", revision: "catalog-1" });
    mock.confirm.onSwitch(false, { choice: "manage", revision: "catalog-2" });
  });
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(2));
  expect(review).toHaveBeenCalledTimes(2);
  expect(retire).toHaveBeenCalledTimes(1);
  expect(prepare.mock.calls[1][0].operationId).not.toBe(
    prepare.mock.calls[0][0].operationId,
  );
  expect(prepare.mock.calls[1][0].catalogDecision).toEqual({
    choice: "manage",
    revision: "catalog-2",
  });
  expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
});
it("cancelling a native quit confirmation invalidates its old callback", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({ status: "none" });
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: true,
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.targetName).toBe("Plan"));
  const old = mock.confirm;
  act(() => {
    old.onCancel();
    old.onSwitch(true);
  });
  await act(async () => {});
  expect(dispatch).toHaveBeenCalledTimes(1);
});

it("requires explicit bound new-intent review before replacing an unresolved operation", async () => {
  const oldId = "11111111-1111-4111-8111-111111111111";
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id
      ? (full(id, "completed") as any)
      : { status: "unknown", app: "codex", operationId: oldId },
  );
  const preview = vi
    .spyOn(vendorApi, "previewNewConnectionIntent")
    .mockResolvedValue({
      status: "newIntentReviewRequired",
      app: "codex",
      operationId: oldId,
      receiptRevision: 3,
      originalRowId: 1,
      originalPlan: "plan",
      targetRowId: 1,
      targetPlan: "plan",
      targetProviderId: "provider",
      reviewToken: "token",
    });
  const confirm = vi
    .spyOn(vendorApi, "confirmNewConnectionIntent")
    .mockResolvedValue({ status: "newIntentPermitted", app: "codex" });
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({
      status: "switched",
      providerName: "Plan",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
      warnings: [],
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.review?.isOpen).toBe(true));
  expect(preview).toHaveBeenCalledTimes(1);
  expect(confirm).not.toHaveBeenCalled();
  expect(prepare).not.toHaveBeenCalled();
  const view = mock.review;
  act(() => {
    view.onConfirm();
    view.onConfirm();
  });
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  expect(confirm).toHaveBeenCalledTimes(1);
  expect(prepare).toHaveBeenCalledTimes(1);
  expect(prepare.mock.calls[0][0].operationId).not.toBe(oldId);
  expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
});

it("requires a new explicit choice when a completed configuration has subsequently changed", async () => {
  const oldId = "11111111-1111-4111-8111-111111111111";
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(
    async (_app, id) => full(id ?? oldId, "completed") as any,
  );
  vi.mocked(applicationOverviewApi.state)
    .mockResolvedValueOnce({
      ...snapshot,
      configurationRevision: "changed",
    } as any)
    .mockResolvedValue(snapshot as any);
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({
      status: "switched",
      providerName: "Plan",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
      warnings: [],
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.review?.isOpen).toBe(true));
  expect(prepare).not.toHaveBeenCalled();
  expect(mock.success).not.toHaveBeenCalled();
  const view = mock.review;
  act(() => {
    view.onConfirm();
    view.onConfirm();
  });
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  expect(prepare.mock.calls[0][0].operationId).not.toBe(oldId);
});

it("binds catalog retirement to the review shown before confirmation, never a fresh confirm-time revision", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id ? (full(id, "completed") as any) : { status: "none" },
  );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog,
    });
  const preview = vi
    .spyOn(vendorApi, "previewConnectionReview")
    .mockImplementation(async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "shown",
    }));
  const retire = vi
    .spyOn(vendorApi, "reviewConnection")
    .mockRejectedValue(new Error("configuration changed"));
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.catalog).toBeTruthy());
  expect(preview).toHaveBeenCalledTimes(1);
  preview.mockImplementation(async (input) => ({
    ...input,
    status: "reviewRequired",
    receiptRevision: 4,
    observedRevision: "changed-after-display",
  }));
  act(() =>
    mock.confirm.onSwitch(false, { choice: "keep", revision: "catalog-1" }),
  );
  await waitFor(() => expect(retire).toHaveBeenCalledTimes(1));
  expect(retire.mock.calls[0][0]).toMatchObject({
    expectedRevision: 3,
    observedRevision: "shown",
  });
  expect(preview).toHaveBeenCalledTimes(1);
  expect(prepare).toHaveBeenCalledTimes(1);
  expect(dispatch).toHaveBeenCalledTimes(1);
});

it("keeps a new vendor prompt when an obsolete catalog prompt cancels late", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({ status: "none" });
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    quitRequired: false,
    catalog,
  });
  vi.spyOn(vendorApi, "previewConnectionReview").mockImplementation(
    async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "shown",
    }),
  );
  vi.spyOn(vendorApi, "previewConnectionCatalog").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    quitRequired: false,
    catalog: { ...catalog, choice: "manage", revision: "new" },
  });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.catalog).toBeTruthy());
  const old = mock.confirm;
  act(() => old.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }));
  await waitFor(() => expect(mock.confirm?.catalog?.revision).toBe("new"));
  act(() => old.onCancel());
  expect(mock.confirm?.catalog?.revision).toBe("new");
  expect(mock.confirm?.targetName).toBe("Plan");
});
it("allows retrying a failed catalog preview without a dead confirmation button", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({ status: "none" });
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    quitRequired: false,
    catalog,
  });
  vi.spyOn(vendorApi, "previewConnectionReview").mockImplementation(
    async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "shown",
    }),
  );
  const preview = vi
    .spyOn(vendorApi, "previewConnectionCatalog")
    .mockRejectedValueOnce(new Error("lost"))
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog: { ...catalog, choice: "manage", revision: "new" },
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.catalog).toBeTruthy());
  act(() =>
    mock.confirm.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }),
  );
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  act(() =>
    mock.confirm.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }),
  );
  await waitFor(() => expect(mock.confirm?.catalog?.revision).toBe("new"));
  expect(preview).toHaveBeenCalledTimes(2);
});
it("allows a fresh explicit use after authoritative retirement survived a lost reply", async () => {
  const oldId = "11111111-1111-4111-8111-111111111111";
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(async (_app, id) =>
    id
      ? (full(id, "completed") as any)
      : ({
          ...full(oldId),
          status: "unknown",
          operationState: "failedBeforeWrite",
        } as any),
  );
  const prepare = vi
    .spyOn(vendorApi, "prepareConnection")
    .mockImplementation(async (input) => full(input.operationId) as any);
  const dispatch = vi
    .spyOn(vendorApi, "switchPreparedConnection")
    .mockResolvedValue({
      status: "switched",
      providerName: "Plan",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
      warnings: [],
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(dispatch).toHaveBeenCalledTimes(1));
  expect(prepare.mock.calls[0][0].operationId).not.toBe(oldId);
});

it("runs catalog confirmation through the real vendor adapter and native-shaped IPC replies", async () => {
  let firstId = "";
  let switched = false;
  mock.invoke.mockImplementation(async (command: string, payload: any) => {
    if (command === "vendor_query_connection")
      return payload.operationId === null
        ? { status: "none" }
        : full(payload.operationId, switched ? "completed" : "prepared");
    if (command === "vendor_prepare_connection") {
      firstId ||= payload.operationId;
      return full(payload.operationId);
    }
    if (command === "vendor_switch") {
      if (payload.operationId === firstId)
        return {
          status: "confirmationRequired",
          targetName: "Plan",
          quitRequired: false,
          catalog,
        };
      switched = true;
      return {
        status: "switched",
        providerName: "Plan",
        chatgptWasRunning: false,
        chatgptRelaunched: false,
        warnings: [],
      };
    }
    if (command === "vendor_preview_connection_review")
      return {
        status: "reviewRequired",
        app: payload.app,
        operationId: payload.operationId,
        receiptRevision: 3,
        observedRevision: "shown-revision",
      };
    if (command === "vendor_review_connection")
      return {
        ...full(payload.operationId),
        status: "failedBeforeWrite",
        operationState: "failedBeforeWrite",
        receiptRevision: 4,
      };
    throw new Error(`unexpected IPC ${command}`);
  });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.catalog).toBeTruthy());
  act(() =>
    mock.confirm.onSwitch(false, { choice: "keep", revision: "catalog-1" }),
  );
  await waitFor(() => expect(mock.success).toHaveBeenCalledTimes(1));
  const calls = mock.invoke.mock.calls;
  const prepares = calls.filter(
    ([name]) => name === "vendor_prepare_connection",
  );
  expect(prepares).toHaveLength(2);
  expect(prepares[1][1].catalogDecision).toEqual({
    choice: "keep",
    revision: "catalog-1",
  });
  expect(prepares[1][1].operationId).not.toBe(firstId);
  const previewIndex = calls.findIndex(
    ([name]) => name === "vendor_preview_connection_review",
  );
  const retireIndex = calls.findIndex(
    ([name]) => name === "vendor_review_connection",
  );
  expect(previewIndex).toBeLessThan(retireIndex);
  expect(calls[retireIndex][1].observedRevision).toBe("shown-revision");
  expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
});

it("keeps catalog retry live when the replacement review returns unknown without throwing", async () => {
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({ status: "none" });
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(
    async (input) => full(input.operationId) as any,
  );
  vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Plan",
    quitRequired: false,
    catalog,
  });
  const review = vi
    .spyOn(vendorApi, "previewConnectionReview")
    .mockImplementationOnce(async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "shown",
    }))
    .mockResolvedValueOnce({ status: "unknown" })
    .mockImplementation(async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 3,
      observedRevision: "shown-again",
    }));
  const preview = vi
    .spyOn(vendorApi, "previewConnectionCatalog")
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog: { ...catalog, choice: "manage", revision: "new" },
    });
  setup();
  fireEvent.click(await screen.findByText("Use vendor"));
  await waitFor(() => expect(mock.confirm?.catalog).toBeTruthy());
  act(() =>
    mock.confirm.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }),
  );
  await waitFor(() => expect(mock.error).toHaveBeenCalled());
  expect(mock.confirm?.catalog?.revision).toBe("catalog-1");
  act(() =>
    mock.confirm.onPreviewCatalog({ choice: "manage", revision: "catalog-1" }),
  );
  await waitFor(() => expect(mock.confirm?.catalog?.revision).toBe("new"));
  expect(preview).toHaveBeenCalledTimes(2);
  expect(review).toHaveBeenCalledTimes(3);
});
