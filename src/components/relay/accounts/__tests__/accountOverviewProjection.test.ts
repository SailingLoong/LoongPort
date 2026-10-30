import { expect, it } from "vitest";
import { createInstance, type TFunction } from "i18next";
import producerText from "../../../../../tests/fixtures/workbuddy/synthetic-account-views.json.txt?raw";
import type { PlatformAccountSummary } from "../useAccountPlatformPresence";
import {
  projectAccountOverviewRow,
  sortAccountOverviewRows,
} from "../accountOverviewProjection";

const nowMs = 1800000000000;
const t = ((key: string, options?: Record<string, unknown>) => {
  let value = String(options?.defaultValue ?? key);
  for (const [name, data] of Object.entries(options ?? {}))
    value = value.replaceAll(`{{${name}}}`, String(data));
  return value;
}) as TFunction;
const context = { nowMs, locale: "en", t, freshness: "fresh" as const };
const row = (
  id: string,
  kind: string,
  value: unknown,
): PlatformAccountSummary =>
  ({
    source: kind,
    platform: kind === "minimax" ? "minimax_code" : "workbuddy",
    id,
    label: `Account ${id}`,
    snapshot: { kind, value },
  }) as PlatformAccountSummary;
const wb = (id: string, balance: number | null) =>
  row(id, "workbuddy", {
    id,
    credits: { totalRemaining: balance },
    claimState: "claimed",
    credited: 999,
    autoClaimEnabled: false,
  });
const mm = (entitlement: unknown) => row("mm", "minimax", { entitlement });
const quota = (fiveHour: number | null, weekly: number | null) => ({
  fiveHour: {
    remainingPercent: fiveHour,
    unlimited: false,
    resetAtMs: nowMs + 1000,
  },
  weekly: {
    remainingPercent: weekly,
    unlimited: false,
    resetAtMs: nowMs + 2000,
  },
});

it("retains an unavailable token quota instead of hiding its cached zeros", () => {
  expect(
    projectAccountOverviewRow(
      mm({
        remainingCredits: "0",
        hasTokenPlan: true,
        quotaState: "unavailable",
        quota: quota(0, 0),
      }),
      context,
    ).availability,
  ).toBe("unknown");
});
it.each([
  [null, "unknown"],
  [20, "available"],
  [0, "exhausted"],
] as const)(
  "retains independent WorkBuddy package rights %#",
  (remaining, expected) => {
    const account = wb("package", 0);
    (account.snapshot!.value as any).credits.packages = [
      { id: "synthetic", name: "synthetic", remaining, expireAt: nowMs + 1000 },
    ];
    expect(projectAccountOverviewRow(account, context).availability).toBe(
      expected,
    );
  },
);

it.each([
  [
    { remainingCredits: "0", hasTokenPlan: true, quota: quota(0, null) },
    "unknown",
  ],
  [
    { remainingCredits: "0", hasTokenPlan: true, quota: quota(0, 20) },
    "available",
  ],
  [
    { remainingCredits: "0", hasTokenPlan: true, quota: quota(0, 0) },
    "exhausted",
  ],
  [{ remainingCredits: "0", hasTokenPlan: false }, "exhausted"],
  [{ remainingCredits: "0", hasTokenPlan: null }, "unknown"],
  [
    { remainingCredits: null, hasTokenPlan: true, quota: quota(0, 0) },
    "unknown",
  ],
  [{ remainingCredits: "20", hasTokenPlan: null }, "available"],
  [{ remainingCredits: "unknown", hasTokenPlan: false }, "unknown"],
])(
  "hides only wholly confirmed exhaustion, retaining zero plus unknown/usable rights %#",
  (entitlement, expected) => {
    expect(
      projectAccountOverviewRow(mm(entitlement), context).availability,
    ).toBe(expected);
  },
);
it.each(["stale", "unknown"] as const)(
  "keeps %s readings unknown even when cached rights are all zero",
  (freshness) => {
    expect(
      projectAccountOverviewRow(wb("a", 0), { ...context, freshness })
        .availability,
    ).toBe("unknown");
  },
);
it("keeps unlimited rights and malformed numbers from being classified as exhausted", () => {
  expect(
    projectAccountOverviewRow(
      mm({
        remainingCredits: "0",
        hasTokenPlan: true,
        quota: {
          ...quota(0, 0),
          fiveHour: { unlimited: true, remainingPercent: 0 },
        },
      }),
      context,
    ).availability,
  ).toBe("available");
  expect(
    projectAccountOverviewRow(wb("negative", -1), context).availability,
  ).toBe("unknown");
  expect(
    projectAccountOverviewRow(wb("nan", Number.NaN), context).availability,
  ).toBe("unknown");
});
it("projects owner success/expiry facts without substituting total, expected reward, or reset timestamps", () => {
  const account = wb("a", 120);
  const value = account.snapshot!.value as any;
  value.lastSuccessfulClaim = {
    confirmedAtMs: nowMs - 1000,
    timeSource: "localReconciled",
    credited: 20,
    unit: "workbuddy:credits",
  };
  value.creditExpiryStatus = "known";
  value.nextKnownCreditExpiry = {
    expiresAtMs: nowMs + 1000,
    remaining: 30,
    unit: "workbuddy:credits",
  };
  const projected = projectAccountOverviewRow(account, context);
  expect(projected.lastSuccess).toContain("本地确认时间");
  expect(projected.lastSuccess).toContain("+20 积分");
  expect(projected.lastSuccess).not.toMatch(/999|120/);
  expect(projected.nextExpiry).toContain("剩余 30 积分");
  expect(projected.earliestKnownExpiryMs).toBe(nowMs + 1000);
  expect(
    projectAccountOverviewRow(
      mm({
        remainingCredits: "20",
        hasTokenPlan: true,
        quota: quota(20, 20),
        tokenPlanExpiresAt: nowMs + 10,
      }),
      context,
    ).earliestKnownExpiryMs,
  ).toBeNull();
});
it("unknown units stay unknown, and elapsed/stale credit expiry is pending with no sortable deadline", () => {
  const account = wb("a", 20);
  const value = account.snapshot!.value as any;
  value.lastSuccessfulClaim = {
    confirmedAtMs: null,
    timeSource: null,
    credited: 2,
    unit: null,
  };
  value.creditExpiryStatus = "known";
  value.nextKnownCreditExpiry = {
    expiresAtMs: nowMs,
    remaining: 2,
    unit: null,
  };
  const projected = projectAccountOverviewRow(account, context);
  expect(projected.lastSuccess).toContain("单位未提供");
  expect(projected.nextExpiry).toBe("到期数据待更新");
  expect(projected.earliestKnownExpiryMs).toBeNull();
  value.nextKnownCreditExpiry.expiresAtMs = nowMs + 1000;
  value.creditExpiryStatus = "stale";
  expect(
    projectAccountOverviewRow(account, context).earliestKnownExpiryMs,
  ).toBeNull();
});
it("sorts numerical expiries earliest first, unknown last, and equal values by stable rowKey", () => {
  const make = (id: string, time: number | null) => {
    const account = wb(id, 1);
    const value = account.snapshot!.value as any;
    value.creditExpiryStatus = time === null ? "unavailable" : "known";
    value.nextKnownCreditExpiry =
      time === null
        ? null
        : { expiresAtMs: time, remaining: 1, unit: "points" };
    return projectAccountOverviewRow(account, context);
  };
  const rows = [
    make("z", nowMs + 100),
    make("u", null),
    make("b", nowMs + 20),
    make("a", nowMs + 20),
  ];
  const before = rows.map((row) => row.rowKey);
  expect(
    sortAccountOverviewRows(rows, "expiryAsc").map((row) => row.accountId),
  ).toEqual(["a", "b", "z", "u"]);
  expect(rows.map((row) => row.rowKey)).toEqual(before);
});

