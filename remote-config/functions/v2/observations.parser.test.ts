import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { parseVeriDropLeaderboard } from "./observations.parser";

const fixture = readFileSync(
  new URL("./__fixtures__/veridrop-leaderboard.html", import.meta.url),
  "utf8",
);

describe("parseVeriDropLeaderboard", () => {
  it("normalizes recognized public leaderboard rows into observations", () => {
    expect(
      parseVeriDropLeaderboard(fixture, "2026-08-14T09:00:00.000Z"),
    ).toEqual({
      schemaVersion: 1,
      sourceUrl: "https://veridrop.org/leaderboard/",
      fetchedAt: "2026-08-14T09:00:00.000Z",
      observations: [
        {
          veridropHost: "example-one.test",
          rank: 1,
          score: 98.5,
          samples: 120,
          observedAt: "2026-08-12",
          reportUrl: "https://veridrop.org/leaderboard/example-one.test",
          issues: ["slow response", "region drift"],
        },
        {
          veridropHost: "example-two.test",
          rank: 2,
          score: 88,
          samples: 40,
          observedAt: "2026-08-11",
          reportUrl: "https://veridrop.org/leaderboard/example-two.test",
          issues: ["missing tool"],
        },
      ],
    });
  });

  it("fails closed for unrecognized rows and nulls malformed optional values", () => {
    const html = `
      <article class="lb-row" data-impression-domain="www.example.test" data-impression-surface="leaderboard_top" data-impression-position="not-a-rank">
        <div class="lb-score-num">invalid</div>
        <div class="lb-meta">unknown samples; 最近 not-a-date</div>
        <a class="lb-detail-link" href="https://untrusted.example/report">Report</a>
        <div class="lb-issues"><code> stale issue </code><code>stale issue</code><code> </code></div>
      </article>
      <article class="lb-row" data-impression-domain="duplicate.example.test" data-impression-surface="not-leaderboard_top"></article>
    `;

    expect(
      parseVeriDropLeaderboard(html, "2026-08-14T09:00:00.000Z"),
    ).toMatchObject({
      observations: [
        {
          veridropHost: "example.test",
          rank: null,
          score: null,
          samples: null,
          observedAt: null,
          reportUrl: null,
          issues: ["stale issue"],
        },
      ],
    });
    expect(
      parseVeriDropLeaderboard(
        "<main>no leaderboard rows</main>",
        "2026-08-14T09:00:00.000Z",
      ).observations,
    ).toEqual([]);
  });
});
