import { createTestQueryClient } from "../utils/testQueryClient";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ManagedAccountProviderSection } from "@/components/settings/ManagedAccountProviderSection";
import { ManagedAccountApplicationPanel } from "@/components/relay/accounts/ManagedAccountApplicationPanel";
import { minimaxCodeApi } from "@/lib/api/minimaxCode";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
import { traeAccountsApi, type TraeAccountView } from "@/lib/api/traeAccounts";
import type { Provider } from "@/types";
import {
  buildManagedAccountProvider,
  managedAccountProviderId,
} from "@/lib/managedAccountProvider";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options: { defaultValue: string }) =>
      options.defaultValue,
  }),
}));
vi.mock("@/lib/api/minimaxCode", () => ({ minimaxCodeApi: { list: vi.fn() } }));
vi.mock("@/lib/api/zcodeAccounts", () => ({
  zcodeAccountsApi: { library: vi.fn() },
}));
vi.mock("@/lib/api/traeAccounts", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/lib/api/traeAccounts")>();
  return {
    ...actual,
    traeAccountsApi: { ...actual.traeAccountsApi, list: vi.fn() },
  };
});
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: {
    list: vi.fn(),
    listInferenceAccounts: vi.fn(),
    discoverModels: vi.fn(),
  },
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

const mini = {
  id: "same-opaque-id",
  label: "Synthetic MiniMax",
  authorized: true,
  status: "unknown",
  accessToken: "must-not-copy",
  realUserId: "must-not-copy",
};
const zcode = {
  revision: "r1",
  pending: false,
  profiles: [
    {
      id: "same-opaque-id",
      label: "Synthetic ZCode",
      family: "zai",
      capabilities: null,
      secret: "must-not-copy",
    },
  ],
};
function mount() {
  return render(
    <QueryClientProvider client={queryClient}>
      <ManagedAccountProviderSection />
    </QueryClientProvider>,
  );
}
async function choose(name = "Synthetic MiniMax") {
  const option = await screen.findByRole("option", { name: new RegExp(name) });
  fireEvent.change(screen.getByLabelText("账号"), {
    target: { value: (option as HTMLOptionElement).value },
  });
  fireEvent.change(screen.getByLabelText("目标应用"), {
    target: { value: "claude" },
  });
  fireEvent.change(screen.getByLabelText("官方模型 ID"), {
    target: { value: "synthetic-model" },
  });
}
let queryClient: ReturnType<typeof createTestQueryClient>;
let rows: Record<string, Provider>;
let chain: string[];
let appendOnSave: boolean;
let discoverySerial: number;
let discoveredRequests: Map<string, string>;
const traeModels = [
  { id: "confirmed-global-model", name: "Discovered TRAE model" },
];
const nativeCalls = (name: string) =>
  vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
const expectNoProviderWrite = () =>
  expect(nativeCalls("add_provider")).toHaveLength(0);
