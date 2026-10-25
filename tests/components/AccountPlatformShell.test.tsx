import { act, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { providersApi } from "@/lib/api/providers";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
import { saveZCodeSourcePreference } from "@/lib/zcodeSourcePreference";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import {
  accountPlatformPresenceKey,
  readAccountPlatformPresence,
} from "@/components/relay/accounts/useAccountPlatformPresence";
import type { ServiceAccount } from "@/components/relay/accounts/useServiceAccounts";
import { ServicesPage } from "@/components/relay/accounts/ServicesPage";
const savedServices = vi.hoisted(() => [] as ServiceAccount[]);
const serviceRead = vi.hoisted(() => ({ error: null as Error | null }));
const native = vi.hoisted(() =>
  vi.fn(async (command: string): Promise<unknown> => {
    if (command === "get_service_sources") return [];
    if (command === "list_workbuddy_inference_accounts") return [];
    if (command === "list_workbuddy_accounts")
      return [{ id: "wb", label: "Saved account" }];
    if (command === "auth_get_status") return { accounts: [] };
    if (command === "get_providers") return {};
    if (command === "get_zcode_account_library")
      return { profiles: [], actions: {} };
    if (
      command === "list_trae_accounts" ||
      command === "list_minimax_code_accounts"
    )
      return [];
    throw new Error(`Unexpected command: ${command}`);
  }),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: native }));
vi.mock("@/components/settings/AuthCenterPanel", () => ({
  AuthCenterPanel: ({ platform }: { platform: string }) => {
    const [step, setStep] = useState(0);
    return (
      <div>
        <p>Native account: {platform}</p>
        <button onClick={() => setStep(step + 1)}>
          Native session {platform}: {step}
        </button>
      </div>
    );
  },
}));
vi.mock("@/components/relay/RelaySection", () => ({
  RelaySection: ({
    renderAccounts,
    accountFilter,
  }: {
    accountFilter?: { kind: string; id: number };
    renderAccounts?: (
      actions: () => null,
      remove: () => null,
    ) => React.ReactNode;
  }) =>
    accountFilter ? (
      <p>
        Native detail {accountFilter.kind}:{accountFilter.id}
      </p>
    ) : (
      renderAccounts?.(
        () => null,
        () => null,
      )
    ),
}));
vi.mock("@/components/relay/accounts/useServiceAccounts", () => ({
  useServiceAccounts: () => ({
    accounts: savedServices,
    isPending: false,
    error: serviceRead.error,
    reload: vi.fn(),
  }),
  configuredApps: () => [],
}));
const defaultRead = native.getMockImplementation()!;
beforeEach(() => {
  savedServices.length = 0;
  serviceRead.error = null;
  localStorage.clear();
  native.mockReset().mockImplementation(defaultRead);
});

it.each([
  [["qoder_cn_device"], "CN"],
  [["qoder_sdk"], "INTL"],
  [["qoder_cn_device", "qoder_sdk"], null],
] as const)(
  "shows a Qoder edition only when the native inventory has one region: %s",
  async (types, expected) => {
    native.mockImplementation(async (command) =>
      command === "get_providers"
        ? Object.fromEntries(
            types.map((type, index) => [
              String(index),
              {
                id: String(index),
                name: `Qoder ${index}`,
                settingsConfig: {},
                meta: { providerType: type },
              },
            ]),
          )
        : defaultRead(command),
    );
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <ServicesPage
          appId="codex"
          onOpenApp={vi.fn()}
          onOpenAddHub={vi.fn()}
        />
      </QueryClientProvider>,
    );
    const tab = await screen.findByRole("tab", { name: "Qoder" });
    if (expected) expect(within(tab).getByText(expected)).toBeVisible();
    else expect(within(tab).queryByText(/CN|INTL/)).not.toBeInTheDocument();
  },
);

