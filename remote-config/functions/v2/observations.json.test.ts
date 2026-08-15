import { afterEach, describe, expect, it, vi } from "vitest";

import { onRequestGet } from "./observations.json";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("GET /v2/observations.json", () => {
  it.each([
    ["a noncanonical fetch timestamp", "2026-08-14", {}],
    [
      "an impossible observation date",
      "2026-08-14T09:00:00.000Z",
      { observedAt: "2026-02-30" },
    ],
    [
      "an issue with uncollapsed whitespace",
      "2026-08-14T09:00:00.000Z",
      { issues: ["stale   issue"] },
    ],
    [
      "a report link with embedded user info and a nondefault port",
      "2026-08-14T09:00:00.000Z",
      {
        reportUrl: "https://user:opaque-capability@veridrop.org:444/report",
      },
    ],
  ])(
    "returns 502 instead of stale data with %s",
    async (_label, fetchedAt, observationOverride) => {
      vi.stubGlobal(
        "fetch",
        vi.fn().mockRejectedValue(new Error("unavailable")),
      );
      vi.stubGlobal("caches", {
        default: {
          match: vi.fn().mockResolvedValue(
            new Response(
              JSON.stringify({
                schemaVersion: 1,
                sourceUrl: "https://veridrop.org/leaderboard/",
                fetchedAt,
                observations: [
                  {
                    veridropHost: "example.test",
                    rank: 1,
                    score: 98,
                    samples: 10,
                    observedAt: "2026-08-12",
                    reportUrl: "https://veridrop.org/leaderboard/example.test",
                    issues: ["stale issue"],
                    ...observationOverride,
                  },
                ],
              }),
            ),
          ),
          put: vi.fn(),
        },
      });

      const response = await onRequestGet({
        request: new Request("https://config.example/v2/observations.json"),
      });

      expect(response.status).toBe(502);
      expect(await response.json()).toEqual({
        error: "Observation source is unavailable.",
      });
    },
  );
});
