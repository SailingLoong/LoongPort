import { describe, expect, it, vi } from "vitest";

import {
  FEEDBACK_RETENTION_SECS,
  FEEDBACK_UPLOAD_LEASE_SECS,
  FEEDBACK_RELATED_APP_IDS,
  handleFeedbackAsset,
  parseFeedbackForm,
} from "./feedback";

function legacyForm(): FormData {
  const form = new FormData();
  form.set("sourceId", "0123456789abcdef0123456789abcdef");
  form.set("appVersion", "6.25.0");
  form.set("description", "Synthetic feedback only");
  form.set("meta", JSON.stringify({ os: "linux" }));
  return form;
}

describe("feedback retention safety contract", () => {
  it("retains feedback objects for 30 days rather than 90", () => {
    expect(FEEDBACK_RETENTION_SECS).toBe(30 * 86400);
  });

  it("keeps the actual legacy multipart shape parseable during rollout", () => {
    expect(parseFeedbackForm(legacyForm()).ok).toBe(true);
  });

  it("does not silently fall back to legacy identity for a malformed new identity", () => {
    const form = legacyForm();
    form.set("installId", "not-an-install-uuid");
    form.set("reportId", "4a267031-4dd2-48ed-822e-aa0bddf681e4");
    expect(parseFeedbackForm(form).ok).toBe(false);
  });

  it("does not silently ignore a malformed new report ID", () => {
    const form = legacyForm();
    form.set("installId", "4a267031-4dd2-48ed-822e-aa0bddf681e4");
    form.set("reportId", "../../someone-else");
    expect(parseFeedbackForm(form).ok).toBe(false);
  });

  it("never reads or returns an object to an anonymous asset request", async () => {
    const getWithMetadata = vi.fn().mockResolvedValue({
      value: new TextEncoder().encode("SYNTHETIC_PRIVATE_DIAGNOSTICS"),
      metadata: { ct: "application/zip" },
    });
    const response = await handleFeedbackAsset(
      new Request(
        "https://metrics.example/v1/feedback-asset/2026-10-18/public-report-id.zip",
      ),
      { FEEDBACK: { getWithMetadata } as unknown as KVNamespace },
      "2026-10-18/public-report-id.zip",
    );
    expect([401, 403, 404]).toContain(response.status);
    expect(getWithMetadata).not.toHaveBeenCalled();
    expect(await response.text()).not.toContain(
      "SYNTHETIC_PRIVATE_DIAGNOSTICS",
    );
  });
});

import { readFileSync } from "node:fs";
import { URL as NodeURL } from "node:url";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach } from "vitest";
import {
  handleFeedback,
  buildIssueBody,
  legacyFeedbackAllowed,
  maintainFeedback,
  MAX_LEGACY_REPORTS,
  RESERVE_REPORT_SQL,
  type FeedbackEnv,
  type FeedbackReportRow,
} from "./feedback";

