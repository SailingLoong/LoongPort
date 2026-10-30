import { StrictMode } from "react";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { RelaySetupConfiguration } from "@/components/relay/onboarding/RelaySetupConfiguration";
import { relayApi } from "@/lib/api/relay";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";

const callbacks = vi.hoisted(() => ({
  finish: undefined as (() => void) | undefined,
  retry: undefined as (() => void) | undefined,
  reconcile: undefined as (() => void) | undefined,
  skip: undefined as (() => void) | undefined,
  refresh: undefined as (() => void) | undefined,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));
vi.mock("@/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
    variant: _variant,
    size: _size,
    ...props
  }: any) => {
    if (children === "结束并返回") callbacks.finish = onClick;
    if (children === "仅重试未完成配置") callbacks.retry = onClick;
    if (children === "核对原操作") callbacks.reconcile = onClick;
    if (children === "暂不接入") callbacks.skip = onClick;
    if (children === "重新读取") callbacks.refresh = onClick;
    return (
      <button onClick={onClick} {...props}>
        {children}
      </button>
    );
  },
}));
vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: () => null,
}));
vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    discoverTiers: vi.fn(),
    getSetupOperation: vi.fn(),
    reconcileSetupOperation: vi.fn(),
  },
}));
vi.mock("@/lib/api/relaySetup", () => ({
  isRelayPreparationRejection: () => false,
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: { dismiss: vi.fn() },
}));

