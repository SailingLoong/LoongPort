import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AccountOverviewRow } from "../AccountOverviewRow";
import { projectAccountOverviewRow } from "../accountOverviewProjection";
import { useTranslation } from "react-i18next";
function ProjectedRow({
  account,
  unavailable = false,
}: {
  account: PlatformAccountSummary;
  unavailable?: boolean;
  onOpen?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const row = projectAccountOverviewRow(account, {
    t,
    locale: i18n.resolvedLanguage || i18n.language,
    nowMs: Date.now(),
    freshness: unavailable ? "stale" : "fresh",
  });
  return <AccountOverviewRow row={row} onOpenDetails={() => {}} />;
}
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
    <ProjectedRow
      account={account}
      onOpen={() => {}}
      unavailable={unavailable}
    />,
  );
  return within(screen.getByRole("article", { name: account.label }));
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

it("shows owner-confirmed claim and actual expiry facts", () => {
  const now = Date.now();
  const card = setup({
    kind: "workbuddy",
    value: {
      claimState: "unconfirmed",
      credited: 999,
      lastSuccessfulClaim: {
        confirmedAtMs: now - 60000,
        timeSource: "localReconciled",
        credited: 100,
        unit: "points",
      },
      creditExpiryStatus: "known",
      nextKnownCreditExpiry: {
        expiresAtMs: now + 86400000,
        remaining: 120,
        unit: "points",
      },
    },
  });
  expect(card.getByText("最近成功")).toBeVisible();
  expect(card.getByText("即将到期")).toBeVisible();
  expect(card.getByText(/本地确认时间.*\+100 积分/)).toBeVisible();
  expect(card.getByText(/剩余 120 积分/)).toBeVisible();
  expect(card.queryByText(/\+999/)).not.toBeInTheDocument();
});

it("never turns checks, already-claimed status, balance or reset-like package dates into confirmed facts", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      claimState: "alreadyClaimed",
      credited: 900,
      credits: {
        totalRemaining: 1234,
        nearestExpiry: Date.now() + 100000,
        packages: [{ expireAt: Date.now() + 100000, remaining: 77 }],
      },
    },
  });
  expect(card.getByText("暂无可核实成功领取记录")).toBeVisible();
  expect(card.getByText("到期时间未提供")).toBeVisible();
  expect(card.queryByText(/\+900|剩余 77/)).not.toBeInTheDocument();
});

it("shows old confirmed WorkBuddy and MiniMax amounts without inventing a time or unit", () => {
  const wb = setup({
    kind: "workbuddy",
    value: {
      claimState: "claimed",
      credited: 25,
      lastCheckedAt: 1900000000000,
    },
  });
  expect(wb.getByText(/时间未记录.*\+25 单位未提供/)).toBeVisible();
  const mm = setup(
    {
      kind: "minimax",
      value: {
        operation: {
          status: "claimed",
          startedAt: 1900000000000,
          creditAmount: 50,
        },
        entitlement: { tokenPlanExpiresAt: 1900000000000 },
      },
    },
    { id: "minimax", label: "MiniMax" },
  );
  expect(mm.getByText(/时间未记录.*\+50 单位未提供/)).toBeVisible();
  expect(mm.getByText("到期时间未提供")).toBeVisible();
});

it.each(["alreadyClaimed", "pending", "unknown"])(
  "does not treat MiniMax %s as success",
  (status) => {
    const card = setup({
      kind: "minimax",
      value: { operation: { status, startedAt: Date.now(), creditAmount: 50 } },
    });
    expect(card.getByText("暂无可核实成功领取记录")).toBeVisible();
  },
);

it("keeps success time, value and unit independently unknown", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      lastSuccessfulClaim: {
        confirmedAtMs: Date.now() - 1000,
        timeSource: null,
        credited: 25,
        unit: null,
      },
    },
  });
  expect(card.getByText("时间未记录 · +25 单位未提供")).toBeVisible();
});

it("keeps a known date without an amount and supports a supplied unit", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      lastSuccessfulClaim: {
        confirmedAtMs: Date.now() - 1000,
        timeSource: "localReconciled",
        credited: null,
        unit: null,
      },
      creditExpiryStatus: "known",
      nextKnownCreditExpiry: {
        expiresAtMs: Date.now() + 10000,
        remaining: null,
        unit: null,
      },
    },
  });
  expect(card.getByText(/本地确认时间.*额度未提供/)).toBeVisible();
  expect(card.getByText(/剩余 额度未提供/)).toBeVisible();
});

