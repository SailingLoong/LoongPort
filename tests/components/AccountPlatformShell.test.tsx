import { act, render, screen } from "@testing-library/react";
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
    error: null,
    reload: vi.fn(),
  }),
  configuredApps: () => [],
}));
const defaultRead = native.getMockImplementation()!;
beforeEach(() => {
  savedServices.length = 0;
  localStorage.clear();
  native.mockReset().mockImplementation(defaultRead);
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
    await screen.findByText("Native account: workbuddy"),
  ).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "平台账号" })).toBeInTheDocument();
  expect(
    screen.getByRole("region", { name: "中转站与官方 API" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "全部" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.queryByRole("tab", { name: "Qoder" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  expect(screen.getByText("Native account: workbuddy")).toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(screen.getByText("Native account: workbuddy")).toBeInTheDocument();
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

it("preserves an open native manager through failed inventory reads, then removes it only on confirmed empty", async () => {
  const user = userEvent.setup();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  const session = await screen.findByRole("button", {
    name: "Native session workbuddy: 0",
  });
  await user.click(session);
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
    screen.queryByText("Native account: workbuddy"),
  ).not.toBeInTheDocument();
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
