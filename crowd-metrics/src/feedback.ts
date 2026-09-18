/**
 * POST /v1/feedback：客户端问题反馈回传（multipart）。
 * GET  /v1/feedback-asset/<...>：按不可猜 key 读回附件（issue 正文内嵌 / 下载用）。
 *
 * 载荷形状（与客户端 commands/feedback.rs 同一契约）：
 * - 文本字段 sourceId（32 hex）/ appVersion / description / meta（环境事实 JSON）
 * - 图片部件 screenshots（≤6 张、单张 ≤5MB、image/*）
 * - 可选 bundle 部件（诊断包 zip ≤10MB）
 *
 * 流：体积闸 → 表单校验 → IP-日限流 → 附件存 R2（不可猜 key）→ 建私有仓 issue。
 *
 * ## 为什么附件自存 R2 而不是 GitHub
 *
 * GitHub 官方 API 没有附件上传端点（issue 级与 user 级都没有，网页端走未公开
 * 内部端点）。附件存 R2、以不可猜 URL 引进 issue 正文 —— 这与 GitHub 私有仓
 * 原生附件的安全模型等同（user-images CDN + 匿名不可猜 URL），不是降级。
 */

export interface FeedbackEnv {
  DB: D1Database;
  FEEDBACK: R2Bucket;
  /** fine-grained PAT：仅 SailingLoong/loongport-feedback 私有仓 Issues 读写。未配置 = 503。 */
  GH_FEEDBACK_TOKEN?: string;
}

/** 私有仓坐标写死在代码里：仓不敏感（公开代码出现名字无妨），token 才是凭据。 */
const GH_REPO = "SailingLoong/loongport-feedback";

export const MAX_BUNDLE_BYTES = 10 * 1024 * 1024;
export const MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024;
export const MAX_SCREENSHOTS = 6;
export const MAX_DESCRIPTION_CHARS = 8_000;
/** 环境摘要上限：issue 正文总长 65536 字符，meta + description 要留得住。 */
export const MAX_META_BYTES = 32 * 1024;
/** 每来源 IP 每自然日（UTC）的提交上限。 */
export const MAX_SUBMITS_PER_IP_DAY = 5;
/** 附件保留期：过期的 R2 对象由 scheduled 清理（issue 不删）。 */
export const FEEDBACK_RETENTION_SECS = 90 * 86400;

const SOURCE_RE = /^[0-9a-f]{32}$/;
const APP_VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,31}$/;
const ASSET_ROUTE_PREFIX = "/v1/feedback-asset/";
const R2_PREFIX = "feedback/";

export type FeedbackForm =
  | {
      ok: true;
      sourceId: string;
      appVersion: string;
      description: string;
      meta: unknown;
      bundle: File | null;
      screenshots: File[];
    }
  | { ok: false; error: string };

