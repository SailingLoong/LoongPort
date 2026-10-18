import { render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { ProviderCard } from "@/components/providers/ProviderCard";
import type { Provider } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";
const entry = vi.hoisted(() => vi.fn());
vi.mock("@/components/providers/qoder/QoderDailyClaim", () => ({
  QoderDailyClaim: (props: unknown) => {
    entry(props);
    return <span>qoder-explicit-entry</span>;
  },
}));
vi.mock("@/components/providers/ProviderActions", () => ({
  ProviderActions: () => null,
}));
vi.mock("@/components/ProviderIcon", () => ({ ProviderIcon: () => null }));
vi.mock("@/components/UsageFooter", () => ({ default: () => null }));
vi.mock("@/components/SubscriptionQuotaFooter", () => ({
  default: () => null,
}));
vi.mock("@/components/CopilotQuotaFooter", () => ({ default: () => null }));
vi.mock("@/components/CodexOauthQuotaFooter", () => ({ default: () => null }));
vi.mock("@/components/XaiOauthQuotaFooter", () => ({ default: () => null }));
vi.mock("@/lib/query/failover", () => ({
  useProviderHealth: () => ({ data: undefined }),
}));
vi.mock("@/lib/query/queries", () => ({
  useUsageQuery: () => ({ data: undefined }),
}));
function card(providerType: string) {
  const provider: Provider = {
    id: "qoder-fixture",
    name: "Synthetic",
    settingsConfig: {},
    meta: { providerType },
  };
  return (
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderCard
        provider={provider}
        appId="codex"
        isCurrent={false}
        isProxyRunning={false}
        mutationsDisabled={true}
        onSwitch={vi.fn()}
        onEdit={vi.fn()}
        onDelete={vi.fn()}
        onConfigureUsage={vi.fn()}
        onOpenWebsite={vi.fn()}
        onDuplicate={vi.fn()}
      />
    </QueryClientProvider>
  );
}
it("connects only the explicit Qoder Provider card and preserves mutation blocking", () => {
  render(card("qoder_sdk"));
  expect(screen.getByText("qoder-explicit-entry")).toBeInTheDocument();
  expect(entry).toHaveBeenCalledWith({
    providerId: "qoder-fixture",
    disabled: true,
  });
});
it("does not expose claim controls for generic inference credentials", () => {
  render(card("generic"));
  expect(screen.queryByText("qoder-explicit-entry")).not.toBeInTheDocument();
});
