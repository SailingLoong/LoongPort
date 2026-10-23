import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import vm from "node:vm";
import { describe, expect, it, vi } from "vitest";

// Execute the actual Rust template, not a copy of the login state machine.
const rust = readFileSync(
  resolve(__dirname, "../../src-tauri/src/vendor/opencode.rs"),
  "utf8",
);
const script = rust.split('r#"(function ()')[1].split('"#')[0];
const js = ("(function ()" + script)
  .replaceAll("{{", "{")
  .replaceAll("}}", "}")
  .replaceAll("{CREDS_SCHEME}", "loongport-opencode-creds");
function page(pathname: string) {
  const timers: Array<() => void> = [];
  const banners: string[] = [];
  let now = 0;
  const location = {
    host: "opencode.ai",
    pathname,
    href: "",
    assign(path: string) {
      this.pathname = path;
    },
  };
  const window = {
    location,
    top: null as unknown,
    self: null as unknown,
    fetch: vi.fn(async () => ({ clone: () => ({ text: async () => "[]" }) })),
  };
  window.top = window.self = window;
  const sandbox = {
    window,
    TextEncoder,
    btoa: (s: string) => Buffer.from(s, "binary").toString("base64"),
    Date: { now: () => now },
    setInterval: (f: () => void) => {
      timers.push(f);
      return timers.length;
    },
    clearInterval: () => {},
    setTimeout: () => {},
    document: {
      createElement: () => ({ textContent: "", style: { cssText: "" } }),
      body: {
        appendChild: (el: { textContent: string }) =>
          banners.push(el.textContent),
      },
      querySelector: vi.fn(() => {
        throw new Error("must not create credentials");
      }),
    },
  };
  const fetch = window.fetch;
  vm.runInNewContext(js, sandbox);
  return {
    fetch,
    location,
    window,
    banners,
    querySelector: sandbox.document.querySelector,
    emptyList() {
      (
        window as unknown as { __loongport_oc: { log: string[] } }
      ).__loongport_oc.log.push("[]");
    },
    response(key: string) {
      (
        window as unknown as { __loongport_oc: { log: string[] } }
      ).__loongport_oc.log.push(
        JSON.stringify([
          { name: "LoongPort专用/awrk_example", key, keyDisplay: "masked" },
        ]),
      );
    },
    tick(ms = 1000) {
      now += ms;
      for (const f of timers) f();
    },
    retry() {
      vm.runInNewContext(js, sandbox);
    },
  };
}
describe("OpenCode console login handoff", () => {
  it("ends unsupported new console instead of spinning", () => {
    const p = page("/console/workspace/example");
    p.tick();
    expect(p.location.href).toContain("error=console_key_required");
    expect(new URL(p.location.href).searchParams.has("d")).toBe(false);
    expect(p.fetch).not.toHaveBeenCalled();
    expect(p.querySelector).not.toHaveBeenCalled();
  });
  it("allows login then notices an SPA console navigation", () => {
    const p = page("/console/login");
    p.tick();
    expect(p.location.href).toBe("");
    p.location.pathname = "/console/workspace/example";
    p.tick();
    expect(p.location.href).toContain("error=console_key_required");
  });
  it("times out unknown routes explicitly", () => {
    const p = page("/unknown");
    p.tick(121000);
    expect(p.location.href).toContain("error=login_timeout");
  });
  it("preserves the legacy workspace keys navigation", () => {
    const p = page("/workspace/wrk_example");
    expect(p.location.pathname).toBe("/workspace/wrk_example/keys");
  });
  it.each(["sk-synthetic-key", "oc_sk_example_synthetic"])(
    "retains a complete existing key: %s",
    (key) => {
      const p = page("/workspace/wrk_example/keys");
      p.response(key);
      p.tick();
      const payload = JSON.parse(
        Buffer.from(
          new URL(p.location.href).searchParams.get("d")!,
          "base64url",
        ).toString(),
      );
      expect(payload.keys).toEqual([
        { name: "LoongPort专用/awrk_example", key },
      ]);
    },
  );
  it.each([
    "oc_sk_prefix",
    "oc_sk_prefix_",
    "oc_sk_prefix_***",
    "oc_sk_prefix_…",
    "sk-masked...",
  ])("never accepts a masked/prefix-only key: %s", (key) => {
    const p = page("/workspace/wrk_example/keys");
    p.response(key);
    p.tick();
    expect(p.location.href).toBe("");
    p.tick(121000);
    expect(p.location.href).toContain("error=login_timeout");
  });
  it("does not create keys when the legacy list is empty", () => {
    const p = page("/workspace/wrk_example/keys");
    p.response("");
    p.tick();
    expect(p.location.href).toBe("");
    expect(js).not.toContain(".click()");
  });
  it("is idempotent and a new page can retry after a failed handoff", () => {
    const p = page("/console/workspace/example");
    const first = p.location.href;
    p.retry();
    p.tick();
    expect(p.location.href).toBe(first);
    const fresh = page("/console/login");
    expect(fresh.location.href).toBe("");
    fresh.location.pathname = "/console/workspace/example";
    fresh.tick();
    expect(fresh.location.href).toContain("error=console_key_required");
  });
  it("does not install credential response hooks on new Console", () => {
    const p = page("/console/workspace/example");
    expect(
      (p.window as unknown as { __loongport_oc: { hooked?: boolean } })
        .__loongport_oc.hooked,
    ).toBeUndefined();
  });
  it("notices a legacy keys page changing to Console in the same document", () => {
    const p = page("/workspace/wrk_example/keys");
    p.location.pathname = "/console/workspace/example";
    p.tick();
    expect(p.location.href).toContain("error=console_key_required");
  });
  it("guides a genuinely empty list without claiming success or creating a key", () => {
    const p = page("/workspace/wrk_example/keys");
    p.emptyList();
    p.tick(6000);
    expect(p.banners.join("")).toContain("不会自动创建");
    expect(p.banners.join("")).toContain("Personal API key");
    expect(p.banners.join("")).toContain("从本机 OpenCode 导入");
    expect(p.banners.join("")).not.toContain("自定义 API");
    expect(p.fetch).not.toHaveBeenCalled();
    expect(p.querySelector).not.toHaveBeenCalled();
    expect(p.location.href).toBe("");
    p.tick(121000);
    expect(p.location.href).toContain("error=login_timeout");
  });
  it("guides a masked legacy key to existing official forms without transmitting it", () => {
    const p = page("/workspace/wrk_example/keys");
    p.response("oc_sk_prefix_***");
    p.tick();
    expect(p.banners.join("")).toContain("Personal API key");
    expect(p.banners.join("")).toContain("从本机 OpenCode 导入");
    expect(p.banners.join("")).not.toContain("自定义 API");
    expect(p.banners.join("")).not.toContain("oc_sk_prefix");
    expect(p.location.href).toBe("");
    expect(p.fetch).not.toHaveBeenCalled();
    expect(p.querySelector).not.toHaveBeenCalled();
    p.tick(121000);
    expect(new URL(p.location.href).searchParams.get("error")).toBe(
      "login_timeout",
    );
    expect(new URL(p.location.href).searchParams.has("d")).toBe(false);
  });
});
