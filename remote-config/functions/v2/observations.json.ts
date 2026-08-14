import {
  parseVeriDropLeaderboard,
  VERIDROP_LEADERBOARD_URL,
  type ObservationFeedV1,
  type VeriDropObservation,
} from "./observations.parser";

const CACHE_CONTROL = "public, max-age=300, stale-while-revalidate=900";
const FETCH_TIMEOUT_MS = 10_000;
const RESPONSE_HEADERS = {
  "Cache-Control": CACHE_CONTROL,
  "Content-Type": "application/json; charset=utf-8",
};

type PagesContext = {
  request: Request;
};

export async function onRequestGet(context: PagesContext): Promise<Response> {
  const cache = (caches as CacheStorage & { default: Cache }).default;
  const cacheKey = observationCacheKey(context.request);

  try {
    const upstream = await fetch(VERIDROP_LEADERBOARD_URL, {
      headers: { Accept: "text/html" },
      signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
    });
    if (!upstream.ok) {
      throw new Error("VeriDrop leaderboard request failed");
    }

    const feed = parseVeriDropLeaderboard(
      await upstream.text(),
      new Date().toISOString(),
    );
    if (!isNormalizedObservationFeed(feed)) {
      throw new Error("VeriDrop observations could not be normalized");
    }

    const response = jsonResponse(feed);
    await cache.put(cacheKey, response.clone());
    return response;
  } catch {
    const cachedFeed = await readCachedFeed(cache, cacheKey);
    if (cachedFeed) {
      return jsonResponse(cachedFeed);
    }

    return jsonResponse({ error: "Observation source is unavailable." }, 502);
  }
}

function observationCacheKey(request: Request): Request {
  const url = new URL(request.url);
  url.pathname = "/v2/observations.json";
  url.search = "";
  return new Request(url.toString());
}

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    headers: RESPONSE_HEADERS,
    status,
  });
}

async function readCachedFeed(
  cache: Cache,
  cacheKey: Request,
): Promise<ObservationFeedV1 | null> {
  try {
    const response = await cache.match(cacheKey);
    if (!response) {
      return null;
    }

    const value: unknown = await response.json();
    return isNormalizedObservationFeed(value) ? value : null;
  } catch {
    return null;
  }
}

function isNormalizedObservationFeed(
  value: unknown,
): value is ObservationFeedV1 {
  if (
    !hasExactKeys(value, [
      "schemaVersion",
      "sourceUrl",
      "fetchedAt",
      "observations",
    ])
  ) {
    return false;
  }

  const feed = value as Record<string, unknown>;
  return (
    feed.schemaVersion === 1 &&
    feed.sourceUrl === VERIDROP_LEADERBOARD_URL &&
    isIsoTimestamp(feed.fetchedAt) &&
    Array.isArray(feed.observations) &&
    feed.observations.every(isNormalizedObservation)
  );
}

function isNormalizedObservation(value: unknown): value is VeriDropObservation {
  if (
    !hasExactKeys(value, [
      "veridropHost",
      "rank",
      "score",
      "samples",
      "observedAt",
      "reportUrl",
      "issues",
    ])
  ) {
    return false;
  }

  const observation = value as Record<string, unknown>;
  return (
    isNormalizedHost(observation.veridropHost) &&
    isNonnegativeNumberOrNull(observation.rank) &&
    isNonnegativeNumberOrNull(observation.score) &&
    isNonnegativeNumberOrNull(observation.samples) &&
    isObservedDateOrNull(observation.observedAt) &&
    isVeriDropReportUrlOrNull(observation.reportUrl) &&
    isIssueList(observation.issues)
  );
}

function hasExactKeys(
  value: unknown,
  expectedKeys: string[],
): value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }

  const keys = Object.keys(value).sort();
  return (
    keys.length === expectedKeys.length &&
    keys.every((key, index) => key === expectedKeys.sort()[index])
  );
}

function isIsoTimestamp(value: unknown): value is string {
  return (
    typeof value === "string" && Number.isFinite(new Date(value).valueOf())
  );
}

function isNormalizedHost(value: unknown): value is string {
  if (
    typeof value !== "string" ||
    !value ||
    value !== value.toLowerCase() ||
    value.startsWith("www.")
  ) {
    return false;
  }

  try {
    return new URL(`https://${value}`).hostname === value;
  } catch {
    return false;
  }
}

function isNonnegativeNumberOrNull(value: unknown): value is number | null {
  return (
    value === null ||
    (typeof value === "number" && Number.isFinite(value) && value >= 0)
  );
}

function isObservedDateOrNull(value: unknown): value is string | null {
  return (
    value === null ||
    (typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value))
  );
}

function isVeriDropReportUrlOrNull(value: unknown): value is string | null {
  if (value === null) {
    return true;
  }
  if (typeof value !== "string") {
    return false;
  }

  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" &&
      url.hostname === "veridrop.org" &&
      url.search === "" &&
      url.hash === ""
    );
  } catch {
    return false;
  }
}

function isIssueList(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.every(
      (issue) =>
        typeof issue === "string" && issue.trim() === issue && issue.length > 0,
    ) &&
    new Set(value).size === value.length
  );
}