it.each(["stale", "known"])(
  "shows expiry as pending for %s data without a valid future batch",
  (status) => {
    const card = setup({
      kind: "workbuddy",
      value: { creditExpiryStatus: status, nextKnownCreditExpiry: null },
    });
    expect(card.getByText("到期数据待更新")).toBeVisible();
  },
);

it("does not display expiry data when its owner says unavailable", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      creditExpiryStatus: "unavailable",
      nextKnownCreditExpiry: {
        expiresAtMs: Date.now() + 10000,
        remaining: 99,
        unit: "points",
      },
    },
  });
  expect(card.getByText("到期时间未提供")).toBeVisible();
  expect(card.queryByText(/剩余 99/)).not.toBeInTheDocument();
});

it.each([
  { expiresAtMs: Date.now() - 1000, remaining: 5, unit: "points" },
  { expiresAtMs: Number.NaN, remaining: 5, unit: "points" },
  { expiresAtMs: Date.now() + 10000, remaining: 0, unit: "points" },
  { expiresAtMs: Date.now() + 10000, remaining: -1, unit: "points" },
])(
  "does not show an expired, malformed or depleted batch",
  (nextKnownCreditExpiry) => {
    const card = setup({
      kind: "workbuddy",
      value: { creditExpiryStatus: "known", nextKnownCreditExpiry },
    });
    expect(card.getByText("到期数据待更新")).toBeVisible();
  },
);

it("hides cached facts when the owner read is unavailable", () => {
  const card = setup(
    {
      kind: "workbuddy",
      value: {
        claimState: "claimed",
        credited: 25,
        lastSuccessfulClaim: {
          confirmedAtMs: Date.now() - 1000,
          timeSource: "localReconciled",
          credited: 100,
          unit: "points",
        },
        creditExpiryStatus: "known",
        nextKnownCreditExpiry: {
          expiresAtMs: Date.now() + 10000,
          remaining: 120,
          unit: "points",
        },
      },
    },
    {},
    true,
  );
  expect(card.getByText("最近成功")).toBeVisible();
  expect(card.getByText("即将到期")).toBeVisible();
  expect(card.queryByText(/\+100|剩余 120|\+25/)).not.toBeInTheDocument();
});

it.each([
  ["zh", "最近成功", "即将到期", "暂无可核实成功领取记录", "到期时间未提供"],
  ["zh-TW", "最近成功", "即將到期", "暫無可核實成功領取紀錄", "到期時間未提供"],
  [
    "en",
    "Latest success",
    "Next expiry",
    "No verifiable successful claim",
    "Expiry time not provided",
  ],
  [
    "ja",
    "直近の成功",
    "次の期限",
    "確認できる受取成功の記録なし",
    "有効期限は未提供",
  ],
])(
  "renders the new reward rows in %s",
  async (language, lastLabel, expiryLabel, noSuccess, noExpiry) => {
    const i18next = (await import("i18next")).default.createInstance();
    const { I18nextProvider } = await import("react-i18next");
    const resources = {
      zh: (await import("@/i18n/locales/zh.json")).default,
      "zh-TW": (await import("@/i18n/locales/zh-TW.json")).default,
      en: (await import("@/i18n/locales/en.json")).default,
      ja: (await import("@/i18n/locales/ja.json")).default,
    };
    await i18next.init({
      lng: language,
      resources: Object.fromEntries(
        Object.entries(resources).map(([key, translation]) => [
          key,
          { translation },
        ]),
      ),
      interpolation: { escapeValue: false },
    });
    const account = {
      id: "example",
      label: "Example account",
      source: "workbuddy",
      platform: "workbuddy",
      snapshot: { kind: "workbuddy", value: {} },
    } as unknown as PlatformAccountSummary;
    render(
      <I18nextProvider i18n={i18next}>
        <ProjectedRow account={account} onOpen={() => {}} />
      </I18nextProvider>,
    );
    for (const text of [lastLabel, expiryLabel, noSuccess, noExpiry])
      expect(screen.getByText(text)).toBeVisible();
  },
);

