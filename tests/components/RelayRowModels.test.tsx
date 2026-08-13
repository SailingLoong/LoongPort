import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { RelayRow } from "@/components/relay/RelayRow";
import type { RelayRow as RelayRowData, TierInfo } from "@/lib/api/relay";

import { createTestQueryClient } from "../utils/testQueryClient";

// 行内的 `RowBalance` 用 react-query 拉余额 ⇒ 得有 provider。余额不是这些闸关心
// 的东西，让 `invoke` reject 即可（行会渲染失败态的用量条，不影响其它断言）。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {
    throw new Error("balance not stubbed in this test");
  }),
}));

function tier(models: string[]): TierInfo {
  return {
    providerId: "loongport-test",
    appId: "codex",
    groupName: "Pro",
    displayName: "Example · Pro",
    model: "gpt-a",
    models,
    rateMultiplier: 1,
    isCurrent: true,
    userEdited: false,
    allowImageGeneration: false,
  };
}

function relay(models: string[]): RelayRowData {
  return {
    id: 1,
    siteOrigin: "https://api.example.com",
    siteName: "Example",
    accountLabel: "user@example.com",
    status: "ready",
    isCurrent: true,
    canQueryBalance: true,
    canRefresh: true,
    canDelete: false,
    tiers: [tier(models)],
  };
}

function renderRow(models: string[], onSelectTierModel = vi.fn()) {
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <RelayRow
        relay={relay(models)}
        open
        onOpenChange={vi.fn()}
        busy={new Set()}
        onLogin={vi.fn()}
        onProvision={vi.fn()}
        onSwitchTier={vi.fn()}
        onSelectTierModel={onSelectTierModel}
        onPurchase={vi.fn()}
        onCheckTier={vi.fn()}
        isCheckingTier={() => false}
        onResetTier={vi.fn()}
        onEditTier={vi.fn()}
        onDelete={vi.fn()}
      />
    </QueryClientProvider>,
  );
  return onSelectTierModel;
}

describe("RelayRow Codex model list", () => {
  it("shows the fetched catalog and switches when a different model is clicked", () => {
    const onSelectTierModel = renderRow(["gpt-a", "gpt-b"]);

    expect(screen.getByRole("button", { name: "gpt-a" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "gpt-b" }));

    expect(onSelectTierModel).toHaveBeenCalledWith(
      expect.objectContaining({ providerId: "loongport-test", model: "gpt-a" }),
      "gpt-b",
    );
  });

  it("does not invent a model list for providers without a fetched catalog", () => {
    renderRow([]);

    expect(
      screen.queryByRole("button", { name: "gpt-a" }),
    ).not.toBeInTheDocument();
  });
});
