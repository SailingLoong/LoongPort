import type { RelayDirectoryItem } from "@/lib/api/relay";

export const DIRECTORY_PAGE_SIZE = 12;

export interface DirectoryViewState {
  search: string;
  page: number;
}

export type DirectoryViewAction =
  { type: "search"; search: string } | { type: "page"; page: number };

function normalizeSearch(value: string): string {
  return value
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, " ")
    .trim();
}

export function filterDirectoryItems(
  items: RelayDirectoryItem[],
  search: string,
): RelayDirectoryItem[] {
  const query = normalizeSearch(search);
  if (!query) return items;
  return items.filter((item) => {
    const haystack = normalizeSearch(
      [item.displayName, item.siteHost, item.siteDomain].join(" "),
    );
    return haystack.includes(query);
  });
}

export function pageDirectoryItems(
  items: RelayDirectoryItem[],
  requestedPage: number,
): { page: number; totalPages: number; items: RelayDirectoryItem[] } {
  const totalPages = Math.max(1, Math.ceil(items.length / DIRECTORY_PAGE_SIZE));
  const page = Math.min(Math.max(1, requestedPage), totalPages);
  const start = (page - 1) * DIRECTORY_PAGE_SIZE;
  return {
    page,
    totalPages,
    items: items.slice(start, start + DIRECTORY_PAGE_SIZE),
  };
}

export function reduceDirectoryView(
  state: DirectoryViewState,
  action: DirectoryViewAction,
): DirectoryViewState {
  switch (action.type) {
    case "search":
      return { ...state, search: action.search, page: 1 };
    case "page":
      return { ...state, page: Math.max(1, action.page) };
  }
}

export function visibleDirectoryRange(
  page: number,
  pageSize: number,
  total: number,
): { from: number; to: number } {
  if (total === 0) return { from: 0, to: 0 };
  return {
    from: (page - 1) * pageSize + 1,
    to: Math.min(page * pageSize, total),
  };
}
