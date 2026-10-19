import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { proxyApi } from "@/lib/api/proxy";
import { ManagedAccountBootstrapSection } from "@/components/relay/ManagedAccountBootstrapSection";
import { ManagedAccountApplicationPanel } from "@/components/relay/accounts/ManagedAccountApplicationPanel";
import type { Provider } from "@/types";
import type {
  ApplicationRouting,
  ApplicationRoutingRequest,
} from "@/lib/api/applicationRouting";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_: string, o: { defaultValue: string }) => o.defaultValue,
  }),
}));
vi.mock("@/components/settings/ManagedAccountProviderSection", () => ({
  ManagedAccountProviderSection: () => null,
}));
vi.mock("@/components/providers/forms/MiniMaxCodeSection", () => ({
  MiniMaxCodeSection: () => null,
}));
vi.mock("@/components/providers/forms/CopilotAuthSection", () => ({
  CopilotAuthSection: () => null,
}));
vi.mock("@/components/providers/forms/CodexOAuthSection", () => ({
  CodexOAuthSection: () => null,
}));
vi.mock("@/components/providers/forms/XaiOAuthSection", () => ({
  XaiOAuthSection: () => null,
}));
vi.mock("@/components/relay/RestoreOfficialLoginButton", () => ({
  RestoreOfficialLoginButton: () => null,
}));
vi.mock("@/components/icons/GithubIcon", () => ({ GithubIcon: () => null }));
vi.mock("@/components/BrandIcons", () => ({ CodexIcon: () => null }));
vi.mock("@/components/ProviderIcon", () => ({ ProviderIcon: () => null }));
vi.mock("@/components/ui/badge", () => ({
  Badge: ({ children }: { children: React.ReactNode }) => (
    <span>{children}</span>
  ),
}));

const row = (id: string, platform = "zcode_start_plan"): Provider => ({
  id,
  name: `Synthetic ${id}`,
  settingsConfig: { env: { ANTHROPIC_MODEL: "synthetic-model" } },
  meta: {
    authBinding: {
      source: "managed_account",
      authProvider: platform,
      accountId: `account-${id}`,
    },
  },
});
const routing = (): ApplicationRouting => ({
  modeState: {
    status: "ready",
    mode: "direct",
    attached: false,
    currentProviderId: null,
    directProviderId: null,
    publicationStarted: null,
    canWrite: true,
    canRecheck: true,
  },
  autoFailoverEnabled: false,
  routingActive: false,
  model: null,
  modelOptions: [],
  chainIds: ["first", "second"],
  tiers: [],
});
const receipt = (id = "request"): ApplicationRoutingRequest => ({
  id,
  scope: "applicationRouting",
  draftDigest: "a".repeat(64),
  revision: "b".repeat(64),
});
let view: ApplicationRouting;
let status: string;
let prepare: (
  args: Record<string, unknown>,
) => Promise<ApplicationRoutingRequest>;
let apply: (args: Record<string, unknown>) => Promise<void>;
let getRows: (app: string) => Promise<Record<string, Provider>>;
let queryFailure: boolean;
const calls = (name: string) =>
  vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
function mount(full = false) {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      {full ? (
        <ManagedAccountApplicationPanel />
      ) : (
        <ManagedAccountBootstrapSection />
      )}
    </QueryClientProvider>,
  );
}
async function select(app = "claude", id = "first") {
  fireEvent.change(screen.getByLabelText("首次启用的应用"), {
    target: { value: app },
  });
  await screen.findByRole("option", { name: `Synthetic ${id}` });
  fireEvent.change(screen.getByLabelText("已保存的账号档位"), {
    target: { value: id },
  });
}
const enable = () =>
  screen.getByRole("button", { name: "启用本地路由并使用此账号" });
beforeEach(() => {
  vi.clearAllMocks();
  view = routing();
  status = "notRecorded";
  queryFailure = false;
  getRows = async () => ({
    first: row("first"),
    second: row("second", "minimax_code"),
  });
  prepare = async (args) => receipt(args.requestId as string);
  apply = async (args) => {
    const initial = args.initialRoute as { providerId: string };
    view = {
      ...view,
      routingActive: true,
      modeState: {
        ...view.modeState!,
        mode: "proxy",
        attached: true,
        currentProviderId: initial.providerId,
      },
    };
    status = "completed";
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    if (command === "get_providers") return getRows(a.app as string);
    if (command === "get_application_routing") return view;
    if (command === "prepare_proxy_takeover_request") return prepare(a);
    if (command === "set_proxy_takeover_for_app") return apply(a);
    if (command === "get_application_routing_request_result") {
      if (queryFailure) throw new Error("synthetic query unavailable");
      return { request: a.request, status };
    }
    throw new Error(`Unexpected command: ${command}`);
  });
});

