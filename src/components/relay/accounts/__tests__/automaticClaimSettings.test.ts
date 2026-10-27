import { expect, it, vi, beforeEach } from "vitest";
import {
  automaticClaimSetting,
  setAutomaticClaimSetting,
  setZCodeAutomaticSelection,
} from "../automaticClaimSettings";
import type { PlatformAccountSummary } from "../useAccountPlatformPresence";
const api = vi.hoisted(() => ({
  wb: vi.fn(),
  mm: vi.fn(),
  trae: vi.fn(),
  qoder: vi.fn(),
  cn: vi.fn(),
  zcode: vi.fn(),
  zcodeRead: vi.fn(),
}));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: { setAutomatic: api.wb },
}));
vi.mock("@/lib/api/minimaxCode", () => ({
  minimaxCodeApi: { setAutomatic: api.mm },
}));
vi.mock("@/lib/api/traeAccounts", () => ({
  traeAccountsApi: { setAutomatic: api.trae },
}));
vi.mock("@/lib/api/qoderDaily", () => ({
  qoderDailyApi: { setAutomatic: api.qoder },
}));
vi.mock("@/lib/api/qoderCnCampaigns", () => ({
  qoderCnCampaignsApi: { setAutomatic: api.cn },
}));
vi.mock("@/lib/api/zcodeClaim", () => ({
  zcodeClaimApi: { setAuto: api.zcode, state: api.zcodeRead },
}));
beforeEach(() => vi.clearAllMocks());
const row = (snapshot: unknown) =>
  ({
    id: "synthetic",
    label: "Synthetic account",
    source: "example",
    platform: "workbuddy",
    snapshot,
  }) as PlatformAccountSummary;
it("rejects changed WorkBuddy policy versions instead of confirming consent", async () => {
  const value = {
    id: "synthetic",
    generation: 2,
    autoClaimRevision: 3,
    autoClaimPolicyVersion: 1,
  };
  api.wb.mockResolvedValue({
    ...value,
    autoClaimRevision: 4,
    autoClaimPolicyVersion: 2,
    autoClaimEnabled: true,
  });
  await expect(
    setAutomaticClaimSetting(row({ kind: "workbuddy", value }), true),
  ).rejects.toThrow();
});
it("retains identity guards and owner failure reasons for MiniMax and TRAE", async () => {
  const mm = row({
    kind: "minimax",
    value: {
      id: "synthetic",
      authorized: true,
      authGeneration: 2,
      autoClaimRevision: 3,
      autoClaimEnabled: false,
      autoClaimFailure: "needsLogin",
    },
  });
  expect(automaticClaimSetting(mm)).toMatchObject({
    enabled: false,
    canEnable: true,
    reason: "minimaxCode.errors.needsLogin",
  });
  api.mm.mockResolvedValue({
    ...mm.snapshot!.value,
    authGeneration: 3,
    autoClaimRevision: 4,
    autoClaimEnabled: true,
  });
  await expect(setAutomaticClaimSetting(mm, true)).rejects.toThrow();
  const trae = row({
    kind: "trae",
    value: {
      id: "synthetic",
      generation: 2,
      autoCheckinRevision: 3,
      autoCheckinEnabled: true,
      canSetAutomatic: false,
    },
  });
  expect(automaticClaimSetting(trae)).toMatchObject({
    enabled: true,
    canEnable: false,
    canDisable: true,
  });
  api.trae.mockResolvedValue({
    ...trae.snapshot!.value,
    autoCheckinRevision: 4,
    autoCheckinEnabled: false,
  });
  expect(
    automaticClaimSetting(await setAutomaticClaimSetting(trae, false)).enabled,
  ).toBe(false);
});
for (const source of [
  "internationalQoderPat",
  "chinaQoderPat",
  "chinaQoderDevice",
] as const)
  it(`retains Qoder consent source ${source}`, async () => {
    const value = {
      providerId: "synthetic",
      credentialGeneration: "g2",
      revision: 3,
      policyVersion: 2,
      enabled: false,
      canEnable: true,
      reason: null,
      subject: null,
    };
    const account = row({ kind: "qoder", value, source });
    (source === "internationalQoderPat" ? api.qoder : api.cn).mockResolvedValue(
      { ...value, revision: 4, enabled: true },
    );
    const next = await setAutomaticClaimSetting(account, true);
    expect(automaticClaimSetting(next).enabled).toBe(true);
    expect(
      source === "internationalQoderPat" ? api.qoder : api.cn,
    ).toHaveBeenCalledWith(
      value,
      true,
      ...(source === "internationalQoderPat" ? [] : [source]),
    );
  });
