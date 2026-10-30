import { describe, expect, it } from "vitest";
import { matchesVendorConnectionTarget } from "@/lib/vendorConnectionIdentity";
const receipt = {
  status: "prepared",
  operationState: "prepared",
  app: "codex",
  rowId: 1,
  plan: "p",
  operationId: "op",
  providerId: "provider",
  receiptRevision: 2,
  completedConfigurationRevision: null,
} as const;
const target = {
  app: "codex",
  rowId: 1,
  plan: "p",
  operationId: "op",
  providerId: "provider",
} as const;
describe("vendor receipt target identity", () => {
  it("matches full identity and allows optional expectation omission", () => {
    expect(matchesVendorConnectionTarget(receipt, target)).toBe(true);
    expect(
      matchesVendorConnectionTarget(receipt, {
        app: "codex",
        rowId: 1,
        plan: "p",
      }),
    ).toBe(true);
  });
  it.each([
    { app: "claude" },
    { rowId: 2 },
    { plan: "q" },
    { operationId: "other" },
    { providerId: "other" },
  ])("rejects mismatched expected identity %o", (bad) => {
    expect(
      matchesVendorConnectionTarget(receipt, { ...target, ...bad } as any),
    ).toBe(false);
  });
  it.each([
    { status: "none" },
    { status: "unknown" },
    { ...receipt, providerId: "" },
    { ...receipt, operationId: "" },
    { ...receipt, rowId: undefined },
    { ...receipt, plan: "" },
  ])("does not promote redacted or incomplete receipts %o", (value) => {
    expect(matchesVendorConnectionTarget(value as any, target)).toBe(false);
  });
});

it.each(["reviewed", "failedBeforeWrite"] as const)(
  "checks full reviewed identity without treating %s as completion",
  (status) => {
    const reviewed = { ...receipt, status, operationState: status };
    expect(matchesVendorConnectionTarget(reviewed, target)).toBe(true);
    expect(
      matchesVendorConnectionTarget(reviewed, {
        ...target,
        providerId: "other",
      }),
    ).toBe(false);
  },
);
