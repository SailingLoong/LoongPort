import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const actions = {
  canAdd: true,
  canImport: true,
  canBackup: true,
  canEditLabels: true,
  blockedReason: null,
};
const catalog = {
  revision: "catalog",
  profiles: [],
  current: null,
  pending: false,
  nativeUnconfirmed: false,
  actions,
};
const source = {
  installPath: "/synthetic/ZCode.app",
  dataRoot: "/synthetic/data",
  keyMode: "standard" as const,
};
const check = {
  state: "accepted",
  reason: null,
  checkedAt: 1234,
  source: "codingKey",
  latestFailure: null,
};

describe("ZCode independent account library", () => {
  it("checks exactly the explicitly selected saved account and cancels by its request ID", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      ...catalog,
      token: "private-secret",
    });
    expect(
      await zcodeAccountsApi.checkConnections({
        requestId: "request",
        dataRoot: "/synthetic/data",
        catalogRevision: "catalog",
        id: "opaque",
        allowOfficialCheck: true,
      }),
    ).toEqual(catalog);
    vi.mocked(invoke).mockResolvedValueOnce("tooLate");
    expect(await zcodeAccountsApi.cancelConnectionCheck("request")).toBe(
      "tooLate",
    );
    expect(vi.mocked(invoke).mock.calls).toEqual([
      [
        "check_zcode_account_connections",
        {
          requestId: "request",
          dataRoot: "/synthetic/data",
          catalogRevision: "catalog",
          id: "opaque",
          allowOfficialCheck: true,
        },
      ],
      ["cancel_zcode_connection_check", { requestId: "request" }],
    ]);
  });
  it("keeps bundle preview local and starts official checks only through the explicit consent command", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ previewId: "lease", rows: [] });
    await zcodeAccountsApi.previewBundle(
      undefined,
      "catalog",
      [1, 2],
      "  password  ",
    );
    const checked = {
      previewId: "lease",
      selected: [{ index: 0, updateDuplicate: false }],
      status: "ready",
      rows: [{ index: 0, capabilities: null, error: null }],
      completed: 1,
      total: 1,
      error: null,
    };
    vi.mocked(invoke).mockResolvedValueOnce({
      ...checked,
      token: "private-token",
    });
    expect(
      await zcodeAccountsApi.checkBundle(
        "lease",
        [{ index: 0, updateDuplicate: false }],
        true,
      ),
    ).toEqual(checked);
    vi.mocked(invoke).mockResolvedValueOnce(checked);
    await zcodeAccountsApi.bundleCheck("lease");
    vi.mocked(invoke).mockResolvedValueOnce(["saved"]);
    await zcodeAccountsApi.importBundle("/synthetic/data", "catalog", "lease", [
      { index: 0, updateDuplicate: false },
    ]);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      [
        "preview_zcode_account_bundle",
        { catalogRevision: "catalog", file: [1, 2], password: "  password  " },
      ],
      [
        "check_zcode_account_bundle",
        {
          previewId: "lease",
          selected: [{ index: 0, updateDuplicate: false }],
          allowOfficialCheck: true,
        },
      ],
      ["get_zcode_bundle_check_progress", { previewId: "lease" }],
      [
        "import_zcode_account_bundle",
        {
          dataRoot: "/synthetic/data",
          catalogRevision: "catalog",
          previewId: "lease",
          selected: [{ index: 0, updateDuplicate: false }],
        },
      ],
    ]);
  });
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });
  it("reads the library and edits labels without a native admission context", async () => {
    vi.mocked(invoke).mockResolvedValue(catalog);
    expect(await zcodeAccountsApi.library()).toEqual(catalog);
    await zcodeAccountsApi.library("/synthetic/data");
    await zcodeAccountsApi.setLabel(
      "/synthetic/data",
      "catalog",
      "opaque",
      "Local label",
    );
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["get_zcode_account_library", {}],
      ["get_zcode_account_library", { dataRoot: "/synthetic/data" }],
      [
        "set_zcode_account_label",
        {
          dataRoot: "/synthetic/data",
          catalogRevision: "catalog",
          id: "opaque",
          label: "Local label",
        },
      ],
    ]);
  });
  it("does not invent action eligibility when an older response omits actions", async () => {
    vi.mocked(invoke).mockResolvedValue({
      revision: "old",
      profiles: [],
      current: null,
      pending: false,
      nativeUnconfirmed: false,
    });
    const result = await zcodeAccountsApi.library();
    expect(result.actions).toMatchObject({
      canAdd: false,
      canImport: false,
      canBackup: false,
      canEditLabels: false,
    });
  });
  it("keeps safe capability facts, zero and unknown values, and separate quota instances", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...catalog,
      secret: "private-top",
      profiles: [
        {
          id: "opaque",
          family: "zai",
          label: "Personal",
          officialLabel: "a…",
          identitySource: "packageDeclared",
          sourceVerified: false,
          canActivate: true,
          activationBlockedReason: null,
          secret: "private-profile",
          capabilities: {
            selectedProfileId: "opaque",
            secret: "private-report",
            business: {
              check: { ...check, secret: "private-check" },
              officialOwnerId: null,
              displayName: null,
              token: "private-business",
            },
            start: {
              check,
              entitlement: "available",
              effectiveAtSeconds: 2,
              quota: check,
              serverTimeSeconds: 3,
              plans: [
                {
                  userPlanId: "plan-instance",
                  planId: "plan",
                  name: "Start",
                  status: "active",
                  startsAtSeconds: 1,
                  endsAtSeconds: 4,
                  entitlements: [],
                  raw: "private-plan",
                },
              ],
              buckets: [
                {
                  bucketId: "bucket-one",
                  userPlanId: "plan-instance",
                  planId: "plan",
                  entitlementId: "ent",
                  showName: "Monthly",
                  meter: "tokens",
                  unitType: "TOKENS",
                  capabilities: [],
                  totalUnits: 10,
                  usedUnits: 10,
                  reservedUnits: 0,
                  remainingUnits: 0,
                  availableUnits: 0,
                  periodStartSeconds: 1,
                  periodEndSeconds: 4,
                  expiresAtSeconds: 4,
                  raw: "private-bucket",
                },
                {
                  bucketId: "bucket-two",
                  userPlanId: "plan-instance-two",
                  planId: null,
                  entitlementId: null,
                  showName: null,
                  meter: null,
                  unitType: null,
                  capabilities: [],
                  totalUnits: null,
                  usedUnits: null,
                  reservedUnits: null,
                  remainingUnits: null,
                  availableUnits: null,
                  periodStartSeconds: null,
                  periodEndSeconds: null,
                  expiresAtSeconds: null,
                },
              ],
            },
            coding: {
              check,
              subscription: check,
              entitlement: "available",
              quota: check,
              subscriptions: [],
              limits: [
                {
                  limitType: "TOKENS_LIMIT",
                  unit: 3,
                  number: 5,
                  usage: 10,
                  currentValue: 10,
                  remaining: 0,
                  percentage: 100,
                  nextResetTimeMs: 12345,
                  usageDetails: [
                    {
                      modelCode: "model",
                      displayName: "Model",
                      usage: 0,
                      raw: "private-detail",
                    },
                  ],
                  raw: "private-limit",
                },
              ],
              key: "private-key",
            },
          },
        },
      ],
    });
    const result = await zcodeAccountsApi.library();
    expect(JSON.stringify(result)).not.toContain("private-");
    expect(result.profiles[0].canActivate).toBe(true);
    expect(result.profiles[0].identitySource).toBe("packageDeclared");
    expect(
      result.profiles[0].capabilities?.start.buckets.map(
        (b) => b.remainingUnits,
      ),
    ).toEqual([0, null]);
    expect(
      result.profiles[0].capabilities?.coding.limits[0].nextResetTimeMs,
    ).toBe(12345);
  });
  it("only reads a current identity explicitly and projects its context and observation time", async () => {
    vi.mocked(invoke).mockResolvedValue({
      contextRevision: "ctx",
      id: "opaque",
      label: "a…",
      family: "zai",
      readAt: 12345,
      token: "private-token",
    });
    expect(await zcodeAccountsApi.readCurrentIdentity(source, "ctx")).toEqual({
      contextRevision: "ctx",
      id: "opaque",
      label: "a…",
      family: "zai",
      readAt: 12345,
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "read_zcode_current_identity",
      { source, contextRevision: "ctx" },
    );
  });
});
