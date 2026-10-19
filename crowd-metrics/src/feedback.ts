/**
 * Feedback candidate: D1 owns the report manifest and lifecycle, FEEDBACK KV owns
 * the submitted content for 30 days, and GitHub receives content-free notifications.
 * Public report/install IDs identify records but authorize no read, replacement, or deletion.
 * Anonymous attachment reads are closed. A proven maintainer retrieval route and
 * an explicit legacy migration deadline are deployment prerequisites, not implemented auth.
 * No client deletion endpoint or deletion credential exists.
 */

export interface FeedbackEnv {
  DB: D1Database;
  FEEDBACK: KVNamespace;
  /** GitHub 凭据（建私有仓 issue 用）。未配置 = 503。 */
  GH_FEEDBACK_TOKEN?: string;
  /** Explicit UTC migration deadline. Unset preserves old clients but blocks rollout approval. */
  FEEDBACK_LEGACY_UNTIL?: string;
}

/** 私有仓坐标写死在代码里：仓不敏感（公开代码出现名字无妨），token 才是凭据。 */
const GH_REPO = "SailingLoong/loongport-feedback";

export const MAX_BUNDLE_BYTES = 10 * 1024 * 1024;
export const MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024;
export const MAX_SCREENSHOTS = 6;
export const MAX_DESCRIPTION_CHARS = 8_000;
/** Size limit for the private metadata object; it is never copied into GitHub. */
export const MAX_META_BYTES = 32 * 1024;
/** 分诊摘要上限：客户端侧已按条数/单行截断，这里再兜一道体积闸。 */
export const MAX_SIGNALS_BYTES = 8 * 1024;
/** Stable registry order, checked against the desktop APP_IDS contract in tests. */
export const FEEDBACK_RELATED_APP_IDS = [
  "claude",
  "claude-desktop",
  "codex",
  "codex-image",
  "pi",
  "grokbuild",
  "gemini",
  "opencode",
  "openclaw",
  "hermes",
  "zcode",
  "other",
] as const;
export const MAX_FEEDBACK_CONTACT_CHARS = 200;
/** Bound input before regex/trim/code-point allocation; UTF-16 matches the desktop wire guard. */
export const MAX_FEEDBACK_CONTACT_RAW_UNITS = 1024;
/** 每来源 IP 每自然日（UTC）的提交上限。 */
export const MAX_SUBMITS_PER_IP_DAY = 5;
/** Absolute KV expiry is a safety net; D1/object/notification cleanup has one owner below. */
export const FEEDBACK_RETENTION_SECS = 30 * 86400;
/** A crashed upload loses its active slot at the next maintenance pass after this deadline. */
export const FEEDBACK_UPLOAD_LEASE_SECS = 10 * 60;
export const MAX_REPORTS_PER_INSTALL = 3;
/** Temporary global ceiling for the unauthenticated old wire format. */
export const MAX_LEGACY_REPORTS = 20;
const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

const SOURCE_RE = /^[0-9a-f]{32}$/;
const APP_VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,31}$/;
const KEY_PREFIX = "feedback/";

export type FeedbackForm =
  | {
      ok: true;
      sourceId: string;
      installId: string | null;
      reportId: string | null;
      appVersion: string;
      description: string;
      /** Optional private form details share the report object and its retention deadline. */
      relatedApps?: string[];
      contact?: string;
      meta: unknown;
      /** 分诊摘要（lastCrash / recentErrors）。可选增强字段，异常时降级为 null。 */
      signals: unknown | null;
      bundle: File | null;
      screenshots: File[];
    }
  | { ok: false; error: string };

/**
 * 解析可选的 signals 字段。**任何异常（缺省/超长/坏 JSON/非对象）都降级为
 * null 而不是拒收**：其余字段按白名单严格拒，唯独摘要是锦上添花——不能让
 * 用户的反馈因客户端摘要 bug 提交失败。
 */