it("keeps a newer owner read when an older refresh completes after cancellation", async () => {
  let reads = 0;
  let finishOlder!: (rows: unknown[]) => void;
  native.mockImplementation(async (command) => {
    if (command !== "list_workbuddy_accounts") return defaultRead(command);
    reads += 1;
    if (reads === 1) return [{ id: "wb", label: "Initial account" }];
    if (reads === 2)
      return new Promise<unknown[]>((resolve) => {
        finishOlder = resolve;
      });
    return [
      { id: "wb", label: "Newest account", credits: { totalRemaining: 22 } },
    ];
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={client}>
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  await screen.findByRole("button", { name: "查看 Initial account" });
  const refresh = screen.getByRole("button", { name: "刷新状态" });
  await user.click(refresh);
  expect(refresh).toBeDisabled();
  await user.click(refresh);
  expect(reads).toBe(2);
  await act(async () => {
    await client.refetchQueries({ queryKey: accountPlatformPresenceKey });
  });
  expect(
    screen.getByRole("button", { name: "查看 Newest account" }),
  ).toBeVisible();
  await act(async () => {
    finishOlder([
      { id: "wb", label: "Older account", credits: { totalRemaining: 11 } },
    ]);
  });
  expect(
    screen.queryByRole("button", { name: "查看 Older account" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "查看 Newest account" }),
  ).toBeVisible();
  expect(screen.getByText("22")).toBeVisible();
});
it("shows all imported platform accounts by default, filters without hiding, and keeps relay accounts separate", async () => {
  const user = userEvent.setup();
  const onOpenAddHub = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServicesPage
        appId="codex"
        onOpenAddHub={onOpenAddHub}
        onOpenApp={vi.fn()}
      />
    </QueryClientProvider>,
  );
  expect(
    await screen.findByRole("button", { name: "查看 Saved account" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByText("Native account: workbuddy"),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("tabpanel", { name: "平台账号" })).toBeVisible();
  expect(screen.getByRole("tab", { name: "中转站" })).toHaveAttribute(
    "aria-selected",
    "false",
  );
  expect(screen.getByRole("tab", { name: "全部" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.queryByRole("tab", { name: "Qoder" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  expect(
    screen.queryByText("Native account: workbuddy"),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "查看 Saved account" }));
  expect(screen.getByText("Native account: workbuddy")).toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(screen.getByText("Native account: workbuddy")).not.toBeVisible();
  expect(
    screen.getByRole("button", { name: "查看 Saved account" }),
  ).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "添加账号" }));
  expect(onOpenAddHub).toHaveBeenCalledWith("accounts");
  expect(localStorage.getItem("loongport.account-platform-tabs.v1")).toBeNull();
  expect(
    native.mock.calls.every(([command]) =>
      [
        "list_workbuddy_accounts",
        "list_workbuddy_inference_accounts",
        "list_trae_accounts",
        "list_minimax_code_accounts",
        "auth_get_status",
        "get_providers",
        "get_zcode_account_library",
      ].includes(command),
    ),
  ).toBe(true);
});

it("projects CN and international inventories separately and preserves successful platforms on partial read failure", async () => {
  native.mockImplementation(async (command: string): Promise<unknown> => {
    if (command === "get_service_sources") return [];
    if (command === "list_workbuddy_inference_accounts") return [];
    if (command === "list_workbuddy_accounts")
      throw new Error("private upstream details");
    if (command === "auth_get_status") return { accounts: [] };
    if (command === "get_providers")
      return {
        openrouter: {
          id: "or",
          name: "Saved connection",
          icon: "openrouter",
          settingsConfig: { secret: "never-show" },
        },
        renamed: { id: "custom", name: "OpenRouter", settingsConfig: {} },
      };
    if (command === "get_zcode_account_library")
      return { profiles: [], actions: {} };
    if (command === "list_trae_accounts")
      return [
        { id: "cn", realm: "cn", status: "stored" },
        { id: "us", realm: "us", status: "stored" },
        { id: "old", realm: "cn", status: "retired" },
      ];
    if (command === "list_minimax_code_accounts")
      return [
        { id: "cn", region: "cn" },
        { id: "global", region: "global" },
      ];
    throw new Error(`Unexpected command: ${command}`);
  });
  const result = await readAccountPlatformPresence();
  expect(result.platforms).toEqual(
    expect.arrayContaining([
      "trae_cn",
      "trae",
      "minimax_code",
      "minimax_code_global",
      "openrouter",
    ]),
  );
  expect(new Set(result.platforms).size).toBe(result.platforms.length);
  expect(result.platforms).not.toContain("workbuddy");
  expect(result.unavailable).toContain("workbuddy");
  expect(result.connections.every((connection) => connection.id === "or")).toBe(
    true,
  );
  expect(JSON.stringify(result)).not.toMatch(/never-show|private upstream/);
});

it("renders an OpenCode vendor detail only in its platform section", async () => {
  savedServices.push({
    kind: "vendor",
    id: 42,
    row: {
      id: 42,
      vendorId: "opencode",
      vendorName: "OpenCode",
      accountLabel: "Saved OpenCode",
      status: "ready",
      plans: [],
    },
    apps: new Map(),
  } as unknown as ServiceAccount);
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServicesPage
        appId="codex"
        account={{ kind: "vendor", id: 42 }}
        onSelectAccount={vi.fn()}
        onOpenAddHub={vi.fn()}
        onOpenApp={vi.fn()}
      />
    </QueryClientProvider>,
  );
  expect(await screen.findAllByText("Native detail vendor:42")).toHaveLength(1);
});

it("preserves an entered native manager through All, failed reads and confirmed empty without cancelling its session", async () => {
  const user = userEvent.setup();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  await user.click(await screen.findByRole("tab", { name: "WorkBuddy" }));
  await user.click(screen.getByRole("button", { name: "查看 Saved account" }));
  const session = await screen.findByRole("button", {
    name: "Native session workbuddy: 0",
  });
  await user.click(session);
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(session).not.toBeVisible();
  native.mockImplementation(async (command: string): Promise<unknown> => {
    if (command === "list_workbuddy_accounts")
      throw new Error("temporary local read failure");
    return defaultRead(command);
  });
  await act(async () => {
    await client.invalidateQueries({ queryKey: accountPlatformPresenceKey });
  });
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "部分平台账号读取失败",
  );
  expect(
    screen.getByRole("button", { name: "查看 Saved account" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("读取失败");
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  await user.click(screen.getByRole("button", { name: "查看 Saved account" }));
  expect(
    screen.getByRole("button", { name: "Native session workbuddy: 1" }),
  ).toBe(session);
  native.mockImplementation(defaultRead);
  await act(async () => {
    await client.invalidateQueries({ queryKey: accountPlatformPresenceKey });
  });
  expect(
    screen.getByRole("button", { name: "Native session workbuddy: 1" }),
  ).toBe(session);
  native.mockImplementation(async (command: string): Promise<unknown> =>
    command === "list_workbuddy_accounts" ? [] : defaultRead(command),
  );
  await act(async () => {
    await client.invalidateQueries({ queryKey: accountPlatformPresenceKey });
  });
  expect(
    screen.getByRole("button", { name: "Native session workbuddy: 1" }),
  ).toBe(session);
});

it("uses the existing selected ZCode library root rather than an empty default library", async () => {
  saveZCodeSourcePreference({
    installPath: "/opt/synthetic-zcode",
    dataRoot: "/tmp/synthetic-zcode-library",
  });
  const library = vi.spyOn(zcodeAccountsApi, "library").mockImplementation(
    async (root) =>
      ({
        revision: "test",
        profiles:
          root === "/tmp/synthetic-zcode-library"
            ? [{ id: "saved", label: "Saved", family: "zai" }]
            : [],
        current: null,
        pending: false,
        nativeUnconfirmed: false,
        actions: {
          canAdd: true,
          canImport: true,
          canBackup: true,
          canEditLabels: true,
          blockedReason: null,
        },
      }) as never,
  );
  try {
    expect((await readAccountPlatformPresence()).platforms).toContain("zcode");
    expect(library).toHaveBeenCalledWith("/tmp/synthetic-zcode-library");
  } finally {
    library.mockRestore();
  }
});

it("drops confirmed-empty Qoder and OpenRouter sources despite an unrelated app read failing", async () => {
  const providers = vi
    .spyOn(providersApi, "getAll")
    .mockImplementation(async (app) => {
      if (app === "claude") throw new Error("unrelated local app read failed");
      return {};
    });
  try {
    const result = await readAccountPlatformPresence({
      platforms: ["qoder", "openrouter"],
      unavailable: [],
      connections: [
        {
          platform: "openrouter",
          app: "codex",
          id: "old",
          name: "Old connection",
        },
      ],
    });
    expect(result.platforms).not.toContain("qoder");
    expect(result.platforms).not.toContain("openrouter");
    expect(result.connections).toEqual([]);
    expect(result.unavailable).not.toContain("qoder");
  } finally {
    providers.mockRestore();
  }
});

it("retains OpenRouter connections only for the app whose local read failed", async () => {
  const providers = vi
    .spyOn(providersApi, "getAll")
    .mockImplementation(async (app) => {
      if (app === "claude") throw new Error("local read failed");
      return {};
    });
  const unavailableConnection = {
    platform: "openrouter" as const,
    app: "claude" as const,
    id: "retained",
    name: "Retained connection",
  };
  try {
    const result = await readAccountPlatformPresence({
      platforms: ["openrouter"],
      unavailable: [],
      connections: [
        unavailableConnection,
        {
          platform: "openrouter",
          app: "codex",
          id: "deleted",
          name: "Deleted connection",
        },
      ],
    });
    expect(result.connections).toEqual([unavailableConnection]);
    expect(result.platforms).toContain("openrouter");
  } finally {
    providers.mockRestore();
  }
});

it("opens the exact OpenCode account across real mutually exclusive route callbacks", async () => {
  savedServices.push(
    ...[41, 42].map(
      (id) =>
        ({
          kind: "vendor",
          id,
          row: {
            id,
            vendorId: "opencode",
            vendorName: "OpenCode",
            accountLabel: `OpenCode ${id}`,
            status: "ready",
            plans: [],
          },
          apps: new Map([["codex", { id, status: "ready", plans: [] }]]),
        }) as unknown as ServiceAccount,
    ),
  );
  function Routed() {
    const [route, setRoute] = useState<{
      platform?: "opencode";
      account?: { kind: "vendor"; id: number };
    }>({});
    return (
      <ServicesPage
        appId="codex"
        platform={route.platform}
        account={route.account}
        onOpenPlatform={(platform) =>
          setRoute({ platform: platform as "opencode" })
        }
        onSelectAccount={(account) =>
          setRoute({ account: account as { kind: "vendor"; id: number } })
        }
        onOpenAddHub={vi.fn()}
        onOpenApp={vi.fn()}
      />
    );
  }
  const user = userEvent.setup();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <Routed />
    </QueryClientProvider>,
  );
  await user.click(
    await screen.findByRole("button", { name: "查看 OpenCode 42" }),
  );
  expect(await screen.findByText("Native detail vendor:42")).toBeVisible();
  expect(screen.queryByText("Native detail vendor:41")).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "全部" }));
  await user.click(screen.getByRole("button", { name: "查看 OpenCode 41" }));
  expect(await screen.findByText("Native detail vendor:41")).toBeVisible();
  expect(screen.queryByText("Native detail vendor:42")).not.toBeInTheDocument();
});