async function originalCommand(
  command: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  if (command === "begin_trae_model_discovery") {
    const requestId = `synthetic-discovery-${++discoverySerial}`;
    discoveredRequests.set(requestId, args!.accountId as string);
    return { requestId };
  }
  if (command === "list_trae_discovered_models") {
    if (!discoveredRequests.has(args!.requestId as string))
      throw new Error("unknown synthetic request");
    return traeModels;
  }
  if (command === "cancel_trae_model_discovery") return;
  if (command === "get_providers") return { ...rows };
  if (command === "add_provider") {
    const p = args!.provider as Provider;
    if (rows[p.id]) throw new Error("existing provider");
    rows[p.id] = p;
    if (appendOnSave) chain.push(p.id);
    return true;
  }
  if (command === "get_application_routing") return { chainIds: [...chain] };
  if (command === "get_order_profiles")
    return { current: "default", profiles: [] };
  if (command === "prepare_application_routing_request")
    return {
      id: args!.requestId,
      scope: "applicationRouting",
      draftDigest: "synthetic",
      revision: "synthetic-revision",
    };
  if (command === "apply_application_routing") {
    chain = [
      ...(args!.change as { order: { providerIds: string[] } }).order
        .providerIds,
    ];
    return { status: "switched" };
  }
  if (command === "get_application_routing_request_result")
    return { status: "completed" };
  throw new Error(`Unexpected command ${command}`);
}
beforeEach(() => {
  queryClient = createTestQueryClient();
  vi.clearAllMocks();
  rows = {};
  chain = [];
  appendOnSave = true;
  discoverySerial = 0;
  discoveredRequests = new Map();
  vi.mocked(minimaxCodeApi.list).mockResolvedValue([mini] as never);
  vi.mocked(zcodeAccountsApi.library).mockResolvedValue(zcode as never);
  vi.mocked(traeAccountsApi.list).mockResolvedValue([]);
  vi.mocked(workbuddyApi.list).mockResolvedValue([]);
  vi.mocked(workbuddyApi.listInferenceAccounts).mockResolvedValue([]);
  vi.mocked(workbuddyApi.discoverModels).mockResolvedValue([]);
  vi.mocked(invoke).mockImplementation(originalCommand as never);
});
describe("explicit account to original provider entry", () => {
  it("is mounted in the application account panel", async () => {
    render(
      <QueryClientProvider client={queryClient}>
        <ManagedAccountApplicationPanel />
      </QueryClientProvider>,
    );
    await screen.findByRole("option", { name: /Synthetic MiniMax/ });
    expect(
      screen.getByRole("heading", { name: "将账号加入应用路由链" }),
    ).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("reads existing catalogs without selecting, saving, logging in, or switching", async () => {
    mount();
    await screen.findByRole("option", { name: /Synthetic ZCode/ });
    expect(zcodeAccountsApi.library).toHaveBeenCalledWith();
    expect(screen.getByLabelText("账号")).toHaveValue("");
    expect(screen.getByLabelText("目标应用")).toHaveValue("");
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each([
    ["Synthetic MiniMax", "minimax_code"],
    ["Synthetic ZCode", "zcode_start_plan"],
  ])(
    "saves %s through the existing add command, without live activation",
    async (label, authProvider) => {
      mount();
      await choose(label);
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith(
          "add_provider",
          expect.objectContaining({ app: "claude", addToLive: false }),
        ),
      );
      const payload = vi
        .mocked(invoke)
        .mock.calls.find(([command]) => command === "add_provider")![1] as {
        provider: { meta: unknown };
      };
      expect(payload.provider.meta).toMatchObject({
        authBinding: {
          source: "managed_account",
          authProvider,
          accountId: "same-opaque-id",
        },
        apiFormat: "anthropic",
      });
      expect(JSON.stringify(payload)).not.toContain("must-not-copy");
      expect(await screen.findByRole("status")).toHaveTextContent(
        "已加入应用路由链",
      );
      expect(screen.getByRole("status")).toHaveTextContent("未切换当前配置");
    },
  );
  it("re-reads the selected owner and refuses a retired account before saving", async () => {
    mount();
    await choose();
    vi.mocked(minimaxCodeApi.list).mockResolvedValue([]);
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("alert");
    expect(invoke).not.toHaveBeenCalled();
  });
  it("does not allow repeated saves while a command is pending", async () => {
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "add_provider")
        await new Promise((done) => {
          resolve = done;
        });
      return originalCommand(command, args as Record<string, unknown>);
    });
    mount();
    await choose();
    const button = screen.getByRole("button", { name: "加入应用路由链" });
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
      ).toHaveLength(1),
    );
    resolve(true);
    await screen.findByRole("status");
  });
  it("marks uncertain saves for a read-only recheck instead of resubmitting", async () => {
    let attempted = false;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "add_provider") {
        attempted = true;
        throw new Error("lost reply");
      }
      if (attempted) throw new Error("owner unavailable");
      return originalCommand(command, args as Record<string, unknown>);
    });
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("button", { name: "核对保存结果" });
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
  it("repeated confirmation and remount reuse the same original row", async () => {
    const first = mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
    first.unmount();
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
    expect(Object.keys(rows)).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
  it("does not replace or report success for a mismatched stable-id row", async () => {
    const account = {
      ...mini,
      available: true,
      authProvider: "minimax_code" as const,
    };
    const id = await managedAccountProviderId(
      account,
      "claude",
      "synthetic-model",
    );
    rows[id] = buildManagedAccountProvider(
      account,
      "claude",
      "different-model",
      id,
    );
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("未覆盖原记录");
    expect(screen.queryByRole("status")).toBeNull();
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(0);
  });
  it("keeps a successful row with failed chain append separate and retries only original order CAS", async () => {
    appendOnSave = false;
    chain = ["existing-route"];
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "已保存，加入路由待恢复",
    );
    expect(screen.queryByRole("status")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "重试加入路由链" }));
    await screen.findByRole("status");
    const created = Object.keys(rows)[0];
    expect(chain).toEqual(["existing-route", created]);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
    expect(invoke).toHaveBeenCalledWith(
      "apply_application_routing",
      expect.objectContaining({
        change: {
          order: {
            profileName: "default",
            providerIds: ["existing-route", created],
          },
        },
        request: expect.objectContaining({ revision: "synthetic-revision" }),
      }),
    );
  });
  it("never applies a pre-prepare chain snapshot after concurrent reordering", async () => {
    appendOnSave = false;
    chain = ["a", "b"];
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      const result = await originalCommand(
        command,
        args as Record<string, unknown>,
      );
      if (command === "prepare_application_routing_request")
        chain = ["b", "a", "concurrent-add"];
      return result;
    });
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "重试加入路由链" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "prepare_application_routing_request",
        expect.anything(),
      ),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "重试加入路由链" }),
      ).not.toBeDisabled(),
    );
    expect(chain).toEqual(["b", "a", "concurrent-add"]);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([cmd]) => cmd === "apply_application_routing"),
    ).toHaveLength(0);
  });

  it("two mounted windows competing for one identity cannot duplicate or overwrite its row", async () => {
    let reads = 0;
    let release!: () => void;
    const barrier = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_providers" && ++reads <= 2) {
        if (reads === 2) release();
        await barrier;
        return {};
      }
      return originalCommand(command, args as Record<string, unknown>);
    });
    const first = mount();
    const second = mount();
    for (const view of [first, second]) {
      const scope = within(view.container);
      const option = await scope.findByRole("option", {
        name: /Synthetic MiniMax/,
      });
      fireEvent.change(scope.getByLabelText("账号"), {
        target: { value: (option as HTMLOptionElement).value },
      });
      fireEvent.change(scope.getByLabelText("目标应用"), {
        target: { value: "claude" },
      });
      fireEvent.change(scope.getByLabelText("官方模型 ID"), {
        target: { value: "synthetic-model" },
      });
      fireEvent.click(scope.getByRole("button", { name: "加入应用路由链" }));
    }
    await waitFor(() => expect(screen.getAllByRole("status")).toHaveLength(2));
    expect(Object.keys(rows)).toHaveLength(1);
    expect(chain).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(2);
  });
  it("a committed save with lost reply and unavailable recheck is never resubmitted after remount", async () => {
    let committed = false;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "add_provider") {
        await originalCommand(command, args as Record<string, unknown>);
        committed = true;
        throw new Error("reply lost");
      }
      if (committed) throw new Error("read temporarily unavailable");
      return originalCommand(command, args as Record<string, unknown>);
    });
    const view = mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("button", { name: "核对保存结果" });
    view.unmount();
    vi.mocked(invoke).mockImplementation(originalCommand as never);
    mount();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
    expect(Object.keys(rows)).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
  it.each([
    {
      usage_script: {
        enabled: false,
        language: "javascript" as const,
        code: "synthetic",
      },
    },
    {
      custom_endpoints: {
        "https://other.invalid": { url: "https://other.invalid", addedAt: 1 },
      },
    },
    { localProxyRequestOverrides: { body: { model: "other-model" } } },
  ])(
    "refuses actual original getAll rows with behavior-changing metadata %j",
    async (metadata) => {
      const account = {
        ...mini,
        available: true,
        authProvider: "minimax_code" as const,
      };
      const id = await managedAccountProviderId(
        account,
        "claude",
        "synthetic-model",
      );
      const provider = buildManagedAccountProvider(
        account,
        "claude",
        "synthetic-model",
        id,
      );
      rows[id] = { ...provider, meta: { ...provider.meta, ...metadata } };
      mount();
      await choose();
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "未覆盖原记录",
      );
      expect(screen.queryByRole("status")).toBeNull();
      expect(
        vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
      ).toHaveLength(0);
    },
  );
});