it("does not enable outsiders or discard active outsiders through ZCode's shared participant owner", async () => {
  const account = row({
    kind: "zcode",
    dataRoot: "synthetic-root",
    value: {
      consentRevision: "context:0",
      enabled: true,
      participants: ["synthetic"],
      records: {},
      busy: false,
    },
  });
  api.zcode.mockImplementation(async (_root, enabled, participants) => ({
    consentRevision: "context:1",
    enabled,
    participants,
    records: {},
    busy: false,
  }));
  api.zcodeRead.mockResolvedValue({
    consentRevision: "context:0",
    enabled: true,
    participants: ["outside", "synthetic"],
    records: {},
    busy: false,
  });
  const [off] = await setZCodeAutomaticSelection([account], false);
  expect(automaticClaimSetting(off).enabled).toBe(false);
  expect(api.zcode).toHaveBeenCalledWith(
    "synthetic-root",
    true,
    ["outside"],
    "context:0",
  );
  const disabled = row({
    kind: "zcode",
    value: {
      consentRevision: "context:0",
      enabled: false,
      participants: ["outside"],
      records: {},
      busy: false,
    },
  });
  api.zcodeRead.mockResolvedValue({
    consentRevision: "context:0",
    enabled: false,
    participants: ["outside"],
    records: {},
    busy: false,
  });
  const [on] = await setZCodeAutomaticSelection([disabled], true);
  expect(automaticClaimSetting(on).enabled).toBe(true);
  expect(api.zcode).toHaveBeenLastCalledWith(
    undefined,
    true,
    ["synthetic"],
    "context:0",
  );
});

it("rejects a mismatched Qoder context before calling its setter", async () => {
  const value = {
    providerId: "another-synthetic",
    credentialGeneration: "g",
    revision: 2,
    policyVersion: 2,
    enabled: false,
    canEnable: true,
  };
  const account = row({
    kind: "qoder",
    value,
    source: "internationalQoderPat",
  });
  api.qoder.mockResolvedValue({ ...value, revision: 3, enabled: true });
  await expect(setAutomaticClaimSetting(account, true)).rejects.toThrow();
  expect(api.qoder).not.toHaveBeenCalled();
});

it("does not rebase a stale ZCode grant onto a newer revocation", async () => {
  const old = {
    enabled: true,
    participants: ["synthetic"],
    records: {},
    busy: false,
    consentRevision: "context:1",
  };
  const account = row({
    kind: "zcode",
    dataRoot: "synthetic-root",
    value: old,
  });
  api.zcodeRead.mockResolvedValue({
    ...old,
    enabled: false,
    participants: [],
    consentRevision: "context:2",
  });
  api.zcode.mockImplementation(async (_root, enabled, participants) => ({
    ...old,
    enabled,
    participants,
    consentRevision: "context:3",
  }));
  await expect(setZCodeAutomaticSelection([account], true)).rejects.toThrow();
  expect(api.zcode).not.toHaveBeenCalled();
});

it("fails closed for missing or mixed ZCode consent snapshots", async () => {
  const value = { enabled: false, participants: [], records: {}, busy: false };
  const missing = row({ kind: "zcode", value });
  api.zcodeRead.mockResolvedValue(value);
  expect(automaticClaimSetting(missing).canEnable).toBe(false);
  await expect(setZCodeAutomaticSelection([missing], true)).rejects.toThrow();
  const a = row({
    kind: "zcode",
    value: { ...value, consentRevision: "context:1" },
  });
  const b = row({
    kind: "zcode",
    value: { ...value, consentRevision: "context:2" },
  });
  api.zcodeRead.mockResolvedValue(a.snapshot!.value);
  await expect(setZCodeAutomaticSelection([a, b], true)).rejects.toThrow();
  expect(api.zcode).not.toHaveBeenCalled();
});
