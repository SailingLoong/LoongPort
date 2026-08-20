import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import type { ComponentProps } from "react";
import { RelayRow } from "../RelayRow";
import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

/** 「查看用量」入口的渲染闸：资格由后端 `canViewUsage` 拥有，行里只消费。 */
function renderRow(
  relayOverrides: Partial<ComponentProps<typeof RelayRow>["relay"]> = {},
  onOpenUsage: (() => void) | undefined,
) {
  const relay: ComponentProps<typeof RelayRow>["relay"] = {
    id: 1,
    siteOrigin: "https://relay.example",
    siteName: "Relay",
    accountLabel: "account",
    status: "ready",
    isCurrent: false,
    canQueryBalance: true,
    canPurchase: true,
    canViewUsage: false,
    canRefresh: true,
    usageBlockers: [],
    removeConfirmation: "configured",
    tiers: [],
    ...relayOverrides,
  };
  const props: ComponentProps<typeof RelayRow> = {
    relay,
    open: false,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin: vi.fn(),
    onProvision: vi.fn(),
    onSwitchTier: vi.fn(),
    onSelectTierModel: vi.fn(),
    onPurchase: vi.fn(),
    onOpenUsage,
    onCheckTier: vi.fn(),
    isCheckingTier: () => false,
    verificationVerdictForTier: undefined,
    onVerifyTier: vi.fn(),
    isVerifyingTier: () => false,
    onResetTier: vi.fn(),
    onEditTier: vi.fn(),
    onDelete: vi.fn(),
  };
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <RelayRow {...props} />
    </QueryClientProvider>,
  );
}

describe("RelayRow 查看用量入口", () => {
  it("有资格（canViewUsage）时渲染入口并回调", async () => {
    const onOpenUsage = vi.fn();
    renderRow({ canViewUsage: true }, onOpenUsage);

    const button = screen.getByRole("button", {
      name: "loongport.row.openUsageHint",
    });
    await userEvent.click(button);
    expect(onOpenUsage).toHaveBeenCalledOnce();
  });

  it("无资格时整个入口不出现（不是禁用置灰）", () => {
    renderRow({ canViewUsage: false }, vi.fn());

    expect(
      screen.queryByRole("button", { name: "loongport.row.openUsageHint" }),
    ).not.toBeInTheDocument();
  });

  it("行级资格与处理器双闸：处理器缺席时即便行有资格也不渲染", () => {
    renderRow({ canViewUsage: true }, undefined);

    expect(
      screen.queryByRole("button", { name: "loongport.row.openUsageHint" }),
    ).not.toBeInTheDocument();
  });
});
