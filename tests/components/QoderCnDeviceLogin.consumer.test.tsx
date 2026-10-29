import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { QoderCnDeviceLogin } from "@/components/providers/qoder/QoderCnDeviceLogin";
import { ServiceSourceList } from "@/components/relay/accounts/ServiceSourceList";
import { useApplicationOverview } from "@/components/applications/useApplicationOverview";
const api = vi.hoisted(() => ({
  start: vi.fn(),
  poll: vi.fn(),
  cancel: vi.fn(),
  context: vi.fn(),
  openExternal: vi.fn(),
  sources: vi.fn(),
  overview: vi.fn(),
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("@/lib/api/serviceSources", () => ({
  serviceSourcesApi: { list: api.sources },
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { get: api.overview },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: unknown) => {
    if (cmd === "qoder_cn_device_login_start") return api.start(args);
    if (cmd === "qoder_cn_device_login_poll") return api.poll(args);
    if (cmd === "qoder_cn_device_login_cancel") return api.cancel(args);
    if (cmd === "qoder_http_identity_context") return api.context(args);
    if (cmd === "open_external") return api.openExternal(args);
    throw new Error(`unexpected ${cmd}`);
  },
}));
const now = Date.UTC(2026, 9, 22);
const started = {
  status: "started",
  sessionId: "synthetic",
  providerId: "device-a",
  credentialGeneration: "revision-a",
  authUrl: "https://qoder.cn/device/selectAccounts?session=synthetic",
  expiresAtMs: now + 60_000,
  pollIntervalMs: 1000,
};
const ctx = {
  providerId: "device-a",
  credentialGeneration: "revision-a",
  source: "chinaQoderDevice",
  status: "ready",
  expiresAtMs: now + 120_000,
  allowedActions: [],
  allowedDeviceActions: ["login", "rebind"],
};
let version = "old";
const source = () => [
  {
    sourceId: "synthetic-relay",
    account: { kind: "relay", id: 7 },
    name: `source-${version}`,
    configurationName: null,
    accountLabel: null,
    serviceName: null,
    bindings: [],
  },
];
const overview = () => ({
  configurations: [
    {
      providerId: "device-a",
      name: `overview-${version}`,
      source: "custom",
      account: null,
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      model: null,
      selection: { kind: "provider" },
      canSelect: false,
    },
  ],
  recentProviderIds: [],
  isAdditive: false,
});
const providers = {};
function Overview() {
  const value = useApplicationOverview("codex", providers, vi.fn());
  return <p>{value.data?.configurations?.[0]?.name}</p>;
}
async function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["serviceSources"], source());
  client.setQueryData(["applicationOverview", "codex"], overview());
  client.setQueryData(["applicationOverview", "claude"], {
    configurations: [],
  });
  const view = render(
    <QueryClientProvider client={client}>
      <ServiceSourceList
        account={{ kind: "relay", id: 7 }}
        onOpenApp={vi.fn()}
      />
      <Overview />
      <QoderCnDeviceLogin providerId="device-a" sourceGeneration="revision-a" />
    </QueryClientProvider>,
  );
  await act(async () => {});
  api.sources.mockClear();
  api.overview.mockClear();
  return { client, view };
}
async function click(name: string) {
  await act(async () => fireEvent.click(screen.getByRole("button", { name })));
}
async function start() {
  await click("登录 / 重新授权");
  await click("在浏览器中授权");
}
async function tick() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
  });
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  version = "old";
  api.start.mockReset().mockResolvedValue(started);
  api.poll.mockReset();
  api.cancel.mockReset().mockResolvedValue({ status: "cancelled" });
  api.context.mockReset().mockResolvedValue(ctx);
  api.openExternal.mockReset().mockResolvedValue(undefined);
  api.sources.mockReset().mockImplementation(async () => source());
  api.overview.mockReset().mockImplementation(async () => overview());
});
afterEach(() => vi.useRealTimers());
it("refreshes mounted source and Codex overview after successful authorization", async () => {
  api.poll.mockImplementation(async () => {
    version = "new";
    return { status: "completed", providerId: "device-a" };
  });
  const { client } = await setup();
  await start();
  await tick();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  expect(screen.getByText("source-new")).toBeInTheDocument();
  expect(screen.getByText("overview-new")).toBeInTheDocument();
  expect(api.sources).toHaveBeenCalled();
  expect(api.overview).toHaveBeenCalledWith("codex");
  expect(
    client.getQueryState(["applicationOverview", "claude"])?.isInvalidated,
  ).toBe(false);
});
it.each(["failed", "wrong-provider"])(
  "does not refresh on %s",
  async (state) => {
    api.poll.mockResolvedValue(
      state === "failed"
        ? { status: "failed", code: "login_failed" }
        : { status: "completed", providerId: "other" },
    );
    await setup();
    await start();
    await tick();
    expect(api.sources).not.toHaveBeenCalled();
    expect(api.overview).not.toHaveBeenCalled();
    expect(screen.getByText("source-old")).toBeInTheDocument();
  },
);
it("does not refresh on cancellation without a completed receipt", async () => {
  api.poll.mockResolvedValue({ status: "pending" });
  await setup();
  await start();
  await click("取消本次授权");
  await tick();
  expect(api.sources).not.toHaveBeenCalled();
  expect(api.overview).not.toHaveBeenCalled();
});
it("refreshes after a confirmed late completion without reviving the old dialog", async () => {
  let resolve!: (v: unknown) => void;
  api.poll.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  await setup();
  await start();
  await tick();
  await click("取消本次授权");
  version = "new";
  await act(async () =>
    resolve({ status: "completed", providerId: "device-a" }),
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  expect(screen.getByText("source-new")).toBeInTheDocument();
  expect(screen.getByText("overview-new")).toBeInTheDocument();
  expect(
    within(screen.getByRole("dialog")).queryByText(
      "授权已保存，不代表真实推理已验证。请通过真实请求验证权益。",
    ),
  ).not.toBeInTheDocument();
});
it("does not refresh for a late completion belonging to another provider", async () => {
  let resolve!: (value: unknown) => void;
  api.poll.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  await setup();
  await start();
  await tick();
  await click("取消本次授权");
  await act(async () => resolve({ status: "completed", providerId: "other" }));
  expect(api.sources).not.toHaveBeenCalled();
  expect(api.overview).not.toHaveBeenCalled();
  expect(screen.getByText("source-old")).toBeInTheDocument();
});
