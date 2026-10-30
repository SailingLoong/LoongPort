import { describe, expect, it } from "vitest";
import type { RelayRow } from "@/lib/api/relay";
import type { ServiceAccount } from "../useServiceAccounts";
import {
  projectServiceAccounts,
  projectSubscriptionWindows,
  projectAccountUsage,
} from "../serviceAccountDetailsProjection";

const row = (name: string) =>
  ({
    siteName: name,
    status: "usable",
    canQueryBalance: false,
    tiers: [],
  }) as unknown as RelayRow;
const account: ServiceAccount = {
  kind: "relay",
  id: 1,
  row: row("Example"),
  apps: new Map([
    ["codex", row("Codex row")],
    ["claude", row("Claude row")],
  ]),
};

describe("service account details projection", () => {
  it("uses the requested App or the account's own first App, never another account's configuration", () => {
    expect(projectServiceAccounts([account], "codex")[0]).toMatchObject({
      key: "relay:1",
      name: "Example",
      contextApp: "codex",
      actionTarget: {
        kind: "relay",
        row: account.apps.get("codex"),
        appId: "codex",
      },
    });
    expect(projectServiceAccounts([account], "gemini")[0].contextApp).toBe(
      "codex",
    );
  });
  it("does not construct an action target for an account without any App snapshot", () => {
    expect(
      projectServiceAccounts([{ ...account, apps: new Map() }], "codex"),
    ).toEqual([]);
  });
  it("deduplicates identical subscription content but keeps separate windows of the same kind, sorting numeric reset seconds", () => {
    const later = { kind: "daily", limitUsd: 10, usedUsd: null, resetAt: 200 };
    const earlier = { kind: "daily", limitUsd: 10, usedUsd: 2, resetAt: 100 };
    const unknown = { kind: "weekly", limitUsd: 20, usedUsd: 3, resetAt: null };
    const windows = projectSubscriptionWindows([
      { tiers: [{ subscriptionWindows: [later, unknown, earlier, later] }] },
    ] as any);
    expect(windows.map((window) => window.resetAt)).toEqual([100, 200, null]);
    expect(new Set(windows.map((window) => window.key)).size).toBe(3);
    expect(windows.map((window) => window.usedLimit)).toEqual([
      "2.00 / 10.00",
      "— / 10.00",
      "3.00 / 20.00",
    ]);
    expect(projectSubscriptionWindows([])).toEqual([]);
  });
  it("preserves original provider-window totals and sorts costs without treating them as balances or credit expiry", () => {
    const data = [
      { providerId: "p1", stats: { costUsd7d: 1, costUsd30d: 2 } },
      { providerId: "p2", stats: { costUsd7d: 3, costUsd30d: 8 } },
    ] as any;
    const result = projectAccountUsage(
      [
        { providerId: "p1", displayName: "One" },
        { providerId: "p2", displayName: "Two" },
        { providerId: "p3", displayName: "Zero" },
      ],
      data,
    );
    expect(result).toMatchObject({
      total7: 4,
      total30: 10,
      rows: [
        { providerId: "p2", cost30: 8, barWidth: "100%" },
        { providerId: "p1", cost30: 2, barWidth: "25%" },
      ],
    });
    expect(projectAccountUsage([], data)).toBeNull();
    expect(
      projectAccountUsage([{ providerId: "p1", displayName: "One" }], []),
    ).toBeNull();
  });
});
