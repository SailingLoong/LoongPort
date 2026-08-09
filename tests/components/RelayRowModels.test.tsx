import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RelayRow } from "@/components/relay/RelayRow";
import type { RelayRow as RelayRowData, TierInfo } from "@/lib/api/relay";

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
    loggedIn: true,
    sessionExpired: false,
    tiers: [tier(models)],
  };
}

function renderRow(models: string[], onSelectTierModel = vi.fn()) {
  render(
    <RelayRow
      relay={relay(models)}
      open
      onOpenChange={vi.fn()}
      busy={new Set()}
      onLogin={vi.fn()}
      onProvision={vi.fn()}
      onSwitchTier={vi.fn()}
      onSelectTierModel={onSelectTierModel}
      balance={null}
      onPurchase={vi.fn()}
      onCheckTier={vi.fn()}
      isCheckingTier={() => false}
      onResetTier={vi.fn()}
      onEditTier={vi.fn()}
      onDelete={vi.fn()}
    />,
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
