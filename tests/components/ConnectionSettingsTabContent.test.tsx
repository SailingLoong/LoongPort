import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConnectionSettingsTabContent } from "@/components/settings/ConnectionSettingsTabContent";

const mocks = vi.hoisted(() => ({
  running: false,
  start: vi.fn(),
  stop: vi.fn(),
  save: vi.fn(),
  global: {
    listenAddress: "127.0.0.1",
    listenPort: 15721,
    enableLogging: false,
  },
  config: {
    enabled: false,
    autoFailoverEnabled: false,
    maxRetries: 3,
    streamingFirstByteTimeout: 60,
    streamingIdleTimeout: 120,
    nonStreamingTimeout: 600,
    circuitFailureThreshold: 5,
    circuitSuccessThreshold: 2,
    circuitTimeoutSeconds: 60,
    circuitErrorRateThreshold: 0.5,
    circuitMinRequests: 10,
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: string | { defaultValue?: string }) =>
      typeof options === "string" ? options : (options?.defaultValue ?? key),
  }),
}));
vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: () => ({
    isRunning: mocks.running,
    isPending: false,
    startProxyServer: mocks.start,
    stopWithRestore: mocks.stop,
  }),
}));
vi.mock("@/lib/query/proxy", () => ({
  useAppProxyConfig: () => ({ data: mocks.config, isLoading: false }),
  useUpdateAppProxyOptions: () => ({
    mutateAsync: mocks.save,
    isPending: false,
  }),
  useProxyStatusQuery: () => ({
    data: {
      running: mocks.running,
      uptime_seconds: 0,
      active_targets: [],
      address: "127.0.0.1",
      port: 15721,
      active_connections: 0,
      total_requests: 0,
      success_requests: 0,
      failed_requests: 0,
      success_rate: 100,
      current_provider: null,
      current_provider_id: null,
      last_request_at: null,
      last_error: null,
      failover_count: 0,
    },
  }),
  useProxyTakeoverStatus: () => ({ data: {} }),
  useSetProxyTakeoverForApp: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useGlobalProxyConfig: () => ({ data: mocks.global }),
  useUpdateGlobalProxyConfig: () => ({
    mutateAsync: mocks.save,
    isPending: false,
  }),
}));
vi.mock("@/lib/query/failover", () => ({
  useFailoverQueue: () => ({
    data: [{ providerId: "old-tier", providerName: "Old tier", sortIndex: 0 }],
  }),
  useProviderHealth: () => ({ data: undefined }),
  useAutoFailoverEnabled: () => ({ data: false }),
}));

function renderTab() {
  return render(
    <ConnectionSettingsTabContent
      settings={{ proxyConfirmed: true } as never}
      onAutoSave={vi.fn()}
    />,
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.running = false;
});

describe("connection settings", () => {
  it("offers connection configuration without mode, strategy, model, queue, or failover controls", () => {
    renderTab();
    expect(
      screen.queryByRole("switch", { name: /省心/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /价格最低|响应最快/ }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getAllByRole("switch")).toHaveLength(2);
    expect(screen.queryByText("自动故障转移")).not.toBeInTheDocument();
    expect(
      screen.queryByText("proxy.failoverQueue.title"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("档位选择、排序和自动故障转移在应用页设置。"),
    ).toBeInTheDocument();
  });

  it("keeps the local service and address controls available", () => {
    renderTab();
    expect(screen.getByDisplayValue("127.0.0.1")).toBeEnabled();
    expect(screen.getByDisplayValue("15721")).toBeEnabled();
    expect(screen.getByText("代理服务")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("switch")[1]);
    expect(mocks.start).toHaveBeenCalledOnce();
  });

  it("allows configuring each app while the service is stopped", async () => {
    renderTab();
    for (const app of ["Claude", "Codex", "Gemini", "Grok Build"]) {
      // App names come from the existing application registry.
      const trigger = screen.getByRole("button", { name: app });
      fireEvent.click(trigger);
    }
    expect(screen.getAllByRole("switch")).toHaveLength(2);
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(
      screen.queryByText("proxy.failoverQueue.title"),
    ).not.toBeInTheDocument();
    const retries = screen.getAllByLabelText("最大重试次数");
    expect(retries).toHaveLength(4);
    expect(screen.getAllByLabelText("流式静默超时（秒）")).toHaveLength(4);
    expect(screen.getAllByLabelText("恢复成功阈值")).toHaveLength(4);
    for (const input of retries) expect(input).toBeEnabled();
    fireEvent.change(retries[1], { target: { value: "4" } });
    fireEvent.click(
      within(retries[1].closest('[role="region"]') as HTMLElement).getByRole(
        "button",
        { name: /^保存$/ },
      ),
    );
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        expect.objectContaining({ appType: "codex", maxRetries: 4 }),
      ),
    );
  });
});

it("does not show legacy queue priorities while the proxy is running", () => {
  mocks.running = true;
  renderTab();
  expect(screen.getByText("启用日志记录")).toBeInTheDocument();
  expect(
    screen.queryByText("proxy.failoverQueue.title"),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("Old tier")).not.toBeInTheDocument();
});
