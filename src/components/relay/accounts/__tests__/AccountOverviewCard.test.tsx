import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { AccountOverviewCard } from "../AccountOverviewCard";
import type { PlatformAccountSummary } from "../useAccountPlatformPresence";

function setup(
  snapshot: unknown,
  options: Partial<PlatformAccountSummary> = {},
  unavailable = false,
) {
  const account = {
    id: "example",
    label: "Example account",
    source: "workbuddy",
    platform: "workbuddy",
    snapshot,
    ...options,
  } as PlatformAccountSummary;
  render(
    <AccountOverviewCard
      account={account}
      onOpen={() => {}}
      unavailable={unavailable}
    />,
  );
  return within(screen.getByRole("button", { name: "查看 Example account" }));
}

it("keeps sparse legacy WorkBuddy snapshots unknown instead of crashing", () => {
  const card = setup({
    kind: "workbuddy",
    value: { id: "example", label: "Example account" },
  });
  expect(card.getAllByText("未知")).toHaveLength(2);
  expect(card.getByText("未提供")).toBeVisible();
});

it("does not turn a missing authorization field into an unauthorized result", () => {
  const card = setup({
    kind: "minimax",
    value: { id: "example", status: "unknown" },
  });
  expect(card.queryByText("未授权")).not.toBeInTheDocument();
  expect(card.getAllByText("未知")).toHaveLength(2);
});

it("uses the balance snapshot including zero, never the last claim's credited amount", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      credits: { totalRemaining: 0 },
      credited: 999,
      claimState: "claimed",
      autoClaimEnabled: false,
    },
  });
  expect(card.getByText("0")).toBeVisible();
  expect(card.queryByText("999")).not.toBeInTheDocument();
  expect(card.getByText("未验证")).toBeVisible();
});

it("distinguishes stored inference credentials from a verified inference request", () => {
  const card = setup(
    { kind: "workbuddy", value: { credits: { totalRemaining: null } } },
    { inferenceState: "ready" },
  );
  expect(card.getByText("凭据就绪")).toBeVisible();
  expect(card.queryByText(/已接通|已连接|请求成功/)).not.toBeInTheDocument();
});

it("preserves backend MiniMax quota and automatic policy without inventing defaults", () => {
  const card = setup({
    kind: "minimax",
    value: {
      authorized: false,
      entitlement: { remainingCredits: "0" },
      autoClaimEnabled: true,
    },
  });
  expect(card.getByText("0")).toBeVisible();
  expect(card.getByText("未授权")).toBeVisible();
  expect(card.getByText("开启")).toBeVisible();
  expect(card.getByText("未验证")).toBeVisible();
});

it("shows a failed inventory read as unknown rather than current cached health", () => {
  const card = setup(
    {
      kind: "minimax",
      value: {
        authorized: true,
        entitlement: { remainingCredits: "50" },
        autoClaimEnabled: true,
      },
    },
    {},
    true,
  );
  expect(card.getAllByText("未知")).toHaveLength(4);
  expect(card.queryByText(/已授权|50|开启/)).not.toBeInTheDocument();
});