it("shows one WorkBuddy identity when its native and inference snapshots share the same ID", async () => {
  native.mockImplementation(async (command) =>
    command === "list_workbuddy_inference_accounts"
      ? [{ id: "wb", state: "ready" }]
      : defaultRead(command),
  );
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  expect(
    await screen.findByRole("button", { name: "查看 Saved account" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "查看 wb" }),
  ).not.toBeInTheDocument();
});

it("keeps CN and Global account summaries with the same native ID in their correct groups", async () => {
  native.mockImplementation(async (command) =>
    command === "list_minimax_code_accounts"
      ? [
          { id: "same-id", region: "cn", label: "Domestic identity" },
          { id: "same-id", region: "global", label: "Global identity" },
        ]
      : defaultRead(command),
  );
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  const cn = await screen.findByRole("region", {
    name: "MiniMax Code 国内版 · 账号总览",
  });
  const global = screen.getByRole("region", {
    name: "MiniMax Code 国际版 · 账号总览",
  });
  expect(
    within(cn).getByRole("button", { name: "查看 Domestic identity" }),
  ).toBeInTheDocument();
  expect(within(cn).queryByText("Global identity")).not.toBeInTheDocument();
  expect(
    within(global).getByRole("button", { name: "查看 Global identity" }),
  ).toBeInTheDocument();
  expect(
    within(global).queryByText("Domestic identity"),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText("Native account: minimax_code"),
  ).not.toBeInTheDocument();
});