describe("original proxy takeover API", () => {
  it("forwards explicit prepared identity while preserving the legacy bool call", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    const initialRoute = { providerId: "chosen", request: receipt() };
    await proxyApi.setProxyTakeoverForApp("claude", true, initialRoute);
    expect(invoke).toHaveBeenLastCalledWith("set_proxy_takeover_for_app", {
      appType: "claude",
      enabled: true,
      initialRoute,
    });
    await proxyApi.setProxyTakeoverForApp("claude", false);
    expect(invoke).toHaveBeenLastCalledWith("set_proxy_takeover_for_app", {
      appType: "claude",
      enabled: false,
    });
  });
});
describe("explicit empty-app bootstrap", () => {
  it("is mounted in the application account panel and selects nothing by default", () => {
    mount(true);
    expect(
      screen.getByRole("region", { name: "首次启用账号路由" }),
    ).toBeInTheDocument();
    expect(enable()).toBeDisabled();
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
  });
  it("requires both explicit selections, then uses original prepare, takeover, receipt and active-route queries", async () => {
    mount();
    fireEvent.change(screen.getByLabelText("首次启用的应用"), {
      target: { value: "claude" },
    });
    await screen.findByRole("option", { name: "Synthetic second" });
    expect(enable()).toBeDisabled();
    fireEvent.change(screen.getByLabelText("已保存的账号档位"), {
      target: { value: "second" },
    });
    fireEvent.click(enable());
    await screen.findByText(/claude: Synthetic second · 已启用本地路由/);
    expect(calls("prepare_proxy_takeover_request")[0][1]).toMatchObject({
      appType: "claude",
      providerId: "second",
    });
    expect(calls("set_proxy_takeover_for_app")[0][1]).toMatchObject({
      appType: "claude",
      enabled: true,
      initialRoute: { providerId: "second" },
    });
    expect(calls("add_provider")).toHaveLength(0);
    expect(calls("switch_provider")).toHaveLength(0);
    expect(JSON.stringify(calls("set_proxy_takeover_for_app"))).not.toContain(
      "account-second",
    );
  });
  it("supports a saved WorkBuddy binding without treating it as a new authorization or model discovery", async () => {
    getRows = async () => ({ wb: row("wb", "workbuddy") });
    mount();
    await select("codex", "wb");
    fireEvent.click(enable());
    await screen.findByText(/codex: Synthetic wb · 已启用本地路由/);
    expect(calls("add_provider")).toHaveLength(0);
  });
  it("rejects unknown, pending, existing direct and existing proxy views", async () => {
    for (const mode of [
      { ...routing().modeState!, status: "unknown" as const, canWrite: false },
      { ...routing().modeState!, status: "pending" as const, canWrite: false },
      {
        ...routing().modeState!,
        directProviderId: "native",
        currentProviderId: "native",
      },
      {
        ...routing().modeState!,
        mode: "proxy" as const,
        attached: true,
        currentProviderId: "first",
      },
    ]) {
      view = { ...routing(), modeState: mode };
      const mounted = mount();
      fireEvent.change(screen.getByLabelText("首次启用的应用"), {
        target: { value: "claude" },
      });
      await screen.findByText(/此应用已有配置或状态待核对/);
      expect(enable()).toBeDisabled();
      mounted.unmount();
    }
    expect(calls("prepare_proxy_takeover_request")).toHaveLength(0);
  });
  it("ignores late app rows when the user chooses another app", async () => {
    let finish!: (rows: Record<string, Provider>) => void;
    getRows = (app) =>
      app === "claude"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : Promise.resolve({ second: row("second") });
    mount();
    fireEvent.change(screen.getByLabelText("首次启用的应用"), {
      target: { value: "claude" },
    });
    fireEvent.change(screen.getByLabelText("首次启用的应用"), {
      target: { value: "codex" },
    });
    await screen.findByRole("option", { name: "Synthetic second" });
    finish({ first: row("first") });
    await waitFor(() =>
      expect(
        screen.queryByRole("option", { name: "Synthetic first" }),
      ).not.toBeInTheDocument(),
    );
    expect(enable()).toBeDisabled();
  });
  it("rejects changed account or model rows after prepare instead of applying a stale selection", async () => {
    for (const field of ["account", "model"]) {
      let reads = 0;
      getRows = async () => {
        const actual = row("first");
        if (++reads > 1) {
          if (field === "account")
            actual.meta!.authBinding!.accountId = "replacement";
          else
            actual.settingsConfig = {
              env: { ANTHROPIC_MODEL: "changed-model" },
            };
        }
        return { first: actual };
      };
      const mounted = mount();
      await select();
      fireEvent.click(enable());
      await screen.findByText(/准备失败，应用可能已变化/);
      expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
      mounted.unmount();
    }
  });

  it("does not apply a prepared request after unmount", async () => {
    let finish!: (r: ApplicationRoutingRequest) => void;
    prepare = () =>
      new Promise((resolve) => {
        finish = resolve;
      });
    const mounted = mount();
    await select();
    fireEvent.click(enable());
    await waitFor(() =>
      expect(calls("prepare_proxy_takeover_request")).toHaveLength(1),
    );
    mounted.unmount();
    finish(receipt());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
  });
  it("double click issues only one apply and unknown completion remains query-only", async () => {
    apply = async () => {
      queryFailure = true;
      throw new Error("synthetic lost reply");
    };
    mount();
    await select();
    fireEvent.click(enable());
    fireEvent.click(enable());
    await screen.findByRole("button", { name: "查询启用结果" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "查询启用结果" }),
      ).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "查询启用结果" }));
    await screen.findByText(/结果暂时无法查询/);
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(1);
    expect(calls("add_provider")).toHaveLength(0);
  });
  it("a failed first start can be explicitly re-prepared without another provider save", async () => {
    let fail = true;
    const success = apply;
    apply = async (args) => {
      if (fail) throw new Error("synthetic occupied port");
      await success(args);
    };
    mount();
    await select();
    fireEvent.click(enable());
    await screen.findByRole("button", { name: "重新核对并选择" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "重新核对并选择" }),
      ).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "重新核对并选择" }));
    await screen.findByRole("option", { name: "Synthetic first" });
    fireEvent.change(screen.getByLabelText("已保存的账号档位"), {
      target: { value: "first" },
    });
    fail = false;
    fireEvent.click(enable());
    await screen.findByText(/已启用本地路由/);
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(2);
    const preparations = calls("prepare_proxy_takeover_request");
    expect(preparations).toHaveLength(2);
    expect(preparations[0][1]).not.toEqual(preparations[1][1]);
    expect(calls("add_provider")).toHaveLength(0);
  });
  it("a completed receipt does not claim success after another window changed the active route", async () => {
    apply = async () => {
      status = "completed";
      view = {
        ...view,
        routingActive: true,
        modeState: {
          ...view.modeState!,
          mode: "proxy",
          attached: true,
          currentProviderId: "other",
        },
      };
    };
    mount();
    await select();
    fireEvent.click(enable());
    await screen.findByText(/尚未确认启用/);
    expect(
      screen.queryByText(/Synthetic first · 已启用本地路由/),
    ).not.toBeInTheDocument();
  });
  it("two UI windows submit their own identities and the losing CAS cannot report success", async () => {
    let active: string | null = null;
    apply = async (args) => {
      if (active) throw new Error("synthetic revision conflict");
      active = (args.initialRoute as { providerId: string }).providerId;
      view = {
        ...view,
        routingActive: true,
        modeState: {
          ...view.modeState!,
          mode: "proxy",
          attached: true,
          currentProviderId: active,
        },
      };
      status = "completed";
    };
    const first = mount();
    const second = mount();
    for (const [mounted, id] of [
      [first, "first"],
      [second, "second"],
    ] as const) {
      const ui = within(mounted.container);
      fireEvent.change(ui.getByLabelText("首次启用的应用"), {
        target: { value: "claude" },
      });
      await ui.findByRole("option", { name: `Synthetic ${id}` });
      fireEvent.change(ui.getByLabelText("已保存的账号档位"), {
        target: { value: id },
      });
    }
    fireEvent.click(
      within(first.container).getByRole("button", {
        name: "启用本地路由并使用此账号",
      }),
    );
    fireEvent.click(
      within(second.container).getByRole("button", {
        name: "启用本地路由并使用此账号",
      }),
    );
    await within(first.container).findByText(
      /Synthetic first · 已启用本地路由/,
    );
    await within(second.container).findByText(/尚未确认启用/);
    expect(calls("add_provider")).toHaveLength(0);
  });
});
