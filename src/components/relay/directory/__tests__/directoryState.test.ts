import { describe, expect, it } from "vitest";

import type { RelayDirectoryItem } from "@/lib/api/relay";
import {
  DIRECTORY_PAGE_SIZE,
  filterDirectoryItems,
  pageDirectoryItems,
  reduceDirectoryView,
  visibleDirectoryRange,
} from "../directoryState";

function item(index: number, overrides: Partial<RelayDirectoryItem> = {}) {
  return {
    siteHost: `site-${index}.example`,
    siteDomain: `site-${index}.example`,
    displayName: `站点 ${index}`,
    rank: index,
    entryUrl: `https://site-${index}.example`,
    ...overrides,
  } satisfies RelayDirectoryItem;
}

describe("relay directory state", () => {
  it("searches normalized names and hosts", () => {
    const items = [
      item(1, { displayName: "Best API", siteHost: "bestapi.store" }),
      item(2, { displayName: "鑫旺" }),
    ];

    expect(filterDirectoryItems(items, "  BESTAPI.STORE ")).toEqual([items[0]]);
    expect(filterDirectoryItems(items, "鑫旺")).toEqual([items[1]]);
    expect(filterDirectoryItems(items, "")).toEqual(items);
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

  it("returns to page one after changing the search", () => {
    const state = { search: "", page: 3 };

    expect(
      reduceDirectoryView(state, { type: "search", search: "best" }),
    ).toEqual({ search: "best", page: 1 });
    expect(reduceDirectoryView(state, { type: "page", page: 0 })).toEqual({
      search: "",
      page: 1,
    });
  });

  it("describes the visible range without inventing rows for an empty result", () => {
    expect(visibleDirectoryRange(2, 12, 25)).toEqual({ from: 13, to: 24 });
    expect(visibleDirectoryRange(1, 12, 0)).toEqual({ from: 0, to: 0 });
  });
});