it("treats a finite but out-of-range expiry timestamp as unknown for display and sorting", () => {
  const account = wb("bad-time", 1);
  const value = account.snapshot!.value as any;
  value.creditExpiryStatus = "known";
  value.nextKnownCreditExpiry = {
    expiresAtMs: Number.MAX_VALUE,
    remaining: 1,
    unit: "points",
  };
  const projected = projectAccountOverviewRow(account, context);
  expect(projected.nextExpiry).toBe("到期数据待更新");
  expect(projected.earliestKnownExpiryMs).toBeNull();
});
it("uses exact source/app row identity and never fabricates native selection or current status", () => {
  const account = wb("same", 20);
  const projected = projectAccountOverviewRow(account, context);
  expect(projected.rowKey).not.toBe(
    projectAccountOverviewRow(
      { ...account, source: "workbuddy-inference" },
      context,
    ).rowKey,
  );
  expect(projected.rowKey).not.toBe(
    projectAccountOverviewRow({ ...account, app: "codex" }, context).rowKey,
  );
  expect(projected.switchAction.enabled).toBe(false);
  expect(projected.switchAction.reason).toBe("当前尚无可验证的客户端切换能力");
  expect(projected).not.toHaveProperty("selection");
  expect(projected).not.toHaveProperty("isCurrent");
  expect(projected.detailTarget).toEqual({ platform: "workbuddy", id: "same" });
});

it.each([
  ["zh", "积分"],
  ["zh-TW", "點數"],
  ["en", "credits"],
  ["ja", "クレジット"],
])(
  "preserves original producer facts, units and unknown/stale semantics in %s",
  async (language, unit) => {
    const resources = {
      zh: (await import("@/i18n/locales/zh.json")).default,
      "zh-TW": (await import("@/i18n/locales/zh-TW.json")).default,
      en: (await import("@/i18n/locales/en.json")).default,
      ja: (await import("@/i18n/locales/ja.json")).default,
    };
    const i18n = createInstance();
    await i18n.init({
      lng: language,
      resources: Object.fromEntries(
        Object.entries(resources).map(([id, translation]) => [
          id,
          { translation },
        ]),
      ),
    });
    const source = JSON.parse(producerText);
    const make = (value: unknown) => row("producer", "workbuddy", value);
    const ctx = { ...context, t: i18n.t, locale: language };
    const known = projectAccountOverviewRow(make(source.known), ctx);
    expect(known.lastSuccess).toContain(`+20 ${unit}`);
    expect(known.nextExpiry).toContain(`30 ${unit}`);
    expect(known.earliestKnownExpiryMs).toBe(1800000010000);
    expect(known.availability).toBe("available");
    expect(known.switchAction.reason).toBe(
      i18n.t("accountsOverview.switchUnavailable"),
    );
    expect(known).not.toHaveProperty("isCurrent");
    const stale = projectAccountOverviewRow(make(source.stale), {
      ...ctx,
      nowMs: 1800000020000,
    });
    expect(stale.earliestKnownExpiryMs).toBeNull();
    expect(stale.nextExpiry).toBe(i18n.t("accountRewardGlance.expiryPending"));
  },
);