it("localizes the confirmed WorkBuddy credit unit without changing unknown units", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      lastSuccessfulClaim: {
        confirmedAtMs: null,
        timeSource: null,
        credited: 100,
        unit: "workbuddy:credits",
      },
      creditExpiryStatus: "known",
      nextKnownCreditExpiry: {
        expiresAtMs: Date.now() + 86400000,
        remaining: 120,
        unit: "workbuddy:credits",
      },
    },
  });
  expect(card.getByText(/\+100 积分/)).toBeVisible();
  expect(card.getByText(/剩余 120 积分/)).toBeVisible();
  expect(card.queryByText(/workbuddy:credits/)).not.toBeInTheDocument();
});

it("leaves an unrecognized explicitly supplied unit unchanged", () => {
  const card = setup({
    kind: "workbuddy",
    value: {
      lastSuccessfulClaim: {
        confirmedAtMs: null,
        timeSource: null,
        credited: 100,
        unit: "example:tokens",
      },
    },
  });
  expect(card.getByText(/\+100 example:tokens/)).toBeVisible();
});

it.each([
  ["zh", "积分"],
  ["zh-TW", "點數"],
  ["en", "credits"],
  ["ja", "クレジット"],
])("displays the WorkBuddy unit in %s", async (language, unit) => {
  const i18next = (await import("i18next")).default.createInstance();
  const { I18nextProvider } = await import("react-i18next");
  const resources = {
    zh: (await import("@/i18n/locales/zh.json")).default,
    "zh-TW": (await import("@/i18n/locales/zh-TW.json")).default,
    en: (await import("@/i18n/locales/en.json")).default,
    ja: (await import("@/i18n/locales/ja.json")).default,
  };
  await i18next.init({
    lng: language,
    resources: Object.fromEntries(
      Object.entries(resources).map(([key, translation]) => [
        key,
        { translation },
      ]),
    ),
    interpolation: { escapeValue: false },
  });
  const account = {
    id: "example",
    label: "Example",
    source: "workbuddy",
    platform: "workbuddy",
    snapshot: {
      kind: "workbuddy",
      value: {
        lastSuccessfulClaim: {
          confirmedAtMs: null,
          timeSource: null,
          credited: 100,
          unit: "workbuddy:credits",
        },
        creditExpiryStatus: "known",
        nextKnownCreditExpiry: {
          expiresAtMs: Date.now() + 86400000,
          remaining: 120,
          unit: "workbuddy:credits",
        },
      },
    },
  } as unknown as PlatformAccountSummary;
  render(
    <I18nextProvider i18n={i18next}>
      <ProjectedRow account={account} onOpen={() => {}} />
    </I18nextProvider>,
  );
  expect(
    screen.getByText((text) => text.includes(`+100 ${unit}`)),
  ).toBeVisible();
  expect(screen.getByText(new RegExp(`120 ${unit}`))).toBeVisible();
  expect(screen.queryByText(/workbuddy:credits/)).not.toBeInTheDocument();
});

it("keeps row details independent from the disabled switch and non-interactive row container", () => {
  const onOpenDetails = vi.fn();
  const row = {
    rowKey: "exact-row",
    groupId: "workbuddy",
    accountId: "id",
    name: "Exact row",
    facts: [],
    lastSuccess: "Unknown",
    nextExpiry: "Unknown",
    earliestKnownExpiryMs: null,
    balance: null,
    availability: "unknown",
    claimIssue: null,
    detailTarget: { platform: "workbuddy", id: "id" },
    switchAction: {
      enabled: false,
      label: "Switch",
      reason: "Verified client capability unavailable",
    },
  } as const;
  render(
    <AccountOverviewRow
      row={{ ...row, facts: [] }}
      onOpenDetails={onOpenDetails}
    />,
  );
  const article = screen.getByRole("article", { name: "Exact row" });
  const details = within(article).getByRole("button", {
    name: "查看 Exact row",
  });
  const switchButton = within(article).getByRole("button", { name: "Switch" });
  expect(article.tagName).toBe("ARTICLE");
  expect(article.querySelector("button button")).toBeNull();
  fireEvent.click(article);
  fireEvent.click(switchButton);
  expect(onOpenDetails).not.toHaveBeenCalled();
  fireEvent.click(details);
  fireEvent.click(details);
  expect(onOpenDetails.mock.calls).toEqual([["exact-row"], ["exact-row"]]);
});
