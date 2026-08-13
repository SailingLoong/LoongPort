import { describe, expect, it } from "vitest";

import type { RelayDirectoryItem } from "@/lib/api/relay";
import {
  DIRECTORY_PAGE_SIZE,
  defaultDirectoryKind,
  filterDirectoryItems,
  pageDirectoryItems,
  reduceDirectoryView,
  visibleDirectoryRange,
} from "../directoryState";

function item(index: number, overrides: Partial<RelayDirectoryItem> = {}) {
  return {
    siteHost: `site-${index}.example`,
    veridropHost: `probe-${index}.example`,
    displayName: `站点 ${index}`,
    rank: index,
    score: 90,
    samples: 20,
    latestDate: "2026-08-13",
    detailUrl: `https://veridrop.org/leaderboard/site-${index}.example`,
    protocolScores: [],
    claudeSignatureRate: null,
    scenarios: [],
    issues: [],
    entryUrl: `https://site-${index}.example`,
    ...overrides,
  } satisfies RelayDirectoryItem;
}

describe("relay directory state", () => {
  it.each([
    ["claude", "claude"],
    ["claude-desktop", "claude"],
    ["codex", "openai"],
    ["codex-image", "openai"],
    ["gemini", "gemini"],
    ["grokbuild", "overall"],
    ["opencode", "overall"],
  ] as const)("maps %s to the %s leaderboard", (appId, kind) => {
    expect(defaultDirectoryKind(appId)).toBe(kind);
  });

  it("searches normalized names, hosts, scenarios, issues, and protocols", () => {
    const items = [
      item(1, {
        displayName: "Best API",
        siteHost: "bestapi.store",
        scenarios: ["Claude Code / Cursor 编程"],
        issues: ["token_usage"],
        protocolScores: [
          {
            protocol: "OpenAI",
            score: 95,
            samples: 27,
            verdict: "通过",
            reportUrl: null,
          },
        ],
      }),
      item(2, { displayName: "鑫旺" }),
    ];

    expect(filterDirectoryItems(items, "  BESTAPI.STORE ")).toEqual([items[0]]);
    expect(filterDirectoryItems(items, "cursor")).toEqual([items[0]]);
    expect(filterDirectoryItems(items, "TOKEN USAGE")).toEqual([items[0]]);
    expect(filterDirectoryItems(items, "openai")).toEqual([items[0]]);
  });

  it("paginates in fixed groups of twelve", () => {
    const items = Array.from({ length: 25 }, (_, index) => item(index + 1));

    expect(DIRECTORY_PAGE_SIZE).toBe(12);
    expect(pageDirectoryItems(items, 2)).toEqual({
      page: 2,
      totalPages: 3,
      items: items.slice(12, 24),
    });
    expect(pageDirectoryItems(items, 99)).toEqual({
      page: 3,
      totalPages: 3,
      items: items.slice(24),
    });
  });

  it("returns to page one after changing the tab or search", () => {
    const state = { kind: "claude" as const, search: "", page: 3 };

    expect(
      reduceDirectoryView(state, { type: "kind", kind: "openai" }),
    ).toEqual({ kind: "openai", search: "", page: 1 });
    expect(
      reduceDirectoryView(state, { type: "search", search: "best" }),
    ).toEqual({ kind: "claude", search: "best", page: 1 });
  });

  it("describes the visible range without inventing rows for an empty result", () => {
    expect(visibleDirectoryRange(2, 12, 25)).toEqual({ from: 13, to: 24 });
    expect(visibleDirectoryRange(1, 12, 0)).toEqual({ from: 0, to: 0 });
  });
});