it("opens the exact OpenRouter connection when two apps use the same provider ID", async () => {
  const providers = vi
    .spyOn(providersApi, "getAll")
    .mockImplementation(async (app) =>
      app === "codex" || app === "claude"
        ? ({
            shared: {
              id: "shared",
              name: `Connection ${app}`,
              icon: "openrouter",
              settingsConfig: {},
            },
          } as never)
        : {},
    );
  const onOpenApp = vi.fn();
  try {
    const user = userEvent.setup();
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <ServicesPage
          appId="codex"
          onOpenAddHub={vi.fn()}
          onOpenApp={onOpenApp}
        />
      </QueryClientProvider>,
    );
    await user.click(
      await screen.findByRole("button", { name: "查看 Connection codex" }),
    );
    const detail = await screen.findByRole("region", { name: "账号详情" });
    expect(
      within(detail).getByRole("heading", { name: "Connection codex" }),
    ).toBeVisible();
    expect(
      within(detail).queryByText("Connection claude"),
    ).not.toBeInTheDocument();
    await user.click(
      within(detail).getByRole("button", { name: "管理原有连接" }),
    );
    expect(onOpenApp).toHaveBeenLastCalledWith("codex");
    await user.click(screen.getByRole("tab", { name: "全部" }));
    await user.click(
      screen.getByRole("button", { name: "查看 Connection claude" }),
    );
    expect(
      within(detail).getByRole("heading", { name: "Connection claude" }),
    ).toBeVisible();
    await user.click(
      within(detail).getByRole("button", { name: "管理原有连接" }),
    );
    expect(onOpenApp).toHaveBeenLastCalledWith("claude");
  } finally {
    providers.mockRestore();
  }
});

it("marks the retained OpenCode summary unknown when its existing service inventory read fails", async () => {
  savedServices.push({
    kind: "vendor",
    id: 42,
    row: {
      id: 42,
      vendorId: "opencode",
      vendorName: "OpenCode",
      accountLabel: "Retained OpenCode",
      status: "ready",
      plans: [],
    },
    apps: new Map(),
  } as unknown as ServiceAccount);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const tree = () => (
    <QueryClientProvider client={client}>
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>
  );
  const view = render(tree());
  const group = await screen.findByRole("region", {
    name: "OpenCode · 账号总览",
  });
  expect(
    within(group).getByRole("button", { name: "查看 Retained OpenCode" }),
  ).toBeVisible();
  serviceRead.error = new Error("private inventory details");
  view.rerender(tree());
  expect(within(group).getByRole("status")).toHaveTextContent("当前状态未知");
  expect(
    within(group).getByRole("button", { name: "查看 Retained OpenCode" }),
  ).toBeVisible();
  expect(
    screen.queryByText("private inventory details"),
  ).not.toBeInTheDocument();
});