/** 表单校验（纯函数，无 IO）。白名单形状 + 上限，与 validate.ts 同一原则。 */
export function parseFeedbackForm(form: FormData): FeedbackForm {
  const sourceId = form.get("sourceId");
  if (typeof sourceId !== "string" || !SOURCE_RE.test(sourceId)) {
    return { ok: false, error: "bad sourceId" };
  }
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
    appVersion,
    description,
    meta,
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

/** epoch 秒 → UTC 日串（限流键与 R2 key 的日期段共用同一口径）。 */
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
 * 反馈每次是 R2 写 + GitHub 建 issue，成本比桶写入贵一个量级。只存 IP 哈希
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

function assetUrl(origin: string, key: string): string {
  return `${origin}${ASSET_ROUTE_PREFIX}${key.slice(R2_PREFIX.length)}`;
}

export function buildIssueTitle(description: string): string {
  const firstLine = description.split("\n", 1)[0] ?? "";
  return `[App 反馈] ${firstLine.slice(0, 50)}`;
}

export function buildIssueBody(
  form: Extract<FeedbackForm, { ok: true }>,
  imageUrls: string[],
  bundleUrl: string | null,
): string {
  const lines: string[] = [form.description.trim(), ""];
  if (imageUrls.length > 0) {
    lines.push("### 截图", "");
    for (const url of imageUrls) lines.push(`![截图](${url})`);
    lines.push("");
  }
  if (bundleUrl) {
    lines.push("### 诊断包", "", `[diagnostics.zip](${bundleUrl})`, "");
  }
  lines.push(
    "### 环境",
    "",
    "```json",
    JSON.stringify(form.meta, null, 2),
    "```",
    "",
    `sourceId: \`${form.sourceId}\` · appVersion: \`${form.appVersion}\``,
  );
  return lines.join("\n");
}

async function createIssue(
  env: FeedbackEnv,
  title: string,
  body: string,
): Promise<{ ok: true; number: number; url: string } | { ok: false; error: string }> {
  let resp: Response;
  try {
    resp = await fetch(`https://api.github.com/repos/${GH_REPO}/issues`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${env.GH_FEEDBACK_TOKEN}`,
        accept: "application/vnd.github+json",
        "x-github-api-version": "2022-11-28",
        "user-agent": "loongport-metrics-worker",
        "content-type": "application/json",
      },
      body: JSON.stringify({ title, body }),
    });
  } catch (error) {
    return { ok: false, error: `github network: ${error}` };
  }
  if (!resp.ok) {
    return { ok: false, error: `github ${resp.status}` };
  }
  const data = (await resp.json()) as { number?: number; html_url?: string };
  if (typeof data.number !== "number" || typeof data.html_url !== "string") {
    return { ok: false, error: "github response shape" };
  }
  return { ok: true, number: data.number, url: data.html_url };
}

export async function handleFeedback(
  request: Request,
  env: FeedbackEnv,
): Promise<Response> {
  if (!env.GH_FEEDBACK_TOKEN) {
    return jsonResponse({ error: "feedback disabled" }, 503);
  }

  // 体积闸：先信 Content-Length 快速拒绝，解析后再按部件复核。
  const contentLength = Number(request.headers.get("content-length") ?? "0");
  if (contentLength > MAX_BUNDLE_BYTES + MAX_SCREENSHOTS * MAX_SCREENSHOT_BYTES + 64 * 1024) {
    return jsonResponse({ error: "payload too large" }, 413);
  }

  let form: FormData;
  try {
    form = await request.formData();
  } catch {
    return jsonResponse({ error: "invalid multipart" }, 400);
  }
  const parsed = parseFeedbackForm(form);
  if (!parsed.ok) {
    return jsonResponse({ error: parsed.error }, 400);
  }

  const nowSec = Math.floor(Date.now() / 1000);
  const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
  if (!(await allowFeedbackByIp(env, ip, dayUtc(nowSec)))) {
    return jsonResponse({ error: "rate limited" }, 429);
  }

  const id = crypto.randomUUID();
  const day = dayUtc(nowSec);
  const origin = new URL(request.url).origin;

  let bundleUrl: string | null = null;
  if (parsed.bundle) {
    const key = `${R2_PREFIX}${day}/${id}.zip`;
    await env.FEEDBACK.put(key, await parsed.bundle.arrayBuffer(), {
      httpMetadata: { contentType: "application/zip" },
    });
    bundleUrl = assetUrl(origin, key);
  }

  const imageUrls: string[] = [];
  for (const [index, shot] of parsed.screenshots.entries()) {
    const key = `${R2_PREFIX}${day}/${id}-shot-${index + 1}.${extensionForType(shot.type)}`;
    await env.FEEDBACK.put(key, await shot.arrayBuffer(), {
      httpMetadata: { contentType: shot.type },
    });
    imageUrls.push(assetUrl(origin, key));
  }

  const issue = await createIssue(
    env,
    buildIssueTitle(parsed.description),
    buildIssueBody(parsed, imageUrls, bundleUrl),
  );
  if (!issue.ok) {
    // R2 对象已写（清理任务按保留期兜底回收）；给客户端一个可重试的失败。
    console.error(`feedback issue creation failed: ${issue.error}`);
    return jsonResponse({ error: "issue creation failed" }, 502);
  }

  return jsonResponse({ accepted: true, issueNumber: issue.number }, 202);
}

/** GET /v1/feedback-asset/<day>/<file>：key 不可猜（uuid 段），与 GitHub 私有仓附件同款模型。 */
export async function handleFeedbackAsset(
  request: Request,
  env: Pick<FeedbackEnv, "FEEDBACK">,
  assetPath: string,
): Promise<Response> {
  // 归一：只允许单段内的安全字符，防 `..` 与拼 key 穿越。
  if (!/^[0-9a-zA-Z][0-9a-zA-Z._-]*$/.test(assetPath)) {
    return jsonResponse({ error: "bad asset path" }, 400);
  }
  const object = await env.FEEDBACK.get(`${R2_PREFIX}${assetPath}`);
  if (!object) {
    return jsonResponse({ error: "not found" }, 404);
  }
  const headers = new Headers({
    "content-type": object.httpMetadata?.contentType ?? "application/octet-stream",
    "cache-control": "private, max-age=3600",
  });
  if ((object.httpMetadata?.contentType ?? "").startsWith("image/")) {
    headers.set("content-disposition", "inline");
  } else {
    headers.set("content-disposition", "attachment");
  }
  return new Response(object.body, { status: 200, headers });
}

/** scheduled 用：删除超过保留期的附件（issue 不删，正文里的旧链接随之失效）。 */
export async function cleanupOldFeedback(
  env: Pick<FeedbackEnv, "FEEDBACK">,
  nowSec: number,
): Promise<number> {
  const cutoff = new Date((nowSec - FEEDBACK_RETENTION_SECS) * 1000);
  let deleted = 0;
  let cursor: string | undefined;
  do {
    const listing = await env.FEEDBACK.list({ prefix: R2_PREFIX, cursor });
    const doomed = listing.objects
      .filter((object) => object.uploaded < cutoff)
      .map((object) => object.key);
    if (doomed.length > 0) {
      await Promise.all(doomed.map((key) => env.FEEDBACK.delete(key)));
      deleted += doomed.length;
    }
    cursor = listing.truncated ? listing.cursor : undefined;
  } while (cursor);
  return deleted;
}
