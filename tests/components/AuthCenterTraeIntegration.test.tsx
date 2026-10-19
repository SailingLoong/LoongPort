import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import {
  TRAE_AUTH_HOST,
  type TraeAccountView,
  type TraeRealm,
} from "@/lib/api/traeAccounts";
import type { Provider } from "@/types";
import { buildManagedAccountProvider } from "@/lib/managedAccountProvider";

// The real mounted TRAE component, dialogs, account selector, bootstrap widget,
// and native API wrappers are consumed. Only native effects and unrelated OAuth
// widgets are replaced. All paths, accounts, and command results are synthetic.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? _key,
  }),
}));
vi.mock("@/lib/api/minimaxCode", () => ({
  minimaxCodeApi: { list: vi.fn(async () => []) },
}));
vi.mock("@/lib/api/zcodeAccounts", () => ({
  zcodeAccountsApi: {
    library: vi.fn(async () => ({ pending: false, profiles: [] })),
  },
}));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: {
    list: vi.fn(async () => []),
    listInferenceAccounts: vi.fn(async () => []),
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

const paths = {
  storagePath: "/synthetic/official-trae/storage.json",
  productPath: "/synthetic/official-trae/product.json",
};
const flowId = "synthetic-authcenter-flow";
let realm: TraeRealm;
let accounts: TraeAccountView[];
let providers: Record<string, Provider>;
let chain: string[];
let modelRequests: Map<string, { accountId: string; consumed: boolean }>;
let activeProvider: string | null;
const takeoverRequest = {
  id: "synthetic-request",
  scope: "proxyTakeover",
  draftDigest: "synthetic-digest",
  revision: "synthetic-revision",
};
function account(): TraeAccountView {
  return { id: "synthetic-original-owner-id", realm, status: "stored" };
}
function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}
async function command(
  name: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  if (name === "list_trae_accounts") return accounts.map((row) => ({ ...row }));
  if (name === "begin_trae_authorization")
    return { flowId, realm, authHost: TRAE_AUTH_HOST };
  if (name === "confirm_trae_authorization") {
    accounts = [account()];
    return account();
  }
  if (name === "cancel_trae_authorization") return;
  if (name === "begin_trae_model_discovery") {
    const requestId = `synthetic-model-discovery-${modelRequests.size + 1}`;
    modelRequests.set(requestId, {
      accountId: args!.accountId as string,
      consumed: false,
    });
    return { requestId };
  }
  if (name === "list_trae_discovered_models") {
    const request = modelRequests.get(args!.requestId as string);
    if (!request || request.consumed)
      throw new Error("unknown or consumed synthetic model request");
    request.consumed = true;
    return [
      { id: "synthetic-explicit-model", name: "Synthetic discovered model" },
    ];
  }
  if (name === "cancel_trae_model_discovery") return;
  if (name === "get_providers") return { ...providers };
  if (name === "add_provider") {
    const provider = args!.provider as Provider;
    if (providers[provider.id]) throw new Error("duplicate synthetic provider");
    providers[provider.id] = provider;
    chain.push(provider.id);
    return true;
  }
  if (name === "get_application_routing")
    return {
      chainIds: [...chain],
      routingActive: activeProvider !== null,
      modeState: {
        status: "ready",
        mode: activeProvider ? "proxy" : "direct",
        attached: activeProvider !== null,
        currentProviderId: activeProvider,
        directProviderId: null,
        canWrite: true,
      },
    };
  if (name === "prepare_proxy_takeover_request") return takeoverRequest;
  if (name === "set_proxy_takeover_for_app") {
    activeProvider = (args!.initialRoute as { providerId: string }).providerId;
    return;
  }
  if (name === "get_application_routing_request_result")
    return { status: "completed" };
  throw new Error(`Unexpected native command: ${name}`);
}
function mount() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <AuthCenterPanel />
    </QueryClientProvider>,
  );
}
async function selectFiles() {
  await screen.findByText("尚无 TRAE 本地账号。");
  fireEvent.click(screen.getByRole("button", { name: "添加 TRAE 账号" }));
  const dialog = await screen.findByRole("dialog");
  expect(
    within(dialog).getByText(new RegExp(TRAE_AUTH_HOST.replaceAll(".", "\\."))),
  ).toBeInTheDocument();
  expect(calls("begin_trae_authorization")).toHaveLength(0);
  fireEvent.click(
    within(dialog).getByRole("radio", { name: realm.toUpperCase() }),
  );
  fireEvent.click(
    within(dialog).getByRole("button", {
      name: "选择 storage.json 和 product.json",
    }),
  );
  return dialog;
}
async function selectedFlow() {
  const dialog = await selectFiles();
  await within(dialog).findByRole("checkbox");
  expect(invoke).toHaveBeenCalledWith("begin_trae_authorization", {
    ...paths,
    realm,
  });
  expect(open).toHaveBeenNthCalledWith(1, {
    title: "选择 storage.json",
    directory: false,
    multiple: false,
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  expect(open).toHaveBeenNthCalledWith(2, {
    title: "选择同一安装的 product.json",
    directory: false,
    multiple: false,
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  return dialog;
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(open).mockReset();
  realm = "sg";
  accounts = [];
  providers = {};
  chain = [];
  activeProvider = null;
  modelRequests = new Map();
  vi.mocked(open)
    .mockResolvedValueOnce(paths.storagePath)
    .mockResolvedValueOnce(paths.productPath);
  vi.mocked(invoke).mockImplementation(command as never);
});
describe("actual AuthCenter TRAE mount and original native command boundary", () => {
  it("mounts TRAE, account entry and the existing bootstrap without implicit authorization or configuration", async () => {
    mount();
    await screen.findByText("尚无 TRAE 本地账号。");
    expect(
      screen.getByRole("region", { name: "TRAE 账号" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "将账号加入应用路由链" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "首次启用账号路由" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("首次启用的应用")).toHaveValue("");
    expect(screen.getByLabelText("账号")).toHaveValue("");
    expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
    expect(vi.mocked(invoke).mock.calls.length).toBeGreaterThan(0);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.every(([name]) => name === "list_trae_accounts"),
    ).toBe(true);
    expect(open).not.toHaveBeenCalled();
  });

  it.each(["sg", "us"] as const)(
    "requires explicit %s consent and confirmation before the same original owner can be explicitly added to the pool",
    async (selectedRealm) => {
      realm = selectedRealm;
      const parent = mount();
      const dialog = await selectedFlow();
      const confirm = within(dialog).getByRole("button", {
        name: "确认读取并授权保存",
      });
      expect(confirm).toBeDisabled();
      fireEvent.click(confirm);
      expect(calls("confirm_trae_authorization")).toHaveLength(0);
      fireEvent.click(within(dialog).getByRole("checkbox"));
      expect(confirm).toBeEnabled();
      expect(calls("confirm_trae_authorization")).toHaveLength(0);
      fireEvent.click(confirm);
      await screen.findByText("账号已加密保存在本地。");
      expect(calls("confirm_trae_authorization")).toEqual([
        ["confirm_trae_authorization", { flowId }],
      ]);
      expect(calls("add_provider")).toHaveLength(0);
      expect(screen.getByLabelText("账号")).toHaveValue("");
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");

      fireEvent.click(screen.getByRole("button", { name: "刷新账号列表" }));
      const option = await screen.findByRole("option", {
        name: /TRAE Global.*synthetic-original-owner-id/,
      });
      fireEvent.change(screen.getByLabelText("账号"), {
        target: { value: (option as HTMLOptionElement).value },
      });
      expect(screen.getByText(/发现列表尚未验证模型可用/)).toHaveTextContent(
        "不支持工具调用、图片或附件",
      );
      fireEvent.change(screen.getByLabelText("目标应用"), {
        target: { value: "codex" },
      });
      expect(screen.getByLabelText("官方模型 ID").tagName).toBe("SELECT");
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
      expect(calls("begin_trae_model_discovery")).toHaveLength(0);
      fireEvent.click(screen.getByRole("button", { name: "获取 TRAE 模型" }));
      await screen.findByRole("option", { name: "Synthetic discovered model" });
      expect(calls("begin_trae_model_discovery")).toEqual([
        ["begin_trae_model_discovery", { accountId: account().id }],
      ]);
      expect(calls("list_trae_discovered_models")).toEqual([
        [
          "list_trae_discovered_models",
          { requestId: "synthetic-model-discovery-1" },
        ],
      ]);
      expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
      expect(calls("add_provider")).toHaveLength(0);
      fireEvent.change(screen.getByLabelText("官方模型 ID"), {
        target: { value: "synthetic-explicit-model" },
      });
      const listBeforeSave = calls("list_trae_accounts").length;
      fireEvent.click(screen.getByRole("button", { name: "加入应用路由链" }));
      await screen.findByText(/已加入应用路由链，未切换当前配置/);
      expect(calls("list_trae_accounts").length).toBeGreaterThan(
        listBeforeSave,
      );
      expect(calls("begin_trae_model_discovery")).toEqual([
        ["begin_trae_model_discovery", { accountId: account().id }],
        ["begin_trae_model_discovery", { accountId: account().id }],
      ]);
      expect(calls("list_trae_discovered_models")).toEqual([
        [
          "list_trae_discovered_models",
          { requestId: "synthetic-model-discovery-1" },
        ],
        [
          "list_trae_discovered_models",
          { requestId: "synthetic-model-discovery-2" },
        ],
      ]);
      expect([...modelRequests.values()]).toEqual([
        { accountId: account().id, consumed: true },
        { accountId: account().id, consumed: true },
      ]);
      expect(calls("add_provider")).toHaveLength(1);
      expect(invoke).toHaveBeenCalledWith(
        "add_provider",
        expect.objectContaining({
          app: "codex",
          addToLive: false,
          provider: expect.objectContaining({
            meta: expect.objectContaining({
              apiFormat: "openai_chat",
              authBinding: {
                source: "managed_account",
                authProvider: "trae",
                accountId: account().id,
              },
            }),
          }),
        }),
      );
      const saved = Object.values(providers)[0];
      expect(saved.settingsConfig.config).toBe(
        'model = "synthetic-explicit-model"\n',
      );
      expect(
        vi
          .mocked(invoke)
          .mock.calls.map(([name]) => name)
          .filter((name) =>
            [
              "begin_trae_model_discovery",
              "list_trae_discovered_models",
              "add_provider",
            ].includes(name),
          ),
      ).toEqual([
        "begin_trae_model_discovery",
        "list_trae_discovered_models",
        "begin_trae_model_discovery",
        "list_trae_discovered_models",
        "add_provider",
      ]);
      expect(chain).toEqual([saved.id]);
      expect(JSON.stringify(saved)).not.toMatch(
        /storagePath|productPath|accessToken|refreshToken/,
      );
      expect(calls("get_application_routing")).toHaveLength(1);
      expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
      parent.unmount();
      expect(calls("cancel_trae_authorization")).toHaveLength(0);
    },
  );

  it("the mounted bootstrap exposes a saved TRAE route but only explicitly enables it with its original prepared receipt", async () => {
    const saved = buildManagedAccountProvider(
      {
        authProvider: "trae",
        id: account().id,
        label: "Synthetic saved TRAE",
        available: true,
      },
      "codex",
      "synthetic-explicit-model",
      "saved-trae-route",
    );
    providers[saved.id] = saved;
    chain = [saved.id];
    mount();
    await screen.findByText("尚无 TRAE 本地账号。");
    const bootstrap = screen.getByRole("region", { name: "首次启用账号路由" });
    const enable = within(bootstrap).getByRole("button", {
      name: "启用本地路由并使用此账号",
    });
    expect(enable).toBeDisabled();
    expect(calls("get_providers")).toHaveLength(0);
    fireEvent.change(within(bootstrap).getByLabelText("首次启用的应用"), {
      target: { value: "codex" },
    });
    await within(bootstrap).findByRole("option", { name: saved.name });
    expect(calls("prepare_proxy_takeover_request")).toHaveLength(0);
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
    fireEvent.change(within(bootstrap).getByLabelText("已保存的账号档位"), {
      target: { value: saved.id },
    });
    expect(enable).toBeEnabled();
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
    fireEvent.click(enable);
    await within(bootstrap).findByText(`codex: ${saved.name} · 已启用本地路由`);
    expect(calls("prepare_proxy_takeover_request")).toEqual([
      [
        "prepare_proxy_takeover_request",
        {
          appType: "codex",
          providerId: saved.id,
          requestId: expect.any(String),
        },
      ],
    ]);
    expect(calls("get_providers")).toHaveLength(2); // initial choices, then original pre-apply identity recheck
    expect(calls("set_proxy_takeover_for_app")).toEqual([
      [
        "set_proxy_takeover_for_app",
        {
          appType: "codex",
          enabled: true,
          initialRoute: { providerId: saved.id, request: takeoverRequest },
        },
      ],
    ]);
    expect(calls("get_application_routing_request_result")).toEqual([
      [
        "get_application_routing_request_result",
        {
          appType: "codex",
          request: takeoverRequest,
        },
      ],
    ]);
    expect(calls("confirm_trae_authorization")).toHaveLength(0);
    expect(calls("add_provider")).toHaveLength(0);
    expect(open).not.toHaveBeenCalled();
  });

  it("closing the mounted dialog cancels its exact selected flow without confirming", async () => {
    mount();
    const dialog = await selectedFlow();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "关闭 TRAE 授权" }),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(calls("cancel_trae_authorization")).toEqual([
      ["cancel_trae_authorization", { flowId }],
    ]);
    expect(calls("confirm_trae_authorization")).toHaveLength(0);
    expect(calls("add_provider")).toHaveLength(0);
  });

  it("leaving the parent AuthCenter cancels a selected flow", async () => {
    const parent = mount();
    await selectedFlow();
    parent.unmount();
    expect(calls("cancel_trae_authorization")).toEqual([
      ["cancel_trae_authorization", { flowId }],
    ]);
    expect(calls("confirm_trae_authorization")).toHaveLength(0);
    expect(calls("add_provider")).toHaveLength(0);
  });

  it("leaving the parent while confirmation is pending cancels and ignores its late result", async () => {
    let resolve!: (value: TraeAccountView) => void;
    vi.mocked(invoke).mockImplementation(async (name, args) =>
      name === "confirm_trae_authorization"
        ? await new Promise<TraeAccountView>((done) => {
            resolve = done;
          })
        : command(name, args as Record<string, unknown>),
    );
    const parent = mount();
    const dialog = await selectedFlow();
    fireEvent.click(within(dialog).getByRole("checkbox"));
    fireEvent.click(
      within(dialog).getByRole("button", { name: "确认读取并授权保存" }),
    );
    await waitFor(() =>
      expect(calls("confirm_trae_authorization")).toHaveLength(1),
    );
    parent.unmount();
    expect(calls("cancel_trae_authorization")).toEqual([
      ["cancel_trae_authorization", { flowId }],
    ]);
    await act(async () => {
      resolve(account());
    });
    mount();
    await screen.findByText("尚无 TRAE 本地账号。");
    expect(
      screen.queryByText("账号已加密保存在本地。"),
    ).not.toBeInTheDocument();
    expect(calls("confirm_trae_authorization")).toHaveLength(1);
    expect(calls("add_provider")).toHaveLength(0);
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
  });

  it("a begin result arriving after parent unmount is canceled rather than retained by a new AuthCenter", async () => {
    let resolve!: (value: {
      flowId: string;
      realm: TraeRealm;
      authHost: typeof TRAE_AUTH_HOST;
    }) => void;
    vi.mocked(invoke).mockImplementation(async (name, args) =>
      name === "begin_trae_authorization"
        ? await new Promise((done) => {
            resolve = done;
          })
        : command(name, args as Record<string, unknown>),
    );
    const parent = mount();
    await selectFiles();
    await waitFor(() =>
      expect(calls("begin_trae_authorization")).toHaveLength(1),
    );
    parent.unmount();
    expect(calls("cancel_trae_authorization")).toHaveLength(0);
    await act(async () => {
      resolve({ flowId, realm, authHost: TRAE_AUTH_HOST });
    });
    expect(calls("cancel_trae_authorization")).toEqual([
      ["cancel_trae_authorization", { flowId }],
    ]);
    mount();
    await screen.findByText("尚无 TRAE 本地账号。");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(calls("confirm_trae_authorization")).toHaveLength(0);
    expect(calls("add_provider")).toHaveLength(0);
  });
});
