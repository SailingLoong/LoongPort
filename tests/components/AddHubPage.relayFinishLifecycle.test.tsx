import { createContext } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AddHubPage } from "@/components/relay/AddHubPage";
import { RelaySetupConfiguration } from "@/components/relay/onboarding/RelaySetupConfiguration";
import { PreservedView } from "@/components/ui/PreservedView";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import type { AppId } from "@/lib/api";

// The exact useRelaySetup implementation is intentionally mocked; these tests
// exercise ownership at the component boundary, not setup's state machine.
const harness = vi.hoisted(() => ({
  phase: "result",
  finish: undefined as (() => void) | undefined,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));
vi.mock("@/components/ui/button", () => ({
  Button: ({ children, onClick, ...props }: any) => {
    if (children === "结束并返回") harness.finish = onClick;
    return (
      <button onClick={onClick} {...props}>
        {children}
      </button>
    );
  },
}));
vi.mock("@/components/relay/accounts/QoderAccountPanel", () => ({
  QoderAccountPanel: () => null,
}));
vi.mock("@/components/relay/accounts/AccountWorkspace", () => ({
  AccountApplicationNavigation: createContext(null),
}));
vi.mock("@/components/relay/accounts/accountPlatforms", () => ({
  accountPlatforms: [],
  accountPlatformLabel: () => "",
}));
vi.mock("@/components/relay/accounts/serviceEntryIntent", () => ({
  serviceEntryIntent: () => null,
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: () => null,
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: ({ onConnected }: any) => (
    <button onClick={() => onConnected({ kind: "relay", rowId: 7, name: "A" })}>
      connect
    </button>
  ),
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => null,
}));
// This bridge keeps the integration probe narrow; it is not a substitute for
// exercising the real ServiceConfiguration and useRelaySetup in the full tree.
vi.mock("@/components/relay/onboarding/ServiceConfiguration", () => ({
  ServiceConfiguration: (props: any) => <RelaySetupConfiguration {...props} />,
}));
vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: () => null,
}));
vi.mock("@/components/relay/onboarding/useRelaySetup", () => ({
  useRelaySetup: () => ({
    busy: false,
    draft: {},
    canStartOperation: () => true,
    phase: harness.phase,
    discovery: { data: null },
    recovery: {},
    receipt: { targets: [], selections: [] },
    needsReconciliation: false,
  }),
}));

const status = { shouldPrompt: false, completed: false, plazaVisible: false };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  harness.phase = "result";
  harness.finish = undefined;
});

describe("relay finish lifecycle ownership", () => {
  it("does not exit a new explicit destination after dismiss resolves", async () => {
    const pending = deferred<typeof status>();
    vi.spyOn(serviceOnboardingApi, "dismiss").mockReturnValue(pending.promise);
    const onBack = vi.fn();
    const client = new QueryClient();
    const view = (id: number) => (
      <QueryClientProvider client={client}>
        <AddHubPage
          sourceAppId="codex"
          entryRequest={{ id, tab: id === 1 ? "directory" : "official" }}
          onBack={onBack}
          onAddProvider={vi.fn()}
        />
      </QueryClientProvider>
    );
    const rendered = render(view(1));
    fireEvent.click(screen.getByText("connect"));
    fireEvent.click(screen.getByText("结束并返回"));
    rendered.rerender(view(2));
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
    expect(onBack).not.toHaveBeenCalled();
    expect(screen.getByText("loongport.onboarding.resume")).toBeTruthy();
  });
  it("does not exit a newer source app after dismiss resolves", async () => {
    const pending = deferred<typeof status>();
    vi.spyOn(serviceOnboardingApi, "dismiss").mockReturnValue(pending.promise);
    const onBack = vi.fn();
    const client = new QueryClient();
    const view = (app: AppId) => (
      <QueryClientProvider client={client}>
        <AddHubPage
          sourceAppId={app}
          initialTab="directory"
          onBack={onBack}
          onAddProvider={vi.fn()}
        />
      </QueryClientProvider>
    );
    const rendered = render(view("codex"));
    fireEvent.click(screen.getByText("connect"));
    fireEvent.click(screen.getByText("结束并返回"));
    rendered.rerender(view("claude"));
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
    expect(onBack).not.toHaveBeenCalled();
  });
  it("rejects a retained finish command after hide and reveal", async () => {
    const dismiss = vi
      .spyOn(serviceOnboardingApi, "dismiss")
      .mockResolvedValue(status);
    const client = new QueryClient();
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
    const oldFinish = harness.finish!;
    rendered.rerender(view(false));
    rendered.rerender(view(true));
    await act(async () => {
      oldFinish();
    });
    expect(dismiss).not.toHaveBeenCalled();
  });
  it("rejects a retained result command after switching to review", async () => {
    const dismiss = vi
      .spyOn(serviceOnboardingApi, "dismiss")
      .mockResolvedValue(status);
    const client = new QueryClient();
    const view = () => (
      <QueryClientProvider client={client}>
        <RelaySetupConfiguration
          account={{ kind: "relay", rowId: 7, name: "A" }}
          sourceAppId="codex"
          onBack={vi.fn()}
          onDone={vi.fn()}
        />
      </QueryClientProvider>
    );
    const rendered = render(view());
    const oldFinish = harness.finish!;
    harness.phase = "review";
    rendered.rerender(view());
    await act(async () => {
      oldFinish();
    });
    expect(dismiss).not.toHaveBeenCalled();
  });
  it("finishes a dispatched dismiss and invalidation while hidden without exiting", async () => {
    const pending = deferred<typeof status>();
    vi.spyOn(serviceOnboardingApi, "dismiss").mockReturnValue(pending.promise);
    const onBack = vi.fn();
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const view = (active: boolean) => (
      <QueryClientProvider client={client}>
        <PreservedView active={active}>
          <AddHubPage
            sourceAppId="codex"
            initialTab="directory"
            onBack={onBack}
            onAddProvider={vi.fn()}
          />
        </PreservedView>
      </QueryClientProvider>
    );
    const rendered = render(view(true));
    fireEvent.click(screen.getByText("connect"));
    fireEvent.click(screen.getByText("结束并返回"));
    rendered.rerender(view(false));
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(onBack).not.toHaveBeenCalled();
  });
  it("dispatches once for synchronous duplicate finish calls", async () => {
    const pending = deferred<typeof status>();
    const dismiss = vi
      .spyOn(serviceOnboardingApi, "dismiss")
      .mockReturnValue(pending.promise);
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <RelaySetupConfiguration
          account={{ kind: "relay", rowId: 7, name: "A" }}
          sourceAppId="codex"
          onBack={vi.fn()}
          onDone={vi.fn()}
        />
      </QueryClientProvider>,
    );
    const finish = harness.finish!;
    act(() => {
      finish();
      finish();
    });
    expect(dismiss).toHaveBeenCalledTimes(1);
    await act(async () => {
      pending.resolve(status);
      await pending.promise;
    });
  });
});
