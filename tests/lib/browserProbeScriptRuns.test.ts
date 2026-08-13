import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import vm from "node:vm";
import { describe, expect, it } from "vitest";

const REPO = resolve(__dirname, "../..");
const SCRIPT = resolve(REPO, "tests/fixtures/browser-probe-script.txt");

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

async function flushPromises(): Promise<void> {
  await new Promise<void>((resolveNow) => setImmediate(resolveNow));
}

describe("browser probe generated script", () => {
  it("waits for the initial document to finish loading before probing", async () => {
    const documentListeners = new Map<string, () => void>();
    const intervalCallbacks: Array<() => void> = [];
    const fetchedPaths: string[] = [];
    const navigations: string[] = [];
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };

    const sandbox = {
      window: { location },
      document: {
        readyState: "loading",
        addEventListener: (event: string, listener: () => void) => {
          documentListeners.set(event, listener);
        },
      },
      fetch: (path: string) => {
        fetchedPaths.push(path);
        return Promise.resolve({
          text: () => Promise.resolve('{"success":true}'),
        });
      },
      setInterval: (callback: () => void) => {
        intervalCallbacks.push(callback);
        return 1;
      },
      setTimeout,
      clearTimeout,
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });
    await flushPromises();

    expect(fetchedPaths).toHaveLength(0);
    expect(navigations).toHaveLength(0);
    expect(intervalCallbacks).toHaveLength(0);

    const onDocumentReady = documentListeners.get("DOMContentLoaded");
    expect(onDocumentReady).toBeDefined();
    onDocumentReady!();
    await flushPromises();

    expect(fetchedPaths).toEqual(["/api/v1/settings/public", "/api/status"]);
    expect(navigations).toHaveLength(1);
    expect(intervalCallbacks).toHaveLength(1);
  });

  it("serializes delayed rounds and emits one complete callback", async () => {
    const intervalCallbacks: Array<() => void> = [];
    const responseBodies: Array<Deferred<string>> = [];
    const fetchedPaths: string[] = [];
    const navigations: string[] = [];
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/login";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };

    const sandbox = {
      window: { location },
      document: { readyState: "complete" },
      fetch: (path: string) => {
        fetchedPaths.push(path);
        const body = deferred<string>();
        responseBodies.push(body);
        return Promise.resolve({ text: () => body.promise });
      },
      setInterval: (callback: () => void) => {
        intervalCallbacks.push(callback);
        return 1;
      },
      setTimeout,
      clearTimeout,
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });

    expect(fetchedPaths).toEqual(["/api/v1/settings/public"]);
    expect(intervalCallbacks).toHaveLength(1);

    intervalCallbacks[0]();
    intervalCallbacks[0]();
    expect(
      fetchedPaths,
      "ticks during a slow round must not start another round",
    ).toHaveLength(1);

    responseBodies[0].resolve('{"code":0}');
    await flushPromises();
    expect(fetchedPaths).toEqual(["/api/v1/settings/public", "/api/status"]);
    expect(
      navigations,
      "a partial round must not emit a callback",
    ).toHaveLength(0);

    intervalCallbacks[0]();
    expect(
      fetchedPaths,
      "the second delayed candidate is still part of the same round",
    ).toHaveLength(2);

    responseBodies[1].resolve('{"success":true}');
    await flushPromises();
    expect(navigations).toHaveLength(1);

    const encoded = new URL(navigations[0]).searchParams.get("d");
    expect(encoded).not.toBeNull();
    const batch = JSON.parse(
      Buffer.from(encoded!, "base64url").toString("utf8"),
    );
    expect(batch).toEqual([
      {
        candidate_id: "sub2api",
        body: '{"code":0}',
        status: null,
        content_type: null,
        body_bytes: Buffer.byteLength('{"code":0}'),
        detector_body_bytes: Buffer.byteLength('{"code":0}'),
        json_like: true,
        error_kind: null,
      },
      {
        candidate_id: "newapi",
        body: '{"success":true}',
        status: null,
        content_type: null,
        body_bytes: Buffer.byteLength('{"success":true}'),
        detector_body_bytes: Buffer.byteLength('{"success":true}'),
        json_like: true,
        error_kind: null,
      },
    ]);

    intervalCallbacks[0]();
    expect(
      fetchedPaths,
      "a finished round releases the next interval",
    ).toHaveLength(3);
  });

  it("keeps a valid oversized JSON detector response within the callback limit", async () => {
    const navigations: string[] = [];
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };
    const oversizedBody = JSON.stringify({
      code: 0,
      data: {
        site_name: "Large Sub2API",
        version: "1.2.3",
        api_base_url: "",
        registration_enabled: true,
        promo_code_enabled: true,
        invitation_code_enabled: true,
        announcements: Array.from({ length: 5000 }, (_, index) => ({
          id: index,
          content: "x".repeat(80),
        })),
      },
    });
    expect(Buffer.byteLength(oversizedBody)).toBeGreaterThan(300_000);

    const sandbox = {
      window: { location },
      document: { readyState: "complete" },
      fetch: (path: string) => {
        const body =
          path === "/api/v1/settings/public"
            ? oversizedBody
            : '{"success":false}';
        return Promise.resolve({
          status: 200,
          headers: { get: () => "application/json" },
          text: () => Promise.resolve(body),
        });
      },
      setInterval: () => 1,
      setTimeout,
      clearTimeout,
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });
    await flushPromises();

    expect(navigations).toHaveLength(1);
    const encoded = new URL(navigations[0]).searchParams.get("d");
    expect(encoded).not.toBeNull();
    const batch = JSON.parse(
      Buffer.from(encoded!, "base64url").toString("utf8"),
    );
    expect(Buffer.byteLength(batch[0].body)).toBeLessThanOrEqual(64 * 1024);
    expect(JSON.parse(batch[0].body)).toEqual({
      code: 0,
      data: {
        site_name: "Large Sub2API",
        version: "1.2.3",
        api_base_url: "",
        registration_enabled: true,
        promo_code_enabled: true,
        invitation_code_enabled: true,
      },
    });
    expect(batch[0]).toMatchObject({
      candidate_id: "sub2api",
      body_bytes: Buffer.byteLength(oversizedBody),
      detector_body_bytes: Buffer.byteLength(batch[0].body),
      json_like: true,
      error_kind: null,
    });
  });

  it("uses the page's candidate-specific bearer session for authenticated browser probes", async () => {
    const requests: Array<{
      path: string;
      authorization: string | undefined;
    }> = [];
    const navigations: string[] = [];
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/dashboard";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };

    const sandbox = {
      window: { location },
      document: { readyState: "complete" },
      localStorage: {
        getItem: (key: string) =>
          key === "auth_token" ? "browser-session-token" : null,
      },
      fetch: (
        path: string,
        init: { headers?: Record<string, string> } = {},
      ) => {
        const authorization = init.headers?.Authorization;
        requests.push({ path, authorization });
        const body =
          path === "/api/v1/settings/public" &&
          authorization === "Bearer browser-session-token"
            ? '{"code":0}'
            : "<html>verification required</html>";
        return Promise.resolve({ text: () => Promise.resolve(body) });
      },
      setInterval: () => 1,
      setTimeout,
      clearTimeout,
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });
    await flushPromises();

    expect(requests[0]).toEqual({
      path: "/api/v1/settings/public",
      authorization: "Bearer browser-session-token",
    });
    expect(requests[1]).toEqual({
      path: "/api/status",
      authorization: undefined,
    });
    expect(navigations).toHaveLength(1);
  });

  it("reports safe response metadata when a verification page is not JSON", async () => {
    const navigations: string[] = [];
    const verificationBody = "<html><body>verification required</body></html>";
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/dashboard";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };

    const sandbox = {
      window: { location },
      document: { readyState: "complete" },
      fetch: () =>
        Promise.resolve({
          status: 403,
          headers: {
            get: (name: string) =>
              name.toLowerCase() === "content-type"
                ? "text/html; charset=UTF-8"
                : null,
          },
          text: () => Promise.resolve(verificationBody),
        }),
      setInterval: () => 1,
      setTimeout,
      clearTimeout,
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });
    await flushPromises();

    expect(navigations).toHaveLength(1);
    const encoded = new URL(navigations[0]).searchParams.get("d");
    expect(encoded).not.toBeNull();
    const batch = JSON.parse(
      Buffer.from(encoded!, "base64url").toString("utf8"),
    );
    expect(batch).toEqual([
      {
        candidate_id: "sub2api",
        body: "",
        status: 403,
        content_type: "text/html; charset=UTF-8",
        body_bytes: Buffer.byteLength(verificationBody),
        detector_body_bytes: 0,
        json_like: false,
        error_kind: null,
      },
      {
        candidate_id: "newapi",
        body: "",
        status: 403,
        content_type: "text/html; charset=UTF-8",
        body_bytes: Buffer.byteLength(verificationBody),
        detector_body_bytes: 0,
        json_like: false,
        error_kind: null,
      },
    ]);
    expect(JSON.stringify(batch)).not.toContain(verificationBody);
  });

  it("times out one candidate, continues the batch, and emits only after settlement", async () => {
    const fetchedPaths: string[] = [];
    const navigations: string[] = [];
    const timeoutCallbacks: Array<() => void> = [];
    const secondBody = deferred<string>();
    const location = {
      origin: "https://relay.example",
      get href() {
        return navigations.at(-1) ?? "https://relay.example/login";
      },
      set href(value: string) {
        navigations.push(value);
      },
    };

    const sandbox = {
      window: { location },
      document: { readyState: "complete" },
      fetch: (path: string) => {
        fetchedPaths.push(path);
        if (path === "/api/v1/settings/public") {
          return new Promise(() => {});
        }
        return Promise.resolve({ text: () => secondBody.promise });
      },
      setInterval: () => 1,
      setTimeout: (callback: () => void) => {
        timeoutCallbacks.push(callback);
        return timeoutCallbacks.length;
      },
      clearTimeout: () => {},
      AbortController,
      TextEncoder,
      btoa: (value: string) => Buffer.from(value, "binary").toString("base64"),
      JSON,
      Error,
    };

    vm.runInNewContext(readFileSync(SCRIPT, "utf8"), sandbox, {
      timeout: 5000,
    });
    expect(fetchedPaths).toEqual(["/api/v1/settings/public"]);
    expect(navigations).toHaveLength(0);

    timeoutCallbacks[0]();
    await flushPromises();
    expect(fetchedPaths).toEqual(["/api/v1/settings/public", "/api/status"]);
    expect(navigations).toHaveLength(0);

    secondBody.resolve('{"success":true}');
    await flushPromises();
    expect(navigations).toHaveLength(1);

    const encoded = new URL(navigations[0]).searchParams.get("d");
    expect(encoded).not.toBeNull();
    const batch = JSON.parse(
      Buffer.from(encoded!, "base64url").toString("utf8"),
    );
    expect(batch[0]).toEqual({
      candidate_id: "sub2api",
      body: "",
      status: null,
      content_type: null,
      body_bytes: 0,
      detector_body_bytes: 0,
      json_like: false,
      error_kind: "ProbeTimeout",
    });
    expect(batch[1]).toMatchObject({
      candidate_id: "newapi",
      body: '{"success":true}',
      json_like: true,
      error_kind: null,
    });
  });
});
