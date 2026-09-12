import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { AutoFailoverConfigPanel } from "@/components/proxy/AutoFailoverConfigPanel";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const config = {
  appType: "codex",
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
};
beforeEach(() => vi.clearAllMocks());

it("saves options through the narrow command without replaying stale routing toggles", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_proxy_config_for_app") return { ...config };
    return undefined;
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <AutoFailoverConfigPanel appType="codex" />
    </QueryClientProvider>,
  );
  const retries = await screen.findByLabelText("proxy.autoFailover.maxRetries");
  fireEvent.change(retries, { target: { value: "7" } });
  fireEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("update_proxy_options_for_app", {
      options: {
        appType: "codex",
        maxRetries: 7,
        streamingFirstByteTimeout: 60,
        streamingIdleTimeout: 120,
        nonStreamingTimeout: 600,
        circuitFailureThreshold: 5,
        circuitSuccessThreshold: 2,
        circuitTimeoutSeconds: 60,
        circuitErrorRateThreshold: 0.5,
        circuitMinRequests: 10,
      },
    }),
  );
  expect(
    vi
      .mocked(invoke)
      .mock.calls.some(
        ([command]) => command === "update_proxy_config_for_app",
      ),
  ).toBe(false);
  expect(
    vi
      .mocked(invoke)
      .mock.calls.slice(0, 2)
      .map(([command]) => command),
  ).toEqual(["get_proxy_config_for_app", "update_proxy_options_for_app"]);
  client.clear();
});
