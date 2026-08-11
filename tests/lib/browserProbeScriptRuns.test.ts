import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import vm from "node:vm";
import { describe, expect, it } from "vitest";

const REPO = resolve(__dirname, "../..");
const SCRIPT = resolve(REPO, "src-tauri/target/browser-probe-script.js");

function ensureScript(): void {
  if (existsSync(SCRIPT)) return;
  execFileSync(
    "cargo",
    [
      "test",
      "--manifest-path",
      resolve(REPO, "src-tauri/Cargo.toml"),
      "--lib",
      "--",
      "--ignored",
      "export_browser_probe_script",
    ],
    {
      stdio: "pipe",
      env: {
        ...process.env,
        PATH: `${process.env.HOME}/.cargo/bin:${process.env.PATH}`,
      },
    },
  );
}

function scriptAvailable(): boolean {
  try {
    ensureScript();
    return true;
  } catch (error) {
    const reason =
      error instanceof Error ? error.message.split("\n")[0] : String(error);
    console.warn(`Skipping browser probe execution test: ${reason}`);
    return false;
  }
}

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

describe.runIf(scriptAvailable())("browser probe generated script", () => {
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
      { candidate_id: "sub2api", body: '{"code":0}' },
      { candidate_id: "newapi", body: '{"success":true}' },
    ]);

    intervalCallbacks[0]();
    expect(
      fetchedPaths,
      "a finished round releases the next interval",
    ).toHaveLength(3);
  });
});