function parseSignalsField(form: FormData): unknown | null {
  const text = form.get("signals");
  if (typeof text !== "string" || text.length === 0) return null;
  if (text.length > MAX_SIGNALS_BYTES) return null;
  try {
    const parsed = JSON.parse(text);
    if (
      typeof parsed !== "object" ||
      parsed === null ||
      Array.isArray(parsed)
    ) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/** 表单校验（纯函数，无 IO）。白名单形状 + 上限，与 validate.ts 同一原则。 */
export function parseFeedbackForm(form: FormData): FeedbackForm {
  const installValue = form.get("installId");
  const reportValue = form.get("reportId");
  const modern = installValue !== null || reportValue !== null;
  if (
    modern &&
    (typeof installValue !== "string" || !UUID_RE.test(installValue))
  ) {
    return { ok: false, error: "bad installId" };
  }
  if (
    modern &&
    (typeof reportValue !== "string" || !UUID_RE.test(reportValue))
  ) {
    return { ok: false, error: "bad reportId" };
  }
  const sourceValue = form.get("sourceId");
  if (
    (!modern || sourceValue !== null) &&
    (typeof sourceValue !== "string" || !SOURCE_RE.test(sourceValue))
  ) {
    return { ok: false, error: "bad sourceId" };
  }
  const sourceId = typeof sourceValue === "string" ? sourceValue : "";
  const appVersion = form.get("appVersion");
  if (typeof appVersion !== "string" || !APP_VERSION_RE.test(appVersion)) {
    return { ok: false, error: "bad appVersion" };
  }
  const description = form.get("description");
  if (
    typeof description !== "string" ||
    description.trim().length === 0 ||
    description.length > MAX_DESCRIPTION_CHARS
  ) {
    return { ok: false, error: "bad description" };
  }
  const metaText = form.get("meta");
  if (typeof metaText !== "string" || metaText.length > MAX_META_BYTES) {
    return { ok: false, error: "bad meta" };
  }
  let meta: unknown;
  try {
    meta = JSON.parse(metaText);
  } catch {
    return { ok: false, error: "bad meta json" };
  }
  if (typeof meta !== "object" || meta === null || Array.isArray(meta)) {
    return { ok: false, error: "bad meta shape" };
  }

  let relatedApps: string[] | undefined;
  const relatedValue = form.get("relatedApps");
  if (relatedValue !== null) {
    if (
      form.getAll("relatedApps").length !== 1 ||
      typeof relatedValue !== "string" ||
      relatedValue.length > MAX_META_BYTES
    )
      return { ok: false, error: "bad relatedApps" };
    let parsed: unknown;
    try {
      parsed = JSON.parse(relatedValue);
    } catch {
      return { ok: false, error: "bad relatedApps" };
    }
    if (
      !Array.isArray(parsed) ||
      parsed.some(
        (app) =>
          typeof app !== "string" ||
          !FEEDBACK_RELATED_APP_IDS.some((known) => known === app),
      )
    )
      return { ok: false, error: "bad relatedApps" };
    // Registry ordering makes repeated selections and click order hash identically.
    relatedApps = FEEDBACK_RELATED_APP_IDS.filter((app) =>
      parsed.includes(app),
    );
  }
  let contact: string | undefined;
  const contactValue = form.get("contact");
  if (contactValue !== null) {
    if (
      form.getAll("contact").length !== 1 ||
      typeof contactValue !== "string" ||
      contactValue.length > MAX_FEEDBACK_CONTACT_RAW_UNITS ||
      /\p{Cc}/u.test(contactValue)
    )
      return { ok: false, error: "bad contact" };
    // Match Rust str::trim's Unicode White_Space semantics; JS trim also strips
    // U+FEFF, which would make the desktop and server count different input.
    const normalized = contactValue.replace(
      /^\p{White_Space}+|\p{White_Space}+$/gu,
      "",
    );
    if ([...normalized].length > MAX_FEEDBACK_CONTACT_CHARS)
      return { ok: false, error: "bad contact" };
    if (normalized) contact = normalized;
  }

  const screenshots = form
    .getAll("screenshots")
    .filter((v): v is File => v instanceof File);
  if (screenshots.length > MAX_SCREENSHOTS) {
    return { ok: false, error: "too many screenshots" };
  }
  for (const shot of screenshots) {
    if (shot.size > MAX_SCREENSHOT_BYTES) {
      return { ok: false, error: "screenshot too large" };
    }
    if (!shot.type.startsWith("image/")) {
      return { ok: false, error: "screenshot not an image" };
    }
  }

  const bundleValue = form.get("bundle");
  const bundle = bundleValue instanceof File ? bundleValue : null;
  if (bundle && bundle.size > MAX_BUNDLE_BYTES) {
    return { ok: false, error: "bundle too large" };
  }

  return {
    ok: true,
    sourceId,
    installId: modern ? (installValue as string) : null,
    reportId: modern ? (reportValue as string) : null,
    appVersion,
    description,
    relatedApps,
    contact,
    meta,
    signals: parseSignalsField(form),
    bundle,
    screenshots,
  };
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}

/** epoch 秒 → UTC 日串（限流键与 KV key 的日期段共用同一口径）。 */
export function dayUtc(nowSec: number): string {
  return new Date(nowSec * 1000).toISOString().slice(0, 10);
}

async function ipHash(ip: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(ip),
  );
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * 反馈独立限流预算（feedback_ip_day 表）：与 ingest 的 upload_ip_hour 分开 ——
 * 反馈每次是 KV 写 + GitHub 建 issue，成本比桶写入贵一个量级。只存 IP 哈希
 * （与 ratelimit.ts 同一隐私纪律）。
 */
export async function allowFeedbackByIp(
  env: Pick<FeedbackEnv, "DB">,
  ip: string,
  day: string,
): Promise<boolean> {
  const hash = await ipHash(ip);
  const result = await env.DB.prepare(
    `INSERT INTO feedback_ip_day (ip_hash, day, count) VALUES (?1, ?2, 1)
     ON CONFLICT (ip_hash, day) DO UPDATE SET count = count + 1
     RETURNING count`,
  )
    .bind(hash, day)
    .first<{ count: number }>();
  return (result?.count ?? 0) <= MAX_SUBMITS_PER_IP_DAY;
}

function extensionForType(type: string): string {
  const map: Record<string, string> = {
    "image/png": "png",
    "image/jpeg": "jpg",
    "image/webp": "webp",
    "image/gif": "gif",
    "image/bmp": "bmp",
  };
  return map[type] ?? "bin";
}

/** GitHub stores notification metadata only, never the submitted description or diagnostics. */
export function buildIssueTitle(reportId: string): string {
  return `[App feedback] ${reportId}`;
}

export function buildIssueBody(reportId: string, receivedAt: number): string {
  return [
    `<!-- loongport-report:${reportId} -->`,
    `Report ID: ${reportId}`,
    `Received: ${new Date(receivedAt * 1000).toISOString()}`,
    "Status: received.",
  ].join("\n");
}

export interface FeedbackReportRow {
  report_id: string;
  generation: string;
  identity_kind: "install" | "legacy";
  identity_key: string;
  content_hash: string;
  created_at: number;
  expires_at: number;
  lease_until: number | null;
  put_pending: 0 | 1;
  state: "uploading" | "stored" | "cleanup" | "retired";
  asset_keys: string;
  issue_number: number | null;
  issue_state: "pending" | "creating" | "unknown" | "created" | "cleaned";
  retry_after: number;
}

/** One SQLite statement reserves capacity; public IDs never authorize replacing a row. */
export const RESERVE_REPORT_SQL = `
INSERT INTO feedback_reports
  (report_id, identity_kind, identity_key, content_hash, created_at, expires_at,
   state, asset_keys, issue_number, issue_state, generation, lease_until)
SELECT ?1, ?2, ?3, ?4, ?5, ?6, 'uploading', ?7, NULL, 'pending', ?10, ?11
WHERE (SELECT COUNT(*) FROM feedback_reports
       WHERE identity_key = ?3 AND state != 'retired') < ?8
  AND (?2 != 'legacy' OR
       (SELECT COUNT(*) FROM feedback_reports
        WHERE identity_kind = 'legacy' AND state != 'retired') < ?9)
ON CONFLICT(report_id) DO NOTHING
RETURNING *`;

async function reportById(
  env: FeedbackEnv,
  id: string,
  generation?: string,
): Promise<FeedbackReportRow | null> {
  if (generation !== undefined) {
    return env.DB.prepare(
      "SELECT * FROM feedback_reports WHERE report_id = ?1 AND generation = ?2",
    )
      .bind(id, generation)
      .first<FeedbackReportRow>();
  }
  return env.DB.prepare("SELECT * FROM feedback_reports WHERE report_id = ?1")
    .bind(id)
    .first<FeedbackReportRow>();
}

function githubHeaders(env: FeedbackEnv): Record<string, string> {
  return {
    authorization: `Bearer ${env.GH_FEEDBACK_TOKEN}`,
    accept: "application/vnd.github+json",
    "x-github-api-version": "2022-11-28",
    "user-agent": "loongport-metrics-worker",
    "content-type": "application/json",
  };
}

async function hashBytes(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

type ParsedForm = Extract<FeedbackForm, { ok: true }>;
type PreparedAsset = { key: string; bytes: ArrayBuffer; contentType: string };

async function prepareAssets(
  form: ParsedForm,
  reportId: string,
  generation: string,
  nowSec: number,
): Promise<{
  assets: PreparedAsset[];
  contentHash: string;
}> {
  const prefix = `${KEY_PREFIX}${dayUtc(nowSec)}/${reportId}-${generation}`;
  const report = new TextEncoder().encode(
    JSON.stringify({
      appVersion: form.appVersion,
      description: form.description,
      relatedApps: form.relatedApps,
      contact: form.contact,
      meta: form.meta,
      signals: form.signals,
    }),
  );
  const assets: PreparedAsset[] = [
    {
      key: `${prefix}.json`,
      bytes: new Uint8Array(report).buffer,
      contentType: "application/json",
    },
  ];
  if (form.bundle) {
    assets.push({
      key: `${prefix}.zip`,
      bytes: await form.bundle.arrayBuffer(),
      contentType: "application/zip",
    });
  }
  for (const [index, shot] of form.screenshots.entries()) {
    assets.push({
      key: `${prefix}-shot-${index + 1}.${extensionForType(shot.type)}`,
      bytes: await shot.arrayBuffer(),
      contentType: shot.type,
    });
  }
  // Hash content and order, not the current UTC day, so a retry crossing midnight still matches.
  const digests: { contentType: string; digest: string }[] = [];
  for (const asset of assets) {
    digests.push({
      contentType: asset.contentType,
      digest: await hashBytes(new Uint8Array(asset.bytes)),
    });
  }
  return {
    assets,
    contentHash: await hashBytes(
      new Uint8Array(new TextEncoder().encode(JSON.stringify(digests))),
    ),
  };
}

function receipt(row: FeedbackReportRow): Response {
  return jsonResponse(
    {
      accepted: true,
      reportId: row.report_id,
      issueNumber: row.issue_number,
      uploadedAt: new Date(row.created_at * 1000).toISOString(),
    },
    202,
  );
}

/** Unset is compatibility, not an unlimited-rollout approval. The deploy gate must set a deadline. */
export function legacyFeedbackAllowed(
  env: FeedbackEnv,
  nowSec: number,
): boolean {
  if (env.FEEDBACK_LEGACY_UNTIL === undefined) return true;
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(
      env.FEEDBACK_LEGACY_UNTIL,
    )
  )
    return false;
  const deadline = Date.parse(env.FEEDBACK_LEGACY_UNTIL);
  return Number.isFinite(deadline) && nowSec * 1000 < deadline;
}

async function createReportNotification(
  env: FeedbackEnv,
  row: FeedbackReportRow,
): Promise<void> {
  if (!env.GH_FEEDBACK_TOKEN) return;
  const claimed = await env.DB.prepare(
    `UPDATE feedback_reports SET issue_state = 'creating'
     WHERE report_id = ?1 AND generation = ?3 AND state = 'stored' AND issue_state = 'pending'
       AND expires_at > ?2 RETURNING *`,
  )
    .bind(row.report_id, Math.floor(Date.now() / 1000), row.generation)
    .first<FeedbackReportRow>();
  if (!claimed) return;
  let issueNumber: number | null = null;
  let outcome: "pending" | "unknown" | "created" = "unknown";
  try {
    const response = await fetch(
      `https://api.github.com/repos/${GH_REPO}/issues`,
      {
        method: "POST",
        headers: githubHeaders(env),
        signal: AbortSignal.timeout(10_000),
        body: JSON.stringify({
          title: buildIssueTitle(row.report_id),
          body: buildIssueBody(row.report_id, row.created_at),
        }),
      },
    );
    if (response.ok) {
      const data = (await response.json()) as { number?: unknown };
      if (
        typeof data.number === "number" &&
        Number.isSafeInteger(data.number) &&
        data.number > 0
      ) {
        issueNumber = data.number;
        outcome = "created";
      }
    } else if ([400, 401, 403, 404, 410, 422, 429].includes(response.status)) {
      // A definitive rejection can be retried. A network/5xx/invalid response cannot.
      outcome = "pending";
    }
  } catch {
    // The remote POST may have committed. Never blindly issue it again.
  }
  await env.DB.prepare(
    `UPDATE feedback_reports SET issue_state = ?2, issue_number = ?3
     WHERE report_id = ?1 AND generation = ?4 AND issue_state = 'creating'`,
  )
    .bind(row.report_id, outcome, issueNumber, row.generation)
    .run();
}

/** Reconcile uncertain notifications with an exact opaque marker; absence is never permission to re-POST. */
async function reconcileNotification(
  env: FeedbackEnv,
  row: FeedbackReportRow,
): Promise<void> {
  if (!env.GH_FEEDBACK_TOKEN) return;
  const marker = `<!-- loongport-report:${row.report_id} -->`;
  // A bounded recent-issue scan; old or unusually busy queues remain explicitly unresolved.
  const url = `https://api.github.com/repos/${GH_REPO}/issues?state=all&sort=created&direction=desc&per_page=100`;
  const response = await fetch(url, {
    headers: githubHeaders(env),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) return;
  const issues = (await response.json()) as unknown;
  if (!Array.isArray(issues)) return;
  const matches = issues.filter(
    (issue: { body?: unknown; number?: unknown }) =>
      typeof issue?.body === "string" &&
      issue.body.split("\n", 1)[0] === marker &&
      issue.body
        .split("\n")
        .includes(
          `Received: ${new Date(row.created_at * 1000).toISOString()}`,
        ) &&
      typeof issue.number === "number" &&
      Number.isSafeInteger(issue.number) &&
      issue.number > 0,
  );
  if (matches.length !== 1) return;
  await env.DB.prepare(
    `UPDATE feedback_reports SET issue_state = 'created', issue_number = ?2
     WHERE report_id = ?1 AND generation = ?3 AND issue_state IN ('creating', 'unknown')`,
  )
    .bind(row.report_id, matches[0].number, row.generation)
    .run();
}

async function cleanReport(
  env: FeedbackEnv,
  row: FeedbackReportRow,
  nowSec: number,
): Promise<void> {
  // Fence the writer before deleting. A retired generation never becomes active again.
  const claimed = await env.DB.prepare(
    `UPDATE feedback_reports
     SET state = CASE WHEN state = 'retired' THEN 'retired' ELSE 'cleanup' END
     WHERE report_id = ?1 AND generation = ?2 RETURNING *`,
  )
    .bind(row.report_id, row.generation)
    .first<FeedbackReportRow>();
  if (!claimed) return;
  const keys = JSON.parse(claimed.asset_keys) as unknown;
  if (
    !Array.isArray(keys) ||
    keys.some((key) => typeof key !== "string" || !key.startsWith(KEY_PREFIX))
  ) {
    throw new Error("invalid stored feedback manifest");
  }
  const expired = nowSec >= claimed.expires_at;
  let deleteFailed = false;
  for (const key of keys as string[]) {
    try {
      await env.FEEDBACK.delete(key);
    } catch {
      deleteFailed = true;
    }
  }
  if (deleteFailed && !expired) return;
  // All stored objects have the same absolute KV expiration. Once that deadline
  // has passed, a failed delete cannot justify retaining installation metadata forever.
  if (deleteFailed)
    console.error(
      "feedback expired; object deletion used absolute-expiry fallback",
    );
  let current = await reportById(env, row.report_id, row.generation);
  if (!current) return;
  // A rejected/abandoned put may still commit after the deletes above. Only its
  // owning writer can confirm success and clear this fence. Keep consuming this
  // manifest until then or the immutable KV expiration, never just a lease timer.
  if (current.put_pending && !expired) return;
  if (current.issue_state === "creating" || current.issue_state === "unknown") {
    try {
      await reconcileNotification(env, current);
    } catch {
      /* The remote notification may remain unknown, but contains no submitted content. */
    }
    current = await reportById(env, row.report_id, row.generation);
    if (!current) return;
    if (
      (current.issue_state === "creating" ||
        current.issue_state === "unknown") &&
      !expired
    )
      return;
  }
  if (current.issue_number !== null && current.issue_state !== "cleaned") {
    let cleaned = false;
    if (env.GH_FEEDBACK_TOKEN) {
      try {
        const response = await fetch(
          `https://api.github.com/repos/${GH_REPO}/issues/${current.issue_number}`,
          {
            method: "PATCH",
            headers: githubHeaders(env),
            signal: AbortSignal.timeout(10_000),
            body: JSON.stringify({
              title: "[App feedback] Expired report",
              body: "Status: report content expired. No report content was copied into this notification.",
              state: "closed",
            }),
          },
        );
        // A 404 may hide an authorization problem; do not call it a confirmed close.
        cleaned = response.ok;
      } catch {
        /* Report content expiry is independent of a content-free notification. */
      }
    }
    if (!cleaned && !expired) return;
    if (!cleaned)
      console.error(
        "feedback expired; content-free notification could not be closed",
      );
  } else if (
    current.issue_state === "unknown" ||
    current.issue_state === "creating"
  ) {
    console.error(
      "feedback expired; content-free notification remains unconfirmed",
    );
  }
  // Terminal at the original retention deadline, never an indefinite report-ID tombstone.
  // A content-free GitHub notice can remain; this is not a claim that its history was erased.
  await env.DB.prepare(
    "DELETE FROM feedback_reports WHERE report_id = ?1 AND generation = ?2 AND state IN ('cleanup', 'retired')",
  )
    .bind(row.report_id, row.generation)
    .run();
}

/** Scheduled and existing request-driven maintenance share this one lifecycle owner. */
export async function maintainFeedback(
  env: FeedbackEnv,
  nowSec: number,
): Promise<void> {
  let results: FeedbackReportRow[] = [];
  try {
    // One atomic transition releases every expired upload lease, independently of
    // the bounded external-I/O queue. Stored reports have no upload lease.
    await env.DB.prepare(
      `UPDATE feedback_reports SET state = 'retired', retry_after = 0
       WHERE state IN ('uploading', 'cleanup') AND lease_until <= ?1`,
    )
      .bind(nowSec)
      .run();
    const queried = await env.DB.prepare(
      `SELECT * FROM feedback_reports WHERE retry_after <= ?1 AND
       (expires_at <= ?1 OR state IN ('cleanup', 'retired')
        OR (state = 'stored' AND issue_state IN ('pending', 'creating', 'unknown')))
       ORDER BY retry_after, created_at LIMIT 2`,
    )
      .bind(nowSec)
      .all<FeedbackReportRow>();
    results = queried.results ?? [];
  } catch {
    console.error("feedback maintenance unavailable");
  }
  for (const row of results ?? []) {
    try {
      // Rotate the bounded queue so one failed issue cannot starve later expired content.
      const claimed = await env.DB.prepare(
        "UPDATE feedback_reports SET retry_after = ?2 WHERE report_id = ?1 AND generation = ?3 AND retry_after <= ?4 RETURNING *",
      )
        .bind(row.report_id, nowSec + 600, row.generation, nowSec)
        .first<FeedbackReportRow>();
      if (!claimed) continue;
      if (
        row.expires_at <= nowSec ||
        row.state === "cleanup" ||
        row.state === "retired"
      )
        await cleanReport(env, row, nowSec);
      else if (row.issue_state === "pending")
        await createReportNotification(env, row);
      else await reconcileNotification(env, row);
    } catch {
      // Keep the authoritative row for a later retry; do not log report data or credentials.
      console.error("feedback maintenance pending");
    }
  }
  try {
    // Absolute KV expiry is the terminal content guarantee. Do not let the two-row
    // external-I/O budget turn expired identifiers into an unbounded D1 backlog.
    // Notifications missed by this bounded pass contain no report content and may remain.
    await env.DB.prepare("DELETE FROM feedback_reports WHERE expires_at <= ?1")
      .bind(nowSec)
      .run();
  } catch {
    console.error("feedback expired manifest purge pending");
  }
}

export async function handleFeedback(
  request: Request,
  env: FeedbackEnv,
): Promise<Response> {
  if (!env.GH_FEEDBACK_TOKEN)
    return jsonResponse({ error: "feedback disabled" }, 503);
  const contentLength = Number(request.headers.get("content-length") ?? "0");
  if (
    contentLength >
    MAX_BUNDLE_BYTES + MAX_SCREENSHOTS * MAX_SCREENSHOT_BYTES + 64 * 1024
  ) {
    return jsonResponse({ error: "payload too large" }, 413);
  }
  let rawForm: FormData;
  try {
    rawForm = await request.formData();
  } catch {
    return jsonResponse({ error: "invalid multipart" }, 400);
  }
  const form = parseFeedbackForm(rawForm);
  if (!form.ok) return jsonResponse({ error: form.error }, 400);
  const nowSec = Math.floor(Date.now() / 1000);
  if (!form.installId && !legacyFeedbackAllowed(env, nowSec)) {
    return jsonResponse({ error: "feedback client update required" }, 503);
  }
  const reportId = form.reportId ?? crypto.randomUUID();
  const identityKind = form.installId ? "install" : "legacy";
  const identityKey = form.installId
    ? `install:${form.installId}`
    : `legacy:${form.sourceId}`;
  // Internal fencing version, not a credential: never sent to or trusted from a client.
  const generation = crypto.randomUUID();
  const { assets, contentHash } = await prepareAssets(
    form,
    reportId,
    generation,
    nowSec,
  );
  let existing = await reportById(env, reportId);
  if (existing)
    return existingReportResponse(existing, identityKey, contentHash, nowSec);
  const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
  if (!(await allowFeedbackByIp(env, ip, dayUtc(nowSec)))) {
    return jsonResponse({ error: "rate limited" }, 429);
  }
  let reserved: FeedbackReportRow | null;
  try {
    reserved = await env.DB.prepare(RESERVE_REPORT_SQL)
      .bind(
        reportId,
        identityKind,
        identityKey,
        contentHash,
        nowSec,
        nowSec + FEEDBACK_RETENTION_SECS,
        JSON.stringify(assets.map((asset) => asset.key)),
        MAX_REPORTS_PER_INSTALL,
        MAX_LEGACY_REPORTS,
        generation,
        nowSec + FEEDBACK_UPLOAD_LEASE_SECS,
      )
      .first<FeedbackReportRow>();
  } catch {
    // D1 write may have committed. Read before returning; never begin KV writes without proven reservation.
    existing = await reportById(env, reportId).catch(() => null);
    if (
      existing?.generation === generation &&
      existing.state === "uploading" &&
      existing.identity_key === identityKey &&
      existing.content_hash === contentHash
    ) {
      // This invocation owns the committed reservation; a lost response does not lose the slot.
      reserved = existing;
    } else if (existing) {
      return existingReportResponse(existing, identityKey, contentHash, nowSec);
    } else {
      return jsonResponse(
        { error: "feedback storage temporarily unavailable" },
        503,
      );
    }
  }
  if (!reserved) {
    existing = await reportById(env, reportId);
    if (existing)
      return existingReportResponse(existing, identityKey, contentHash, nowSec);
    return jsonResponse(
      { error: "feedback temporarily unavailable; please try later" },
      429,
    );
  }
  try {
    for (const asset of assets) {
      // The boundary is before issuing put, not after it: a crash or a lost KV
      // response must leave evidence that this generation can still write late.
      const writing = await env.DB.prepare(
        `UPDATE feedback_reports SET put_pending = 1
         WHERE report_id = ?1 AND generation = ?2 AND state = 'uploading'
           AND lease_until > ?3 AND put_pending = 0 RETURNING *`,
      )
        .bind(reportId, generation, Math.floor(Date.now() / 1000))
        .first<FeedbackReportRow>();
      if (!writing) throw new Error("feedback upload lease ended");
      await env.FEEDBACK.put(asset.key, asset.bytes, {
        expiration: reserved.expires_at,
        metadata: { ct: asset.contentType },
      });
      // A successful put promise confirms this write. Rejection is deliberately
      // not cleared: KV has no cancellation/fencing API shared with D1.
      await env.DB.prepare(
        "UPDATE feedback_reports SET put_pending = 0 WHERE report_id = ?1 AND generation = ?2",
      )
        .bind(reportId, generation)
        .run();
    }
    const confirmed = await env.DB.prepare(
      `UPDATE feedback_reports SET state = 'stored', lease_until = NULL
       WHERE report_id = ?1 AND generation = ?2 AND state = 'uploading'
         AND lease_until > ?3 AND put_pending = 0 RETURNING *`,
    )
      .bind(reportId, generation, Math.floor(Date.now() / 1000))
      .first<FeedbackReportRow>();
    if (!confirmed) throw new Error("feedback upload lease ended");
  } catch {
    // Do not erase a successfully committed stored transition after an uncertain D1 response.
    const checked = await reportById(env, reportId, generation).catch(
      () => null,
    );
    if (checked?.state === "stored") return receipt(checked);
    if (checked) {
      try {
        await cleanReport(env, checked, Math.floor(Date.now() / 1000));
      } catch {
        /* The manifest remains for the maintenance retry. */
      }
    }
    return jsonResponse({ error: "feedback upload not confirmed" }, 503);
  }
  const stored = await reportById(env, reportId, generation);
  if (!stored || stored.state !== "stored") {
    return jsonResponse({ error: "feedback upload not confirmed" }, 503);
  }
  try {
    await createReportNotification(env, stored);
  } catch {
    /* Durable report success does not depend on a GitHub notification. */
  }
  return receipt(
    (await reportById(env, reportId, generation).catch(() => null)) ?? stored,
  );
}

function existingReportResponse(
  row: FeedbackReportRow,
  identityKey: string,
  contentHash: string,
  nowSec: number,
): Response {
  if (row.identity_key !== identityKey || row.content_hash !== contentHash) {
    return jsonResponse({ error: "report ID conflict" }, 409);
  }
  if (
    row.expires_at <= nowSec ||
    row.state === "cleanup" ||
    row.state === "retired"
  ) {
    return jsonResponse({ error: "report expired or cleanup pending" }, 409);
  }
  if (row.state !== "stored")
    return jsonResponse({ error: "report upload pending" }, 409);
  return receipt(row);
}

/** Preserved path validator for compatibility; an opaque path is never read authorization. */
export function isValidAssetPath(assetPath: string): boolean {
  return /^[0-9a-zA-Z][0-9a-zA-Z._-]*\/[0-9a-zA-Z][0-9a-zA-Z._-]*$/.test(
    assetPath,
  );
}

/** No proven maintainer-auth route exists. Fail closed; never forward client credentials. */
export async function handleFeedbackAsset(
  _request: Request,
  _env: Pick<FeedbackEnv, "FEEDBACK">,
  _assetPath: string,
): Promise<Response> {
  return jsonResponse({ error: "not found" }, 404);
}
