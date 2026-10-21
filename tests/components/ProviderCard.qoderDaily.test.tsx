import { render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { ProviderCard } from "@/components/providers/ProviderCard";
import type { Provider } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";
const entry = vi.hoisted(() => vi.fn());
const identityEntry = vi.hoisted(() => vi.fn());
vi.mock("@/components/providers/qoder/QoderInferenceIdentity", () => ({
  QoderInferenceIdentity: (props: unknown) => {
    identityEntry(props);
    return <span>qoder-identity-entry</span>;
  },
}));
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
function card(
  providerType: string,
  appId: "codex" | "claude" = "codex",
  generation?: string,
) {
  const meta = {
    providerType,
    qoderCampaignState: generation ? { generation, pending: null } : undefined,
  };
  const provider: Provider = {
    id: "qoder-fixture",
    name: "Synthetic",
    settingsConfig: {},
    meta,
  };
  return (
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderCard
        provider={provider}
        appId={appId}
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

it("mounts the inference identity beside Daily in the original Qoder slot", () => {
  render(card("qoder_sdk"));
  expect(screen.getByText("qoder-identity-entry")).toBeInTheDocument();
  expect(screen.getByText("qoder-explicit-entry")).toBeInTheDocument();
  expect(identityEntry).toHaveBeenCalledWith({
    providerId: "qoder-fixture",
    sourceGeneration: undefined,
    disabled: true,
  });
});
it.each(["generic", "codex_oauth"])(
  "does not mount inference identity for %s",
  (type) => {
    render(card(type));
    expect(screen.queryByText("qoder-identity-entry")).not.toBeInTheDocument();
  },
);
it("keeps Qoder identity specific to the original Codex Provider route", () => {
  render(card("qoder_sdk", "claude"));
  expect(screen.queryByText("qoder-identity-entry")).not.toBeInTheDocument();
});

it("passes only the non-secret original source generation as an invalidation signal", () => {
  const view = render(card("qoder_sdk", "codex", "generation-a"));
  expect(identityEntry).toHaveBeenLastCalledWith({
    providerId: "qoder-fixture",
    sourceGeneration: "generation-a",
    disabled: true,
  });
  view.rerender(card("qoder_sdk", "codex", "generation-b"));
  expect(identityEntry).toHaveBeenLastCalledWith({
    providerId: "qoder-fixture",
    sourceGeneration: "generation-b",
    disabled: true,
  });
});
