import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { QoderAccountPanel } from "@/components/relay/accounts/QoderAccountPanel";
import { providersApi } from "@/lib/api/providers";
vi.mock("@/lib/api/providers", () => ({ providersApi: { getAll: vi.fn() } }));
vi.mock("@/components/providers/qoder/QoderInferenceIdentity", () => ({
  QoderInferenceIdentity: ({
    providerId,
    sourceGeneration,
  }: {
    providerId: string;
    sourceGeneration?: string;
  }) => (
    <button>
      identity:{providerId}:{sourceGeneration}
    </button>
  ),
}));
vi.mock("@/components/providers/qoder/QoderDailyClaim", () => ({
  QoderDailyClaim: ({ providerId }: { providerId: string }) => (
    <button>claim:{providerId}</button>
  ),
}));
beforeEach(() => vi.clearAllMocks());
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <QoderAccountPanel />
    </QueryClientProvider>,
  );
  return client;
}
it("uses original Codex Qoder rows only and their original identity and claim controls", async () => {
  vi.mocked(providersApi.getAll).mockResolvedValue({
    one: {
      id: "one",
      name: "Existing account",
      settingsConfig: { secret: "never-show" },
      meta: {
        providerType: "qoder_sdk",
        qoderCampaignState: { generation: "r1" },
      },
    },
    other: { id: "other", name: "Not Qoder", settingsConfig: {} },
  } as never);
  setup();
  expect(await screen.findByText("Existing account")).toBeInTheDocument();
  expect(providersApi.getAll).toHaveBeenCalledWith("codex");
  expect(
    screen.getByRole("button", { name: "identity:one:r1" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "claim:one" })).toBeInTheDocument();
  expect(screen.queryByText("Not Qoder")).not.toBeInTheDocument();
  expect(screen.queryByText("never-show")).not.toBeInTheDocument();
});
it("does not mistake a read failure for an empty account and supports retry", async () => {
  vi.mocked(providersApi.getAll)
    .mockRejectedValueOnce(new Error("private raw error"))
    .mockResolvedValueOnce({});
  setup();
  expect(await screen.findByRole("alert")).toHaveTextContent("读取失败");
  expect(screen.queryByText("private raw error")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  expect(await screen.findByText(/尚无已保存的 Qoder/)).toBeInTheDocument();
});
it("refreshes changed original account rows without creating an alternate account", async () => {
  vi.mocked(providersApi.getAll)
    .mockResolvedValueOnce({
      a: {
        id: "a",
        name: "Old account",
        settingsConfig: {},
        meta: { providerType: "qoder_sdk" },
      },
    } as never)
    .mockResolvedValueOnce({});
  setup();
  await screen.findByText("Old account");
  fireEvent.click(screen.getByRole("button", { name: "刷新账号" }));
  await waitFor(() =>
    expect(screen.queryByText("Old account")).not.toBeInTheDocument(),
  );
  expect(
    screen.queryByRole("button", { name: "claim:a" }),
  ).not.toBeInTheDocument();
});

it("keeps CN original account IDs separate and never mounts the international claim consumer for CN", async () => {
  vi.mocked(providersApi.getAll).mockResolvedValue({
    global: {
      id: "global",
      name: "Global account",
      settingsConfig: {},
      meta: { providerType: "qoder_sdk" },
    },
    cn: {
      id: "cn",
      name: "CN account",
      settingsConfig: { secret: "never-show-cn" },
      meta: {
        providerType: "qoder_cn",
        qoderCampaignState: { generation: "cn-generation" },
      },
    },
    legacy: { id: "legacy", name: "Unknown old source", settingsConfig: {} },
  } as never);
  setup();
  await screen.findByText("CN account");
  expect(
    screen.getByRole("button", { name: "identity:cn:cn-generation" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Qoder CN · 独立 PAT")).toBeInTheDocument();
  expect(screen.getByText(/CN 签到仍未接通/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "CN 活动状态" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "claim:cn" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "claim:global" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("Unknown old source")).not.toBeInTheDocument();
  expect(screen.queryByText("never-show-cn")).not.toBeInTheDocument();
});