const wb = {
  id: "same-opaque-id",
  label: "Synthetic WorkBuddy",
  canRefresh: true,
  canClaim: true,
};
const wbModels = [
  { id: "synthetic-wb-model", name: "Synthetic WorkBuddy Model" },
];
function workbuddyReady(state = "ready") {
  vi.mocked(workbuddyApi.list).mockResolvedValue([wb] as never);
  vi.mocked(workbuddyApi.listInferenceAccounts).mockResolvedValue([
    { id: wb.id, state },
  ] as never);
  vi.mocked(workbuddyApi.discoverModels).mockResolvedValue(wbModels);
}
async function selectWorkbuddy(app = "claude") {
  const option = await screen.findByRole("option", {
    name: /Synthetic WorkBuddy/,
  });
  fireEvent.change(screen.getByLabelText("账号"), {
    target: { value: (option as HTMLOptionElement).value },
  });
  fireEvent.change(screen.getByLabelText("目标应用"), {
    target: { value: app },
  });
}
async function discoverWorkbuddy() {
  fireEvent.click(screen.getByRole("button", { name: "获取 WorkBuddy 模型" }));
  await screen.findByRole("option", { name: "Synthetic WorkBuddy Model" });
  fireEvent.change(screen.getByLabelText("官方模型 ID"), {
    target: { value: "synthetic-wb-model" },
  });
}
describe("WorkBuddy original account and live model consumer", () => {
  it("joins inference status by exact original row id and rejects foreign status", async () => {
    workbuddyReady();
    vi.mocked(workbuddyApi.listInferenceAccounts).mockResolvedValue([
      { id: "foreign-row", state: "ready" },
    ]);
    mount();
    expect(
      await screen.findByRole("option", { name: /Synthetic WorkBuddy/ }),
    ).toBeDisabled();
    expect(workbuddyApi.discoverModels).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each(["needsAuthorization", "expired"])(
    "never treats old check-in capabilities as inference authorization (%s)",
    async (state) => {
      workbuddyReady(state);
      mount();
      expect(
        await screen.findByRole("option", { name: /Synthetic WorkBuddy/ }),
      ).toBeDisabled();
      expect(workbuddyApi.discoverModels).not.toHaveBeenCalled();
      expect(invoke).not.toHaveBeenCalled();
    },
  );
  it.each(["claude", "codex"])(
    "explicitly discovers and rechecks the same account before original add(false) for %s",
    async (app) => {
      workbuddyReady("refreshRequired");
      mount();
      await selectWorkbuddy(app);
      expect(workbuddyApi.discoverModels).not.toHaveBeenCalled();
      expect(screen.getByLabelText("官方模型 ID").tagName).toBe("SELECT");
      expect(
        screen.getByRole("button", { name: "加入应用路由链" }),
      ).toBeDisabled();
      await discoverWorkbuddy();
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await screen.findByRole("status");
      expect(workbuddyApi.discoverModels).toHaveBeenNthCalledWith(1, wb.id);
      expect(workbuddyApi.discoverModels).toHaveBeenNthCalledWith(2, wb.id);
      expect(invoke).toHaveBeenCalledWith(
        "add_provider",
        expect.objectContaining({
          app,
          addToLive: false,
          provider: expect.objectContaining({
            meta: {
              authBinding: {
                source: "managed_account",
                authProvider: "workbuddy",
                accountId: wb.id,
              },
              apiFormat: "openai_chat",
              commonConfigEnabled: false,
            },
          }),
        }),
      );
      const saved = Object.values(rows)[0];
      expect(chain).toEqual([saved.id]);
      expect(JSON.stringify(saved)).not.toMatch(
        /canRefresh|canClaim|accessToken|refreshToken|BASE_URL/,
      );
    },
  );
  it("rejects a model removed after discovery and requires a new explicit selection", async () => {
    workbuddyReady();
    mount();
    await selectWorkbuddy();
    await discoverWorkbuddy();
    vi.mocked(workbuddyApi.discoverModels).mockResolvedValue([
      { id: "other", name: "Other" },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("重新获取");
    expect(invoke).not.toHaveBeenCalled();
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
  });
  it("rejects inference retirement despite still-valid check-in flags", async () => {
    workbuddyReady();
    mount();
    await selectWorkbuddy();
    await discoverWorkbuddy();
    vi.mocked(workbuddyApi.listInferenceAccounts).mockResolvedValue([
      { id: wb.id, state: "expired" },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("alert");
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each([false, true])(
    "ignores an old discovery after account ABA, including late failure=%s",
    async (fail) => {
      workbuddyReady();
      let resolve!: (models: typeof wbModels) => void;
      let reject!: (error: Error) => void;
      vi.mocked(workbuddyApi.discoverModels).mockImplementationOnce(
        () =>
          new Promise((ok, no) => {
            resolve = ok;
            reject = no;
          }),
      );
      mount();
      await selectWorkbuddy();
      fireEvent.click(
        screen.getByRole("button", { name: "获取 WorkBuddy 模型" }),
      );
      const miniOption = screen.getByRole("option", {
        name: /Synthetic MiniMax/,
      });
      fireEvent.change(screen.getByLabelText("账号"), {
        target: { value: (miniOption as HTMLOptionElement).value },
      });
      await selectWorkbuddy();
      await discoverWorkbuddy();
      if (fail) reject(new Error("must-not-copy late error"));
      else resolve([{ id: "stale-model", name: "Stale model" }]);
      await waitFor(() =>
        expect(workbuddyApi.discoverModels).toHaveBeenCalledTimes(2),
      );
      await new Promise((done) => setTimeout(done, 0));
      expect(screen.queryByRole("option", { name: "Stale model" })).toBeNull();
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue(
        "synthetic-wb-model",
      );
      expect(screen.queryByRole("alert")).toBeNull();
    },
  );
  it("discards an unmounted pending discovery and never copies it into a reopened section", async () => {
    workbuddyReady();
    let resolve!: (models: typeof wbModels) => void;
    vi.mocked(workbuddyApi.discoverModels).mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const view = mount();
    await selectWorkbuddy();
    fireEvent.click(
      screen.getByRole("button", { name: "获取 WorkBuddy 模型" }),
    );
    view.unmount();
    mount();
    await selectWorkbuddy();
    resolve(wbModels);
    await new Promise((done) => setTimeout(done, 0));
    expect(
      screen.queryByRole("option", { name: "Synthetic WorkBuddy Model" }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("does not create a provider after unmount during live-model revalidation", async () => {
    workbuddyReady();
    const view = mount();
    await selectWorkbuddy();
    await discoverWorkbuddy();
    let resolve!: (models: typeof wbModels) => void;
    vi.mocked(workbuddyApi.discoverModels).mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await waitFor(() =>
      expect(workbuddyApi.discoverModels).toHaveBeenCalledTimes(2),
    );
    view.unmount();
    resolve(wbModels);
    await new Promise((done) => setTimeout(done, 0));
    expect(invoke).not.toHaveBeenCalled();
  });
  it("does not create a provider after unmount during the original provider-store read", async () => {
    workbuddyReady();
    const view = mount();
    await selectWorkbuddy();
    await discoverWorkbuddy();
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_providers")
        return new Promise((done) => {
          resolve = done;
        });
      return originalCommand(command, args as Record<string, unknown>);
    });
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_providers", { app: "claude" }),
    );
    view.unmount();
    resolve({});
    await new Promise((done) => setTimeout(done, 0));
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(0);
  });
  it("prevents repeated discovery and saves while preserving original row reuse", async () => {
    workbuddyReady();
    mount();
    await selectWorkbuddy();
    let resolve!: (models: typeof wbModels) => void;
    vi.mocked(workbuddyApi.discoverModels).mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const discover = screen.getByRole("button", {
      name: "获取 WorkBuddy 模型",
    });
    fireEvent.click(discover);
    fireEvent.click(discover);
    expect(workbuddyApi.discoverModels).toHaveBeenCalledTimes(1);
    resolve(wbModels);
    await screen.findByRole("option", { name: "Synthetic WorkBuddy Model" });
    fireEvent.change(screen.getByLabelText("官方模型 ID"), {
      target: { value: "synthetic-wb-model" },
    });
    const save = screen.getByRole("button", { name: "加入应用路由链" });
    fireEvent.click(save);
    fireEvent.click(save);
    await screen.findByRole("status");
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
    await selectWorkbuddy();
    await discoverWorkbuddy();
    fireEvent.click(save);
    await screen.findByRole("status");
    expect(Object.keys(rows)).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
});

const trae: TraeAccountView = {
  id: "trae-sg-opaque",
  realm: "sg",
  status: "stored",
};
async function selectTrae(
  id = trae.id,
  app = "claude",
  model = "confirmed-global-model",
) {
  const option = await screen.findByRole("option", { name: new RegExp(id) });
  fireEvent.change(screen.getByLabelText("账号"), {
    target: { value: (option as HTMLOptionElement).value },
  });
  fireEvent.change(screen.getByLabelText("目标应用"), {
    target: { value: app },
  });
  if (model) {
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    await screen.findByRole("option", { name: "Discovered TRAE model" });
    fireEvent.change(screen.getByLabelText("官方模型 ID"), {
      target: { value: model },
    });
  }
}
describe("TRAE Global explicit text-only account entry", () => {
  it("shows stored status and capabilities without guessing a model or claiming readiness", async () => {
    vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
    mount();
    await selectTrae(trae.id, "claude", "");
    expect(
      screen.getByRole("option", { name: /trae-sg-opaque/ }),
    ).toHaveTextContent("SG");
    expect(
      screen.getByRole("option", { name: /trae-sg-opaque/ }),
    ).toHaveTextContent("已保存");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expect(screen.getByText(/TRAE Global.*仅纯文本/)).toHaveTextContent(
      "不支持工具调用、图片或附件",
    );
    expect(screen.getByText(/TRAE Global.*仅纯文本/)).toHaveTextContent(
      "未验证模型可用",
    );
    expect(screen.getByText(/TRAE Global.*仅纯文本/)).toHaveTextContent(
      "已发现模型",
    );
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    expect(workbuddyApi.discoverModels).not.toHaveBeenCalled();
    expectNoProviderWrite();
  });
  it.each([
    ["sg", "claude"],
    ["sg", "codex"],
    ["us", "claude"],
    ["us", "codex"],
  ] as const)(
    "saves only the selected %s account/model for %s through original add(false)",
    async (realm, app) => {
      const row = { ...trae, realm, id: `trae-${realm}-opaque` };
      vi.mocked(traeAccountsApi.list).mockResolvedValue([row]);
      mount();
      await selectTrae(row.id, app);
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await screen.findByRole("status");
      expect(traeAccountsApi.list).toHaveBeenCalledTimes(2);
      expect(nativeCalls("begin_trae_model_discovery")).toEqual([
        ["begin_trae_model_discovery", { accountId: row.id }],
        ["begin_trae_model_discovery", { accountId: row.id }],
      ]);
      expect(nativeCalls("list_trae_discovered_models")).toEqual([
        ["list_trae_discovered_models", { requestId: "synthetic-discovery-1" }],
        ["list_trae_discovered_models", { requestId: "synthetic-discovery-2" }],
      ]);
      expect(invoke).toHaveBeenCalledWith(
        "add_provider",
        expect.objectContaining({
          app,
          addToLive: false,
          provider: expect.objectContaining({
            meta: {
              authBinding: {
                source: "managed_account",
                authProvider: "trae",
                accountId: row.id,
              },
              apiFormat: "openai_chat",
              commonConfigEnabled: false,
            },
          }),
        }),
      );
      const saved = Object.values(rows)[0];
      expect(JSON.stringify(saved)).not.toMatch(
        /BASE_URL|base_url|accessToken|refreshToken|authHost|realm|stored|inference_verified|discovered/,
      );
      expect(chain).toEqual([saved.id]);
      expect(screen.getByRole("status")).toHaveTextContent("未切换当前配置");
    },
  );
  it.each([
    { ...trae, realm: "cn" },
    { ...trae, status: "retired" },
    { ...trae, status: "refreshPending" },
    { ...trae, realm: "unknown" },
    { ...trae, status: "ready" },
  ])("does not admit unsupported or uncertain local status %j", async (row) => {
    vi.mocked(traeAccountsApi.list).mockResolvedValue([
      row,
    ] as TraeAccountView[]);
    mount();
    expect(
      await screen.findByRole("option", { name: /trae-sg-opaque/ }),
    ).toBeDisabled();
    expectNoProviderWrite();
  });
  it.each(["retired", "refreshPending"] as const)(
    "rechecks %s before writing a Provider",
    async (status) => {
      vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
      mount();
      await selectTrae();
      vi.mocked(traeAccountsApi.list).mockResolvedValue([{ ...trae, status }]);
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await screen.findByRole("alert");
      expectNoProviderWrite();
    },
  );
  it("keeps other catalogs available when the TRAE owner list fails", async () => {
    vi.mocked(traeAccountsApi.list).mockRejectedValue(
      new Error("must-not-copy native details"),
    );
    mount();
    await screen.findByRole("option", { name: /Synthetic MiniMax/ });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "部分账号库不可用",
    );
    expect(screen.queryByText(/must-not-copy/)).toBeNull();
    await choose();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
  });
  it.each([false, true])(
    "ignores an old owner reread after unmount/reopen, late failure=%s",
    async (fail) => {
      vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
      const view = mount();
      await selectTrae();
      let resolve!: (value: TraeAccountView[]) => void;
      let reject!: (reason: Error) => void;
      vi.mocked(traeAccountsApi.list).mockImplementationOnce(
        () =>
          new Promise((ok, no) => {
            resolve = ok;
            reject = no;
          }),
      );
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await waitFor(() =>
        expect(traeAccountsApi.list).toHaveBeenCalledTimes(2),
      );
      view.unmount();
      mount();
      await selectTrae();
      if (fail) reject(new Error("must-not-copy late failure"));
      else resolve([trae]);
      await new Promise((done) => setTimeout(done, 0));
      expectNoProviderWrite();
      expect(screen.queryByRole("alert")).toBeNull();
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue(
        "confirmed-global-model",
      );
    },
  );
  it("does not reuse a model after account ABA and repeated submission keeps one row", async () => {
    vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
    mount();
    await selectTrae();
    const other = screen.getByRole("option", { name: /Synthetic MiniMax/ });
    fireEvent.change(screen.getByLabelText("账号"), {
      target: { value: (other as HTMLOptionElement).value },
    });
    await selectTrae(trae.id, "claude", "");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    await selectTrae();
    const save = screen.getByRole("button", { name: "加入应用路由链" });
    fireEvent.click(save);
    fireEvent.click(save);
    await screen.findByRole("status");
    await selectTrae();
    fireEvent.click(save);
    await screen.findByRole("status");
    expect(Object.keys(rows)).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
  it("does not announce chain success until original CAS restores a saved TRAE row", async () => {
    vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
    appendOnSave = false;
    chain = ["existing-route"];
    mount();
    await selectTrae();
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "已保存，加入路由待恢复",
    );
    expect(screen.queryByRole("status")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "重试加入路由链" }));
    await screen.findByRole("status");
    expect(chain).toEqual(["existing-route", Object.keys(rows)[0]]);
    expect(invoke).toHaveBeenCalledWith(
      "apply_application_routing",
      expect.objectContaining({
        request: expect.objectContaining({ revision: "synthetic-revision" }),
      }),
    );
    expect(
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "add_provider"),
    ).toHaveLength(1);
  });
});

// These tests do not mock traeModels. The production thin API must reach the
// native invoke mock with the exact opaque account/request contracts.
describe("TRAE cancellable discovered-model entry", () => {
  beforeEach(() => {
    queryClient = createTestQueryClient();
    vi.mocked(traeAccountsApi.list).mockResolvedValue([trae]);
  });
  it("does not discover on mount or account selection, and does not accept a hand-typed/default model", async () => {
    mount();
    await selectTrae(trae.id, "claude", "");
    expect(invoke).not.toHaveBeenCalled();
    expect(screen.getByLabelText("官方模型 ID").tagName).toBe("SELECT");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expect(screen.getByLabelText("官方模型 ID")).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "获取 TRAE 模型" }),
    ).toBeEnabled();
  });
  it("performs one metadata begin then one list only on explicit discovery, suppressing repeated clicks", async () => {
    let resolve!: (value: { requestId: string }) => void;
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "begin_trae_model_discovery"
        ? await new Promise((done) => {
            resolve = done;
          })
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    mount();
    await selectTrae(trae.id, "claude", "");
    const button = screen.getByRole("button", { name: "获取 TRAE 模型" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(nativeCalls("begin_trae_model_discovery")).toEqual([
      ["begin_trae_model_discovery", { accountId: trae.id }],
    ]);
    expect(nativeCalls("list_trae_discovered_models")).toHaveLength(0);
    discoveredRequests.set("metadata-only", trae.id);
    await act(async () => {
      resolve({ requestId: "metadata-only" });
    });
    await screen.findByRole("option", { name: "Discovered TRAE model" });
    expect(nativeCalls("list_trae_discovered_models")).toEqual([
      ["list_trae_discovered_models", { requestId: "metadata-only" }],
    ]);
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expectNoProviderWrite();
  });
  it.each(["begin_trae_model_discovery", "list_trae_discovered_models"])(
    "keeps %s errors safe and requires a new explicit request",
    async (failedCommand) => {
      vi.mocked(invoke).mockImplementation(async (cmd, args) => {
        if (cmd === failedCommand)
          throw new Error("must-not-copy native secret echo");
        return originalCommand(cmd, args as Record<string, unknown>);
      });
      mount();
      await selectTrae(trae.id, "claude", "");
      fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("重新获取");
      expect(screen.queryByText(/must-not-copy/)).toBeNull();
      expect(nativeCalls(failedCommand)).toHaveLength(1);
      expect(
        screen.getByRole("button", { name: "加入应用路由链" }),
      ).toBeDisabled();
      expectNoProviderWrite();
    },
  );
  it("rejects a model removed during the fresh pre-save discovery", async () => {
    mount();
    await selectTrae();
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "list_trae_discovered_models"
        ? [{ id: "other", name: "Other" }]
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("重新获取");
    expect(nativeCalls("begin_trae_model_discovery")).toHaveLength(2);
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expectNoProviderWrite();
  });
  it("never falls back to the old discovered list when the save-time request fails", async () => {
    mount();
    await selectTrae();
    vi.mocked(invoke).mockImplementation(async (cmd, args) => {
      if (cmd === "list_trae_discovered_models") throw new Error("expired");
      return originalCommand(cmd, args as Record<string, unknown>);
    });
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("alert");
    expectNoProviderWrite();
    expect(nativeCalls("get_providers")).toHaveLength(0);
  });
  it.each([false, true])(
    "cancels the exact old request and ignores an account ABA late failure=%s",
    async (fail) => {
      let resolve!: (models: typeof traeModels) => void;
      let reject!: (error: Error) => void;
      vi.mocked(invoke).mockImplementation(async (cmd, args) =>
        cmd === "list_trae_discovered_models" &&
        (args as { requestId: string }).requestId === "synthetic-discovery-1"
          ? await new Promise((ok, no) => {
              resolve = ok;
              reject = no;
            })
          : originalCommand(cmd, args as Record<string, unknown>),
      );
      mount();
      await selectTrae(trae.id, "claude", "");
      fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
      await waitFor(() =>
        expect(nativeCalls("list_trae_discovered_models")).toHaveLength(1),
      );
      const other = screen.getByRole("option", { name: /Synthetic MiniMax/ });
      fireEvent.change(screen.getByLabelText("账号"), {
        target: { value: (other as HTMLOptionElement).value },
      });
      expect(nativeCalls("cancel_trae_model_discovery")).toEqual([
        ["cancel_trae_model_discovery", { requestId: "synthetic-discovery-1" }],
      ]);
      await selectTrae();
      await act(async () => {
        if (fail) reject(new Error("must-not-copy"));
        else resolve([{ id: "stale", name: "Stale TRAE model" }]);
      });
      expect(
        screen.queryByRole("option", { name: "Stale TRAE model" }),
      ).toBeNull();
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue(
        "confirmed-global-model",
      );
      expect(screen.queryByRole("alert")).toBeNull();
      expectNoProviderWrite();
    },
  );
  it("cancels a begin handle arriving after account change without listing it", async () => {
    let resolve!: (value: { requestId: string }) => void;
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "begin_trae_model_discovery"
        ? await new Promise((done) => {
            resolve = done;
          })
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    mount();
    await selectTrae(trae.id, "claude", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    const other = screen.getByRole("option", { name: /Synthetic MiniMax/ });
    fireEvent.change(screen.getByLabelText("账号"), {
      target: { value: (other as HTMLOptionElement).value },
    });
    await act(async () => {
      resolve({ requestId: "late-begin" });
    });
    expect(nativeCalls("cancel_trae_model_discovery")).toEqual([
      ["cancel_trae_model_discovery", { requestId: "late-begin" }],
    ]);
    expect(nativeCalls("list_trae_discovered_models")).toHaveLength(0);
  });
  it.each(["discovery", "save-time"])(
    "unmount cancels %s and prevents a late Provider save",
    async (phase) => {
      const view = mount();
      await selectTrae(
        trae.id,
        "claude",
        phase === "save-time" ? "confirmed-global-model" : "",
      );
      let resolve!: (models: typeof traeModels) => void;
      vi.mocked(invoke).mockImplementation(async (cmd, args) =>
        cmd === "list_trae_discovered_models"
          ? await new Promise((done) => {
              resolve = done;
            })
          : originalCommand(cmd, args as Record<string, unknown>),
      );
      fireEvent.click(
        screen.getByRole("button", {
          name: phase === "save-time" ? "加入应用路由链" : "获取 TRAE 模型",
        }),
      );
      const number = phase === "save-time" ? 2 : 1;
      await waitFor(() =>
        expect(nativeCalls("list_trae_discovered_models")).toHaveLength(number),
      );
      view.unmount();
      expect(nativeCalls("cancel_trae_model_discovery")).toContainEqual([
        "cancel_trae_model_discovery",
        { requestId: `synthetic-discovery-${number}` },
      ]);
      await act(async () => {
        resolve(traeModels);
      });
      expectNoProviderWrite();
      expect(nativeCalls("get_providers")).toHaveLength(0);
    },
  );
  it("unmount while begin is pending cancels the late handle without calling list", async () => {
    let resolve!: (value: { requestId: string }) => void;
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "begin_trae_model_discovery"
        ? await new Promise((done) => {
            resolve = done;
          })
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    const view = mount();
    await selectTrae(trae.id, "claude", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    view.unmount();
    await act(async () => {
      resolve({ requestId: "late-unmounted" });
    });
    expect(nativeCalls("cancel_trae_model_discovery")).toEqual([
      ["cancel_trae_model_discovery", { requestId: "late-unmounted" }],
    ]);
    expect(nativeCalls("list_trae_discovered_models")).toHaveLength(0);
  });
  it("refresh cancels the active request and discards the model selection", async () => {
    let resolve!: (models: typeof traeModels) => void;
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "list_trae_discovered_models"
        ? await new Promise((done) => {
            resolve = done;
          })
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    mount();
    await selectTrae(trae.id, "claude", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    await waitFor(() =>
      expect(nativeCalls("list_trae_discovered_models")).toHaveLength(1),
    );
    fireEvent.click(screen.getByRole("button", { name: "刷新账号列表" }));
    await act(async () => {
      resolve(traeModels);
    });
    expect(nativeCalls("cancel_trae_model_discovery")).toEqual([
      ["cancel_trae_model_discovery", { requestId: "synthetic-discovery-1" }],
    ]);
    expect(screen.getByLabelText("账号")).toHaveValue("");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expectNoProviderWrite();
  });
  it("an empty discovered catalog has no hand-entry fallback or implicit model", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "list_trae_discovered_models"
        ? []
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    mount();
    await selectTrae(trae.id, "claude", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "未发现可选模型",
    );
    expect(screen.getByLabelText("官方模型 ID").tagName).toBe("SELECT");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expect(
      screen.getByRole("button", { name: "加入应用路由链" }),
    ).toBeDisabled();
    expect(nativeCalls("list_trae_discovered_models")).toHaveLength(1);
    expectNoProviderWrite();
  });
  it("a cancel failure and old result never replace a newer account's discovery", async () => {
    let resolve!: (models: typeof traeModels) => void;
    vi.mocked(invoke).mockImplementation(async (cmd, args) => {
      if (cmd === "cancel_trae_model_discovery")
        throw new Error("must-not-copy cancellation details");
      if (
        cmd === "list_trae_discovered_models" &&
        (args as { requestId: string }).requestId === "synthetic-discovery-1"
      )
        return await new Promise((done) => {
          resolve = done;
        });
      return originalCommand(cmd, args as Record<string, unknown>);
    });
    mount();
    await selectTrae(trae.id, "claude", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    await waitFor(() =>
      expect(nativeCalls("list_trae_discovered_models")).toHaveLength(1),
    );
    const other = screen.getByRole("option", { name: /Synthetic MiniMax/ });
    fireEvent.change(screen.getByLabelText("账号"), {
      target: { value: (other as HTMLOptionElement).value },
    });
    await selectTrae();
    await act(async () => {
      resolve([{ id: "stale", name: "Stale TRAE model" }]);
    });
    expect(
      screen.queryByRole("option", { name: "Stale TRAE model" }),
    ).toBeNull();
    expect(screen.queryByText(/must-not-copy/)).toBeNull();
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue(
      "confirmed-global-model",
    );
    expectNoProviderWrite();
  });
  it("preserves an exact valid non-ASCII discovered ID in selection and save without inferring verification", async () => {
    const id = ' 模型 "Global" / v1 ';
    vi.mocked(invoke).mockImplementation(async (cmd, args) =>
      cmd === "list_trae_discovered_models"
        ? [{ id, name: "Exact original ID" }]
        : originalCommand(cmd, args as Record<string, unknown>),
    );
    mount();
    await selectTrae(trae.id, "codex", "");
    fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
    await screen.findByRole("option", { name: "Exact original ID" });
    fireEvent.change(screen.getByLabelText("官方模型 ID"), {
      target: { value: id },
    });
    fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
    await screen.findByRole("status");
    const saved = Object.values(rows)[0];
    expect(saved.settingsConfig.config).toBe(`model = ${JSON.stringify(id)}\n`);
    expect(JSON.stringify(saved.meta)).not.toMatch(
      /verified|discovered|requestId/,
    );
  });
});

afterEach(() => {
  cleanup();
  queryClient.clear();
});