const status = { shouldPrompt: false, completed: false, plazaVisible: false };
const discovery = {
  schemaVersion: 1,
  relayId: 7,
  revision: "d1",
  account: {
    identity: "account-7",
    accountLabel: "A",
    siteOrigin: "https://example.test",
  },
  choices: [],
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function mount(onDone: () => void, client: QueryClient) {
  return render(
    <QueryClientProvider client={client}>
      <RelaySetupConfiguration
        account={{ kind: "relay", rowId: 7, name: "A" }}
        sourceAppId="codex"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
}
afterEach(() => {
  vi.resetAllMocks();
  callbacks.finish = undefined;
  callbacks.retry = undefined;
  callbacks.reconcile = undefined;
  callbacks.skip = undefined;
  callbacks.refresh = undefined;
});

describe("relay finish and recovered setup ownership", () => {
  it("does not exit a recovered operation when a pending skip completes", async () => {
    const pending = deferred<typeof status>();
    vi.mocked(serviceOnboardingApi.dismiss).mockReturnValue(pending.promise);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
    vi.mocked(relayApi.getSetupOperation)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(receipt as any);
    const onDone = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    mount(onDone, client);
    await screen.findByText("未能完成读取；已有操作结果仍需核对。");
    fireEvent.click(await screen.findByRole("button", { name: "暂不接入" }));
    fireEvent.click(screen.getAllByRole("button", { name: "重新读取" })[0]);
    await waitFor(() =>
      expect(relayApi.getSetupOperation).toHaveBeenCalledTimes(2),
    );
    await screen.findByText("op-7");
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(onDone).not.toHaveBeenCalled();
  });

  it("does not exit a retry review started in the same turn as finish", async () => {
    const pending = deferred<typeof status>();
    vi.mocked(serviceOnboardingApi.dismiss).mockReturnValue(pending.promise);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
      targets: [
        {
          app: "codex",
          groupId: "g1",
          providerId: "provider-1",
          configuration: "failed",
          connection: { state: "notRequested" },
          compatibility: "supported",
        },
      ],
      selections: [{ app: "codex", groupId: "g1" }],
    };
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
    const onDone = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    mount(onDone, client);
    await screen.findByRole("button", { name: "仅重试未完成配置" });
    const finish = callbacks.finish!;
    const retry = callbacks.retry!;
    act(() => {
      finish();
      retry();
    });
    await screen.findByText("本次准备范围");
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(onDone).not.toHaveBeenCalled();
  });
  it("fences retry intent before the new surface commits", async () => {
    const pending = deferred<typeof status>();
    vi.mocked(serviceOnboardingApi.dismiss).mockReturnValue(pending.promise);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
      targets: [
        {
          app: "codex",
          groupId: "g1",
          providerId: "provider-1",
          configuration: "failed",
          connection: { state: "notRequested" },
          compatibility: "supported",
        },
      ],
      selections: [{ app: "codex", groupId: "g1" }],
    };
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
    const onDone = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    vi.spyOn(client, "invalidateQueries").mockResolvedValue();
    mount(onDone, client);
    await screen.findByRole("button", { name: "仅重试未完成配置" });
    const finish = callbacks.finish!;
    const retry = callbacks.retry!;
    await act(async () => {
      finish();
      retry();
      pending.resolve(status);
      await pending.promise;
      await Promise.resolve();
      await Promise.resolve();
      expect(onDone).not.toHaveBeenCalled();
    });
  });

  it("does not dispatch reconcile while finish is pending", async () => {
    const pending = deferred<typeof status>();
    vi.mocked(serviceOnboardingApi.dismiss).mockReturnValue(pending.promise);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
    const reconcilePending = deferred<any>();
    vi.mocked(relayApi.reconcileSetupOperation).mockReturnValue(
      reconcilePending.promise,
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    mount(vi.fn(), client);
    await screen.findByText("op-7");
    const finish = callbacks.finish!;
    const reconcile = callbacks.reconcile!;
    act(() => {
      finish();
      reconcile();
    });
    const calls = vi.mocked(relayApi.reconcileSetupOperation).mock.calls.length;
    await act(async () => {
      pending.resolve(status);
      reconcilePending.resolve(receipt);
      await Promise.all([pending.promise, reconcilePending.promise]);
    });
    expect(calls).toBe(0);
  });

  it("does not dispatch finish while reconcile is pending", async () => {
    const pending = deferred<any>();
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
    vi.mocked(relayApi.reconcileSetupOperation).mockReturnValue(
      pending.promise,
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    mount(vi.fn(), client);
    await screen.findByText("op-7");
    const finish = callbacks.finish!;
    const reconcile = callbacks.reconcile!;
    act(() => {
      reconcile();
      finish();
    });
    expect(serviceOnboardingApi.dismiss).not.toHaveBeenCalled();
    await act(async () => {
      pending.resolve(receipt);
      await pending.promise;
    });
  });

  it("releases a hidden settled reconcile and allows finish after reveal", async () => {
    const pending = deferred<any>();
    vi.mocked(serviceOnboardingApi.dismiss).mockResolvedValue(status);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
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
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(receipt as any);
    vi.mocked(relayApi.reconcileSetupOperation).mockReturnValue(
      pending.promise,
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const view = (active: boolean) => (
      <QueryClientProvider client={client}>
        <PreservedView active={active}>
          <RelaySetupConfiguration
            account={{ kind: "relay", rowId: 7, name: "A" }}
            sourceAppId="codex"
            onBack={vi.fn()}
            onDone={vi.fn()}
          />
        </PreservedView>
      </QueryClientProvider>
    );
    const rendered = render(view(true));
    await screen.findByText("op-7");
    fireEvent.click(screen.getByRole("button", { name: "核对原操作" }));
    rendered.rerender(view(false));
    await act(async () => {
      pending.resolve(receipt);
      await pending.promise;
    });
    rendered.rerender(view(true));
    await screen.findByText("op-7");
    fireEvent.click(screen.getByRole("button", { name: "结束并返回" }));
    await act(async () => {});
    expect(relayApi.reconcileSetupOperation).toHaveBeenCalledTimes(1);
    expect(serviceOnboardingApi.dismiss).toHaveBeenCalledTimes(1);
  });

  it("blocks a retained finish while an earlier refresh is in flight", async () => {
    const pending = deferred<any>();
    const loaded = {
      ...discovery,
      choices: [
        {
          app: "codex",
          groupId: "g",
          groupName: "G",
          providerId: "provider",
          localConfiguration: "absent",
          compatibility: "unknown",
          keyStatus: "unknown",
        },
      ],
    };
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(loaded as any);
    vi.mocked(relayApi.getSetupOperation)
      .mockRejectedValueOnce(new Error("offline"))
      .mockReturnValue(pending.promise);
    vi.mocked(serviceOnboardingApi.dismiss).mockResolvedValue(status);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    mount(vi.fn(), client);
    await screen.findByText("未能完成读取；已有操作结果仍需核对。");
    await screen.findByRole("button", { name: "暂不接入" });
    const finish = callbacks.skip!;
    const refresh = callbacks.refresh!;
    act(() => {
      refresh();
      finish();
    });
    const calls = vi.mocked(serviceOnboardingApi.dismiss).mock.calls.length;
    await act(async () => {
      pending.resolve(null);
      await pending.promise;
    });
    expect(calls).toBe(0);
  });

  it("does not reauthorize an old finish before a completed refresh commits", async () => {
    const pending = deferred<any>();
    const loaded = {
      ...discovery,
      choices: [
        {
          app: "codex",
          groupId: "g",
          groupName: "G",
          providerId: "provider",
          localConfiguration: "absent",
          compatibility: "unknown",
          keyStatus: "unknown",
        },
      ],
    };
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(loaded as any);
    vi.mocked(relayApi.getSetupOperation)
      .mockRejectedValueOnce(new Error("offline"))
      .mockReturnValue(pending.promise);
    vi.mocked(serviceOnboardingApi.dismiss).mockResolvedValue(status);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    mount(vi.fn(), client);
    await screen.findByText("未能完成读取；已有操作结果仍需核对。");
    await screen.findByRole("button", { name: "暂不接入" });
    const finish = callbacks.skip!;
    const refresh = callbacks.refresh!;
    await act(async () => {
      refresh();
      pending.resolve(null);
      await pending.promise;
      for (let index = 0; index < 10; index++) await Promise.resolve();
      finish();
      expect(serviceOnboardingApi.dismiss).not.toHaveBeenCalled();
    });
  });

  it("StrictMode reveal rejects the old finish and allows a new explicit finish", async () => {
    vi.mocked(serviceOnboardingApi.dismiss).mockResolvedValue(status);
    vi.mocked(relayApi.discoverTiers).mockResolvedValue(discovery as any);
    vi.mocked(relayApi.getSetupOperation).mockResolvedValue(null);
    const onDone = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const view = (active: boolean) => (
      <StrictMode>
        <QueryClientProvider client={client}>
          <PreservedView active={active}>
            <RelaySetupConfiguration
              account={{ kind: "relay", rowId: 7, name: "A" }}
              sourceAppId="codex"
              onBack={vi.fn()}
              onDone={onDone}
            />
          </PreservedView>
        </QueryClientProvider>
      </StrictMode>
    );
    const rendered = render(view(true));
    await screen.findByRole("button", { name: "暂不接入" });
    const oldFinish = callbacks.skip!;
    rendered.rerender(view(false));
    rendered.rerender(view(true));
    await screen.findByRole("button", { name: "暂不接入" });
    await act(async () => {
      oldFinish();
    });
    expect(serviceOnboardingApi.dismiss).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "暂不接入" }));
    await act(async () => {});
    expect(serviceOnboardingApi.dismiss).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledTimes(1);
  });
});
