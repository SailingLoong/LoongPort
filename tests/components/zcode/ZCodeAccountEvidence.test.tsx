import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { ZCodeAccountEvidence } from "@/components/zcode/ZCodeAccountEvidence";
import type { SessionCheckDisplay } from "@/lib/api/zcodeAccounts";

it.each([1e20, Number.MAX_VALUE, Number.POSITIVE_INFINITY, Number.NaN])(
  "keeps account evidence usable for an out-of-range official timestamp %s",
  (timestamp) => {
    const check = {
      state: "accepted" as const,
      reason: null,
      checkedAt: 100,
      source: "accountStartJwt" as const,
      latestFailure: null,
    };
    const value: SessionCheckDisplay = {
      selectedProfileId: "synthetic-account",
      business: { check, officialOwnerId: null, displayName: null },
      start: {
        check,
        entitlement: "available",
        effectiveAtSeconds: null,
        quota: check,
        serverTimeSeconds: timestamp,
        plans: [],
        buckets: [],
      },
      coding: {
        check,
        subscription: check,
        entitlement: "available",
        quota: check,
        subscriptions: [],
        limits: [],
      },
    };
    expect(() => render(<ZCodeAccountEvidence value={value} />)).not.toThrow();
    expect(
      screen.getByText("Official server time: Unknown"),
    ).toBeInTheDocument();
  },
);