const INSTALL = "4a267031-4dd2-48ed-822e-aa0bddf681e4";
const NOW = Math.floor(Date.parse("2026-10-18T10:00:00Z") / 1000);
const id = (index: number): string =>
  `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;

type SqlFault = { matches: string; afterCommit: boolean };

class SyntheticD1 {
  readonly sqlite = new DatabaseSync(":memory:");
  fault: SqlFault | null = null;
  followupFault: SqlFault | null = null;
  constructor() {
    this.sqlite.exec(
      readFileSync(new NodeURL("../schema.sql", import.meta.url), "utf8"),
    );
  }
  prepare(sql: string): D1PreparedStatement {
    let values: unknown[] = [];
    const execute = (kind: "first" | "run" | "all") => {
      const fault =
        this.fault?.matches && sql.includes(this.fault.matches)
          ? this.fault
          : null;
      if (fault) {
        this.fault = this.followupFault;
        this.followupFault = null;
      }
      if (fault && !fault.afterCommit)
        throw new Error("synthetic D1 transport failure");
      const statement = this.sqlite.prepare(sql);
      const args = values as (string | number | null)[];
      const result =
        kind === "first"
          ? (statement.get(...args) ?? null)
          : kind === "all"
            ? { results: statement.all(...args), success: true }
            : { meta: statement.run(...args), success: true };
      if (fault?.afterCommit) {
        throw new Error("synthetic D1 response lost after commit");
      }
      return result;
    };
    const prepared = {
      bind: (...args: unknown[]) => {
        values = args;
        return prepared;
      },
      first: async () => execute("first"),
      all: async () => execute("all"),
      run: async () => execute("run"),
    };
    return prepared as unknown as D1PreparedStatement;
  }
  rows(): FeedbackReportRow[] {
    return this.sqlite
      .prepare("SELECT * FROM feedback_reports ORDER BY report_id")
      .all() as unknown as FeedbackReportRow[];
  }
}

class SyntheticKV {
  readonly objects = new Map<string, ArrayBuffer>();
  readonly expirations = new Map<string, number>();
  putCount = 0;
  failPutAt = 0;
  failDelete = false;
  onDelete: ((key: string) => Promise<void>) | null = null;
  onPut: ((key: string) => Promise<void>) | null = null;
  async put(
    key: string,
    value: ArrayBuffer,
    options: { expiration: number },
  ): Promise<void> {
    this.putCount++;
    if (this.onPut) await this.onPut(key);
    this.objects.set(key, value);
    this.expirations.set(key, options.expiration);
    if (this.putCount === this.failPutAt)
      throw new Error("synthetic KV response lost after write");
  }
  async delete(key: string): Promise<void> {
    if (this.onDelete) await this.onDelete(key);
    if (this.failDelete) throw new Error("synthetic KV delete unavailable");
    this.objects.delete(key);
  }
}

function modernForm(index = 1, installId = INSTALL): FormData {
  const form = legacyForm();
  form.set("installId", installId);
  form.set("reportId", id(index));
  form.set(
    "bundle",
    new File(["synthetic zip contents"], "diagnostics.zip", {
      type: "application/zip",
    }),
  );
  return form;
}

function request(form: FormData, ip = "192.0.2.1"): Request {
  return new Request("https://metrics.example/v1/feedback", {
    method: "POST",
    body: form,
    headers: { "cf-connecting-ip": ip },
  });
}

let database: SyntheticD1;
let kv: SyntheticKV;
let env: FeedbackEnv;
let remote: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW * 1000);
  database = new SyntheticD1();
  kv = new SyntheticKV();
  env = {
    DB: database as unknown as D1Database,
    FEEDBACK: kv as unknown as KVNamespace,
    GH_FEEDBACK_TOKEN: "SYNTHETIC_TEST_VALUE_NOT_A_CREDENTIAL",
  };
  let issue = 100;
  remote = vi.fn(async (_url: unknown, options?: RequestInit) => {
    if (options?.method === "POST")
      return Response.json({ number: ++issue }, { status: 201 });
    if (options?.method === "PATCH") return Response.json({ state: "closed" });
    return Response.json([]);
  });
  vi.stubGlobal("fetch", remote);
});

afterEach(() => {
  database.sqlite.close();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("single-manifest feedback lifecycle with real SQLite", () => {
  it("confirms durable storage and returns a unique report receipt", async () => {
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(202);
    expect(await response.json()).toMatchObject({
      accepted: true,
      reportId: id(1),
      issueNumber: 101,
      uploadedAt: "2026-10-18T10:00:00.000Z",
    });
    expect(database.rows()[0]).toMatchObject({
      state: "stored",
      identity_kind: "install",
    });
    expect(kv.objects.size).toBe(2);
    expect([...kv.expirations.values()]).toEqual([
      NOW + FEEDBACK_RETENTION_SECS,
      NOW + FEEDBACK_RETENTION_SECS,
    ]);
  });

  it("uses the actual atomic SQL for concurrent admission and never evicts an older report", async () => {
    const attempts = await Promise.all(
      Array.from({ length: 12 }, (_, i) =>
        database
          .prepare(RESERVE_REPORT_SQL)
          .bind(
            id(i),
            "install",
            `install:${INSTALL}`,
            "synthetic",
            NOW,
            NOW + FEEDBACK_RETENTION_SECS,
            "[]",
            3,
            MAX_LEGACY_REPORTS,
            `synthetic-generation-${i}`,
            NOW + FEEDBACK_UPLOAD_LEASE_SECS,
          )
          .first(),
      ),
    );
    expect(attempts.filter(Boolean)).toHaveLength(3);
    expect(database.rows().map((row) => row.report_id)).toEqual([
      id(0),
      id(1),
      id(2),
    ]);
    const rejected = await handleFeedback(request(modernForm(99)), env);
    expect(rejected.status).toBe(429);
    expect(database.rows().map((row) => row.report_id)).toEqual([
      id(0),
      id(1),
      id(2),
    ]);
    expect(kv.putCount).toBe(0);
    expect(remote).not.toHaveBeenCalled();
  });

  it("replays a same-content receipt without more object writes or notifications", async () => {
    await handleFeedback(request(modernForm()), env);
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(202);
    expect(database.rows()).toHaveLength(1);
    expect(kv.putCount).toBe(2);
    expect(remote).toHaveBeenCalledTimes(1);
  });

  it("rejects report-ID collisions from another install or changed contents", async () => {
    await handleFeedback(request(modernForm()), env);
    expect(
      (await handleFeedback(request(modernForm(1, id(77))), env)).status,
    ).toBe(409);
    const changed = modernForm();
    changed.set("description", "changed synthetic description");
    expect((await handleFeedback(request(changed), env)).status).toBe(409);
    expect(kv.putCount).toBe(2);
    expect(database.rows()).toHaveLength(1);
  });

  it("does not send description, install ID, diagnostics or attachment links to GitHub", async () => {
    const form = modernForm();
    form.set("description", "SYNTHETIC_PRIVATE_DESCRIPTION");
    form.set(
      "meta",
      JSON.stringify({
        appVersion: "6.25.0",
        diagnostics: { schemaVersion: 1, marker: "SYNTHETIC_PRIVATE_META" },
      }),
    );
    form.set(
      "signals",
      JSON.stringify({ recentErrors: ["SYNTHETIC_PRIVATE_SIGNAL"] }),
    );
    await handleFeedback(request(form), env);
    const notification = String(remote.mock.calls[0][1].body);
    expect(notification).toContain(id(1));
    for (const forbidden of [
      INSTALL,
      "SYNTHETIC_PRIVATE",
      "https://",
      "feedback-asset",
    ]) {
      expect(notification).not.toContain(forbidden);
    }
  });

  it("keeps the old wire shape on the same manifest and bounds its global capacity", async () => {
    for (let i = 0; i < MAX_LEGACY_REPORTS; i++) {
      const form = legacyForm();
      form.set("sourceId", i.toString(16).padStart(32, "0"));
      expect(
        (await handleFeedback(request(form, `192.0.2.${i + 1}`), env)).status,
      ).toBe(202);
    }
    const next = legacyForm();
    next.set("sourceId", "f".repeat(32));
    expect(
      (await handleFeedback(request(next, "198.51.100.1"), env)).status,
    ).toBe(429);
    expect(database.rows()).toHaveLength(MAX_LEGACY_REPORTS);
    expect(database.rows().every((row) => row.identity_kind === "legacy")).toBe(
      true,
    );
  });

  it("makes the optional migration deadline explicit without interpreting old source IDs as installations", () => {
    expect(legacyFeedbackAllowed(env, NOW)).toBe(true);
    expect(
      legacyFeedbackAllowed(
        { ...env, FEEDBACK_LEGACY_UNTIL: "2026-10-19T00:00:00Z" },
        NOW,
      ),
    ).toBe(true);
    expect(
      legacyFeedbackAllowed(
        { ...env, FEEDBACK_LEGACY_UNTIL: "2026-10-18T10:00:00Z" },
        NOW,
      ),
    ).toBe(false);
    expect(
      legacyFeedbackAllowed({ ...env, FEEDBACK_LEGACY_UNTIL: "invalid" }, NOW),
    ).toBe(false);
  });

  it("does not create an issue or claim success after a partial KV write failure", async () => {
    kv.failPutAt = 2;
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(503);
    expect(await response.json()).not.toMatchObject({ accepted: true });
    expect(kv.objects.size).toBe(0);
    expect(database.rows()[0]).toMatchObject({
      state: "cleanup",
      put_pending: 1,
    });
    expect(remote).not.toHaveBeenCalled();
  });

  it("retains a cleanup manifest and capacity until failed object deletes recover", async () => {
    kv.failPutAt = 2;
    kv.failDelete = true;
    expect((await handleFeedback(request(modernForm()), env)).status).toBe(503);
    expect(database.rows()[0]).toMatchObject({
      state: "cleanup",
      identity_key: `install:${INSTALL}`,
    });
    kv.failDelete = false;
    await maintainFeedback(env, NOW);
    expect(kv.objects.size).toBe(0);
    expect(database.rows()[0]).toMatchObject({
      state: "cleanup",
      put_pending: 1,
    });
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(database.rows()).toHaveLength(0);
  });

  it("handles a committed stored-state update with a lost D1 response as success", async () => {
    database.fault = { matches: "SET state = 'stored'", afterCommit: true };
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(202);
    expect(database.rows()[0].state).toBe("stored");
    expect(kv.objects.size).toBe(2);
  });

  it("recovers an uncertain reservation only when this request's generation proves ownership", async () => {
    database.fault = {
      matches: "INSERT INTO feedback_reports",
      afterCommit: true,
    };
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(202);
    expect(database.rows()[0].state).toBe("stored");
    expect(kv.putCount).toBe(2);
  });

  it("does not write objects when an uncertain reservation cannot be read back", async () => {
    database.fault = {
      matches: "INSERT INTO feedback_reports",
      afterCommit: true,
    };
    database.followupFault = {
      matches: "SELECT * FROM feedback_reports",
      afterCommit: false,
    };
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(503);
    expect(database.rows()[0].state).toBe("uploading");
    expect(kv.putCount).toBe(0);
    expect(remote).not.toHaveBeenCalled();
  });

  it("keeps stored content accepted when GitHub rejects a notification", async () => {
    remote.mockResolvedValue(
      new Response("synthetic forbidden", { status: 403 }),
    );
    const response = await handleFeedback(request(modernForm()), env);
    expect(response.status).toBe(202);
    expect(await response.json()).toMatchObject({
      accepted: true,
      issueNumber: null,
    });
    expect(database.rows()[0]).toMatchObject({
      state: "stored",
      issue_state: "pending",
    });
  });

  it("reconciles an uncertain GitHub POST without ever issuing a second POST", async () => {
    remote.mockRejectedValueOnce(new Error("synthetic response lost"));
    await handleFeedback(request(modernForm()), env);
    expect(database.rows()[0].issue_state).toBe("unknown");
    remote.mockResolvedValue(
      Response.json([{ number: 321, body: buildIssueBody(id(1), NOW) }]),
    );
    await maintainFeedback(env, NOW);
    expect(database.rows()[0]).toMatchObject({
      issue_state: "created",
      issue_number: 321,
    });
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "POST"),
    ).toHaveLength(1);
  });

  it("removes objects and manifest only after the 30-day boundary and closes the notification", async () => {
    await handleFeedback(request(modernForm()), env);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS - 1);
    expect(kv.objects.size).toBe(2);
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(kv.objects.size).toBe(0);
    expect(database.rows()).toHaveLength(0);
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "PATCH"),
    ).toHaveLength(1);
  });

  it("fences stale cleanup from a replacement generation with the same public report ID", async () => {
    await handleFeedback(request(modernForm()), env);
    const original = database.rows()[0];
    const oldKeys = JSON.parse(original.asset_keys) as string[];
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    let replacementGeneration = "";
    kv.onDelete = async () => {
      kv.onDelete = null;
      // Another cleanup already finished, then the client reused its public ID.
      database.sqlite
        .prepare("DELETE FROM feedback_reports WHERE report_id = ?")
        .run(id(1));
      oldKeys.forEach((key) => kv.objects.delete(key));
      const replacement = await handleFeedback(request(modernForm()), env);
      expect(replacement.status).toBe(202);
      replacementGeneration = database.rows()[0].generation;
    };
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(replacementGeneration).not.toBe(original.generation);
    expect(database.rows()[0]).toMatchObject({
      generation: replacementGeneration,
      state: "stored",
      issue_number: 102,
    });
    expect(kv.objects.size).toBe(2);
    expect(
      [...kv.objects.keys()].every((key) =>
        key.includes(replacementGeneration),
      ),
    ).toBe(true);
    expect(
      remote.mock.calls.some(
        (call) =>
          call[1]?.method === "PATCH" && String(call[0]).endsWith("/102"),
      ),
    ).toBe(false);
  });

  it("does not retry a potentially committed 408 notification", async () => {
    remote.mockResolvedValueOnce(
      new Response("synthetic timeout", { status: 408 }),
    );
    await handleFeedback(request(modernForm()), env);
    expect(database.rows()[0].issue_state).toBe("unknown");
    await maintainFeedback(env, NOW);
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "POST"),
    ).toHaveLength(1);
  });

  it("ends uncertain notification reconciliation at the original retention deadline", async () => {
    remote.mockRejectedValueOnce(new Error("synthetic response lost"));
    await handleFeedback(request(modernForm()), env);
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(kv.objects.size).toBe(0);
    expect(database.rows()).toHaveLength(0);
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "POST"),
    ).toHaveLength(1);
  });

  it("does not keep an unbounded tombstone when closing a content-free notification fails", async () => {
    await handleFeedback(request(modernForm()), env);
    remote.mockResolvedValueOnce(
      new Response("synthetic forbidden", { status: 403 }),
    );
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(database.rows()).toHaveLength(0);
    expect(kv.objects.size).toBe(0);
  });

  it("uses the original absolute KV expiration as the terminal fallback after failed deletes", async () => {
    await handleFeedback(request(modernForm()), env);
    kv.failDelete = true;
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(database.rows()).toHaveLength(0);
    expect(
      [...kv.expirations.values()].every(
        (expiration) => expiration <= Date.now() / 1000,
      ),
    ).toBe(true);
  });
});

const UPLOAD_LEASE_SECS = FEEDBACK_UPLOAD_LEASE_SECS;

async function strandReservation(index: number): Promise<void> {
  database.fault = {
    matches: "INSERT INTO feedback_reports",
    afterCommit: true,
  };
  database.followupFault = {
    matches: "SELECT * FROM feedback_reports",
    afterCommit: false,
  };
  expect((await handleFeedback(request(modernForm(index)), env)).status).toBe(
    503,
  );
}

describe("bounded upload lease recovery", () => {
  it("reclaims a committed reservation whose response and recovery read were both lost", async () => {
    await strandReservation(1);
    expect(kv.putCount).toBe(0);
    vi.setSystemTime((NOW + UPLOAD_LEASE_SECS) * 1000);
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS);
    expect(database.rows()).toHaveLength(0);
    expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
      202,
    );
  });

  it("reclaims a writer interrupted after KV storage and before the stored transition", async () => {
    database.fault = { matches: "SET state = 'stored'", afterCommit: false };
    database.followupFault = {
      matches: "SELECT * FROM feedback_reports",
      afterCommit: false,
    };
    expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
      503,
    );
    expect(database.rows()[0].state).toBe("uploading");
    expect(kv.objects.size).toBe(2);
    vi.setSystemTime((NOW + UPLOAD_LEASE_SECS) * 1000);
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS);
    expect(kv.objects.size).toBe(0);
    expect(database.rows()).toHaveLength(0);
    expect(remote).not.toHaveBeenCalled();
  });

  it("allows another submission after three interrupted reservations reach their finite lease", async () => {
    for (let i = 1; i <= 3; i++) await strandReservation(i);
    expect((await handleFeedback(request(modernForm(4)), env)).status).toBe(
      429,
    );
    vi.setSystemTime((NOW + UPLOAD_LEASE_SECS) * 1000);
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS);
    expect((await handleFeedback(request(modernForm(4)), env)).status).toBe(
      202,
    );
    expect(
      database.rows().filter((row) => row.state === "stored"),
    ).toHaveLength(1);
  });

  it("never allows a same-ID retry to take over a live generation", async () => {
    await strandReservation(1);
    const original = database.rows()[0];
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS - 1);
    expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
      409,
    );
    expect(database.rows()[0].generation).toBe(original.generation);
    expect(kv.putCount).toBe(0);
  });

  it("tracks a pending put past lease expiry without letting it revive the report", async () => {
    let release!: () => void;
    let entered!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    kv.onPut = async () => {
      kv.onPut = null;
      entered();
      await gate;
    };
    const upload = handleFeedback(request(modernForm(1)), env);
    await started;
    const original = database.rows()[0];
    expect(original.put_pending).toBe(1);
    let response: Response | undefined;
    try {
      vi.setSystemTime((NOW + UPLOAD_LEASE_SECS) * 1000);
      await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS);
      expect(database.rows()[0]).toMatchObject({
        state: "retired",
        put_pending: 1,
      });
      expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
        409,
      );
      expect((await handleFeedback(request(modernForm(2)), env)).status).toBe(
        202,
      );
    } finally {
      release();
      response = await upload;
    }
    expect(response.status).toBe(503);
    expect(database.rows().map((row) => row.report_id)).toEqual([id(2)]);
    expect(kv.objects.size).toBe(2);
    expect(
      [...kv.objects.keys()].every((key) => !key.includes(original.generation)),
    ).toBe(true);
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "POST"),
    ).toHaveLength(1);
    // Completion/cleanup of the retired generation finally permits a fresh generation of the ID.
    expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
      202,
    );
    expect(
      database.rows().find((row) => row.report_id === id(1))?.generation,
    ).not.toBe(original.generation);
  });

  it("fences a put completing after its absolute expiry from a replacement generation", async () => {
    let release!: () => void;
    let entered!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    kv.onPut = async () => {
      kv.onPut = null;
      entered();
      await gate;
    };
    const upload = handleFeedback(request(modernForm(1)), env);
    await started;
    const original = database.rows()[0];
    let response: Response | undefined;
    let replacement: FeedbackReportRow | undefined;
    try {
      vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
      await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
      expect(database.rows()).toHaveLength(0);
      expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
        202,
      );
      replacement = database.rows()[0];
    } finally {
      release();
      response = await upload;
    }
    expect(response.status).toBe(503);
    expect(database.rows()).toHaveLength(1);
    expect(database.rows()[0]).toMatchObject({
      generation: replacement!.generation,
      state: "stored",
      put_pending: 0,
    });
    expect(replacement!.generation).not.toBe(original.generation);
    // A provider may complete the old operation late, but its original absolute
    // expiration makes that old value unreadable; it cannot extend its lifetime.
    const liveKeys = [...kv.objects.keys()].filter(
      (key) => kv.expirations.get(key)! > Date.now() / 1000,
    );
    expect(liveKeys).toHaveLength(2);
    expect(liveKeys.every((key) => key.includes(replacement!.generation))).toBe(
      true,
    );
    expect(
      [...kv.expirations]
        .filter(([key]) => key.includes(original.generation))
        .every(([, expiry]) => expiry === NOW + FEEDBACK_RETENTION_SECS),
    ).toBe(true);
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "POST"),
    ).toHaveLength(1);
  });

  it("rotates unresolved cleanup fairly and retains no identifiers beyond the original expiry", async () => {
    for (let i = 1; i <= 3; i++) {
      kv.failPutAt = kv.putCount + 1;
      expect((await handleFeedback(request(modernForm(i)), env)).status).toBe(
        503,
      );
    }
    const oldGenerations = database.rows().map((row) => row.generation);
    const deleted = new Set<string>();
    kv.onDelete = async (key) => {
      deleted.add(key);
    };
    kv.failDelete = true;
    vi.setSystemTime((NOW + UPLOAD_LEASE_SECS) * 1000);
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS);
    expect(database.rows().every((row) => row.state === "retired")).toBe(true);
    for (let i = 4; i <= 6; i++) {
      expect(
        (await handleFeedback(request(modernForm(i), `198.51.100.${i}`), env))
          .status,
      ).toBe(202);
    }
    expect(
      (await handleFeedback(request(modernForm(7), "198.51.100.7"), env))
        .status,
    ).toBe(429);
    expect(
      database.rows().filter((row) => row.state === "stored"),
    ).toHaveLength(3);
    await maintainFeedback(env, NOW + UPLOAD_LEASE_SECS + 600);
    expect(
      oldGenerations.every((generation) =>
        [...deleted].some((key) => key.includes(generation)),
      ),
    ).toBe(true);
    // A retry cannot grow another row or restart the original deadline.
    expect((await handleFeedback(request(modernForm(1)), env)).status).toBe(
      409,
    );
    expect(database.rows()).toHaveLength(6);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(
      database
        .rows()
        .every((row) => row.expires_at > NOW + FEEDBACK_RETENTION_SECS),
    ).toBe(true);
    expect(database.rows()).toHaveLength(3);
  });

  it("purges every expired manifest even when external cleanup exceeds its two-row budget", async () => {
    for (let i = 1; i <= 5; i++) {
      expect(
        (
          await handleFeedback(
            request(modernForm(i, id(i + 100)), `198.51.100.${i}`),
            env,
          )
        ).status,
      ).toBe(202);
    }
    kv.failDelete = true;
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(database.rows()).toHaveLength(0);
    expect(
      [...kv.expirations.values()].every(
        (expiry) => expiry <= Date.now() / 1000,
      ),
    ).toBe(true);
    // Closing content-free notifications is best effort and cannot retain a D1 tombstone.
    expect(
      remote.mock.calls.filter((call) => call[1]?.method === "PATCH"),
    ).toHaveLength(2);
  });

  it("does not expose the internal expiration policy in a successful receipt", async () => {
    const response = await handleFeedback(request(modernForm(1)), env);
    expect(response.status).toBe(202);
    expect(await response.json()).not.toHaveProperty("expiresAt");
  });
});

describe("private feedback form extensions", () => {
  it("keeps the server app allowlist in the desktop registry order plus other", () => {
    const source = readFileSync(
      new NodeURL("../../src/config/appConfig.tsx", import.meta.url),
      "utf8",
    );
    const body = source.match(
      /export const APP_IDS: AppId\[\] = \[([\s\S]*?)\];/,
    )?.[1];
    expect(body).toBeDefined();
    const appIds = [...body!.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
    expect(appIds).toHaveLength(11);
    expect(FEEDBACK_RELATED_APP_IDS).toEqual([...appIds, "other"]);
  });
  it("stores normalized related apps and contact only in the same expiring report object", async () => {
    const form = modernForm(1);
    form.set(
      "relatedApps",
      JSON.stringify(["other", "zcode", "claude", "claude"]),
    );
    form.set("contact", "  synthetic-contact@example.invalid  ");
    const response = await handleFeedback(request(form), env);
    expect(response.status).toBe(202);
    const reportKey = [...kv.objects.keys()].find((key) =>
      key.endsWith(".json"),
    )!;
    expect(
      JSON.parse(new TextDecoder().decode(kv.objects.get(reportKey))),
    ).toMatchObject({
      relatedApps: ["claude", "zcode", "other"],
      contact: "synthetic-contact@example.invalid",
    });
    expect(kv.expirations.get(reportKey)).toBe(NOW + FEEDBACK_RETENTION_SECS);
    const receipt = JSON.stringify(await response.json());
    expect(receipt).not.toContain("synthetic-contact");
    expect(receipt).not.toContain("relatedApps");
    expect(JSON.stringify(remote.mock.calls)).not.toContain(
      "synthetic-contact",
    );
    expect(JSON.stringify(remote.mock.calls)).not.toContain("zcode");
    vi.setSystemTime((NOW + FEEDBACK_RETENTION_SECS) * 1000);
    await maintainFeedback(env, NOW + FEEDBACK_RETENTION_SECS);
    expect(kv.objects.size).toBe(0);
    expect(database.rows()).toHaveLength(0);
  });

  it.each([
    ["contact", "line one\nline two"],
    ["contact", "\ttrim must not hide controls"],
    ["contact", "\u0085contact"],
    ["contact", "😀".repeat(201)],
    ["contact", "\ufeff" + "x".repeat(200)],
    ["relatedApps", "not json"],
    ["relatedApps", "null"],
    ["relatedApps", "{}"],
    ["relatedApps", '["unknown-app"]'],
    ["relatedApps", '["claude", 1]'],
  ])(
    "rejects invalid %s without any object or issue write",
    async (field, value) => {
      const form = modernForm(1);
      form.set(field, value);
      const response = await handleFeedback(request(form), env);
      expect(response.status).toBe(400);
      expect(kv.putCount).toBe(0);
      expect(database.rows()).toHaveLength(0);
      expect(remote).not.toHaveBeenCalled();
    },
  );

  it.each(["contact", "relatedApps"])(
    "rejects duplicate or file-valued %s fields",
    async (field) => {
      const duplicated = modernForm(1);
      duplicated.append(field, field === "contact" ? "synthetic" : "[]");
      duplicated.append(field, field === "contact" ? "synthetic" : "[]");
      expect((await handleFeedback(request(duplicated), env)).status).toBe(400);
      const file = modernForm(1);
      file.set(field, new File(["synthetic"], "synthetic.txt"));
      expect((await handleFeedback(request(file), env)).status).toBe(400);
      expect(kv.putCount).toBe(0);
      expect(remote).not.toHaveBeenCalled();
    },
  );

  it("counts Unicode code points and accepts exactly 200 non-BMP contact characters", async () => {
    const form = modernForm(1);
    form.set("relatedApps", "[]");
    form.set("contact", "😀".repeat(200));
    expect((await handleFeedback(request(form), env)).status).toBe(202);
    const key = [...kv.objects.keys()].find((candidate) =>
      candidate.endsWith(".json"),
    )!;
    expect(
      JSON.parse(new TextDecoder().decode(kv.objects.get(key))),
    ).toMatchObject({
      relatedApps: [],
      contact: "😀".repeat(200),
    });
  });

  it("omits an empty normalized contact and leaves an old multipart shape compatible", async () => {
    const form = legacyForm();
    form.set("contact", " 　");
    expect((await handleFeedback(request(form), env)).status).toBe(202);
    const key = [...kv.objects.keys()].find((candidate) =>
      candidate.endsWith(".json"),
    )!;
    const report = JSON.parse(new TextDecoder().decode(kv.objects.get(key)));
    expect(report).not.toHaveProperty("contact");
    expect(report).not.toHaveProperty("relatedApps");
  });

  it("includes normalized private form extensions in content-hash conflict checks", async () => {
    const form = modernForm(1);
    form.set("relatedApps", '["zcode", "claude"]');
    form.set("contact", "  synthetic-contact@example.invalid ");
    expect((await handleFeedback(request(form), env)).status).toBe(202);
    form.set("relatedApps", '["claude", "zcode", "claude"]');
    form.set("contact", "synthetic-contact@example.invalid");
    expect((await handleFeedback(request(form), env)).status).toBe(202);
    form.set("contact", "different-synthetic-contact@example.invalid");
    expect((await handleFeedback(request(form), env)).status).toBe(409);
    form.set("contact", "synthetic-contact@example.invalid");
    form.set("relatedApps", '["other"]');
    expect((await handleFeedback(request(form), env)).status).toBe(409);
    expect(kv.putCount).toBe(2);
    expect(remote).toHaveBeenCalledTimes(1);
  });
});

describe("raw contact allocation bound", () => {
  it.each([
    ["long whitespace", " ".repeat(4096)],
    ["long BMP", "界".repeat(4096)],
    ["long non-BMP", "😀".repeat(2048)],
    ["1025 whitespace units", " ".repeat(1025)],
    [
      "1025 units around a valid BMP contact",
      " ".repeat(825) + "x".repeat(200),
    ],
    [
      "1025 units around a valid non-BMP contact",
      " ".repeat(625) + "😀".repeat(200),
    ],
  ])(
    "rejects %s before any contact scanning/allocation",
    async (_label, raw) => {
      const form = modernForm(1);
      form.set("contact", raw);
      const scan = vi.spyOn(RegExp.prototype, "test");
      const trim = vi.spyOn(String.prototype, "replace");
      const iterate = vi.spyOn(String.prototype, Symbol.iterator);
      try {
        expect(parseFeedbackForm(form)).toEqual({
          ok: false,
          error: "bad contact",
        });
        expect(scan.mock.calls.some(([value]) => value === raw)).toBe(false);
        expect(
          trim.mock.contexts.some((context) => String(context) === raw),
        ).toBe(false);
        expect(
          iterate.mock.contexts.some((context) => String(context) === raw),
        ).toBe(false);
      } finally {
        scan.mockRestore();
        trim.mockRestore();
        iterate.mockRestore();
      }
      expect((await handleFeedback(request(form), env)).status).toBe(400);
      expect(kv.putCount).toBe(0);
      expect(database.rows()).toHaveLength(0);
      expect(remote).not.toHaveBeenCalled();
    },
  );

  it.each([
    ["exact 1024 whitespace", " ".repeat(1024), undefined],
    [
      "exact 1024 UTF-16 units with BMP",
      " ".repeat(824) + "x".repeat(200),
      "x".repeat(200),
    ],
    [
      "exact 1024 UTF-16 units with non-BMP",
      " ".repeat(624) + "😀".repeat(200),
      "😀".repeat(200),
    ],
  ])(
    "accepts %s while preserving the 200-code-point contract",
    (_label, raw, expected) => {
      const form = modernForm(1);
      form.set("contact", raw!);
      const parsed = parseFeedbackForm(form);
      expect(parsed.ok).toBe(true);
      if (parsed.ok) expect(parsed.contact).toBe(expected);
    },
  );
});

import worker from "./index";

describe("feedback route boundary", () => {
  it("exposes neither client delete operations nor a token-based reader", async () => {
    const context = { waitUntil: vi.fn() } as unknown as ExecutionContext;
    for (const [method, path] of [
      ["DELETE", `/v1/feedback/${id(1)}`],
      ["POST", `/v1/feedback/${id(1)}/delete`],
      ["GET", `/v1/feedback-asset/2026-10-18/${id(1)}.zip`],
    ]) {
      const response = await worker.fetch(
        new Request(`https://metrics.example${path}`, {
          method,
          headers: { authorization: "Bearer SYNTHETIC_NOT_A_CREDENTIAL" },
        }),
        env as Parameters<typeof worker.fetch>[1],
        context,
      );
      expect(response.status).toBe(404);
    }
    expect(remote).not.toHaveBeenCalled();
    expect(database.rows()).toHaveLength(0);
  });
});
