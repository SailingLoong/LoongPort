import { parseHTML } from "linkedom";

export type VeriDropObservation = {
  veridropHost: string;
  rank: number | null;
  score: number | null;
  samples: number | null;
  observedAt: string | null;
  reportUrl: string | null;
  issues: string[];
};

export type ObservationFeedV1 = {
  schemaVersion: 1;
  sourceUrl: string;
  fetchedAt: string;
  observations: VeriDropObservation[];
};

export const VERIDROP_LEADERBOARD_URL = "https://veridrop.org/leaderboard/";

const VERIDROP_ORIGIN = "https://veridrop.org";
const datePattern = /\b(\d{4}-\d{2}-\d{2})\b/;

export function parseVeriDropLeaderboard(
  html: string,
  fetchedAt: string,
): ObservationFeedV1 {
  const { document } = parseHTML(html);
  const observations: VeriDropObservation[] = [];
  const observedHosts = new Set<string>();

  for (const row of document.querySelectorAll(
    'article.lb-row[data-impression-domain][data-impression-surface="leaderboard_top"]',
  )) {
    const veridropHost = normalizeHost(
      row.getAttribute("data-impression-domain"),
    );
    if (!veridropHost || observedHosts.has(veridropHost)) {
      continue;
    }

    observedHosts.add(veridropHost);
    const meta = row.querySelector(".lb-meta")?.textContent ?? null;

    observations.push({
      veridropHost,
      rank: parseNonnegativeNumber(
        row.getAttribute("data-impression-position"),
      ),
      score: parseNonnegativeNumber(
        row.querySelector(".lb-score-num")?.textContent,
      ),
      samples: parseSampleCount(meta),
      observedAt: parseObservedAt(meta),
      reportUrl: normalizeReportUrl(
        row
          .querySelector(".lb-domain a, .lb-detail-link")
          ?.getAttribute("href"),
      ),
      issues: parseIssues(row.querySelectorAll(".lb-issues code")),
    });
  }

  return {
    schemaVersion: 1,
    sourceUrl: VERIDROP_LEADERBOARD_URL,
    fetchedAt,
    observations,
  };
}

function normalizeHost(value: string | null): string | null {
  if (!value) {
    return null;
  }

  const candidate = value.trim();
  if (!candidate || /\s/.test(candidate)) {
    return null;
  }

  const urlValue = /^[a-z][a-z\d+.-]*:/i.test(candidate)
    ? candidate
    : `https://${candidate}`;

  try {
    const url = new URL(urlValue);
    if (url.protocol !== "https:" && url.protocol !== "http:") {
      return null;
    }

    const hostname = url.hostname.toLowerCase();
    if (!hostname) {
      return null;
    }

    return hostname.startsWith("www.") ? hostname.slice(4) : hostname;
  } catch {
    return null;
  }
}

function parseNonnegativeNumber(
  value: string | null | undefined,
): number | null {
  if (!value) {
    return null;
  }

  const match = value.replace(/,/g, "").match(/^\s*(\d+(?:\.\d+)?)\b/);
  if (!match) {
    return null;
  }

  const number = Number(match[1]);
  return Number.isFinite(number) && number >= 0 ? number : null;
}

function parseSampleCount(meta: string | null): number | null {
  if (!meta) {
    return null;
  }

  const match = meta.replace(/,/g, "").match(/(?:^|\s)(\d+(?:\.\d+)?)\s*次/);
  return match ? parseNonnegativeNumber(match[1]) : null;
}

function parseObservedAt(meta: string | null): string | null {
  const value = meta?.match(datePattern)?.[1];
  return value && isCanonicalObservedDate(value) ? value : null;
}

export function isCanonicalObservedDate(value: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    return false;
  }

  const parsed = new Date(`${value}T00:00:00.000Z`);
  return (
    Number.isFinite(parsed.valueOf()) &&
    parsed.toISOString().slice(0, 10) === value
  );
}

function normalizeReportUrl(value: string | null | undefined): string | null {
  if (!value) {
    return null;
  }

  try {
    const url = new URL(value, VERIDROP_LEADERBOARD_URL);
    if (!hasCanonicalVeriDropOrigin(url)) {
      return null;
    }

    url.search = "";
    url.hash = "";
    return url.href;
  } catch {
    return null;
  }
}

export function isCanonicalVeriDropReportUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      hasCanonicalVeriDropOrigin(url) && url.search === "" && url.hash === ""
    );
  } catch {
    return false;
  }
}

function hasCanonicalVeriDropOrigin(url: URL): boolean {
  return (
    url.origin === VERIDROP_ORIGIN &&
    url.username === "" &&
    url.password === "" &&
    url.port === ""
  );
}

function parseIssues(
  issueNodes: Iterable<{ textContent: string | null }>,
): string[] {
  const issues = new Set<string>();

  for (const issueNode of issueNodes) {
    const issue = issueNode.textContent?.replace(/\s+/g, " ").trim();
    if (issue) {
      issues.add(issue);
    }
  }

  return [...issues];
}
