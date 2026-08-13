import { describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import vm from "node:vm";

/**
 * 登录注入脚本**真的跑一遍**（不是字符串断言）。
 *
 * ## 为什么需要这条测试
 *
 * `src-tauri/src/relay/login.rs` 生成的是**另一门语言的代码**，而 Rust 侧
 * 那批测试全是字符串断言 —— 它们能验「该出现的出现了」，验不了「这段 JS
 * 跑得起来」。
 *
 * 2026-08-04 一个 P0 正是这样溜过 2562 个绿测试的：`tryPrefillPromo()` 的
 * **调用点在、定义不在**（定义在一段 `promo_code = None` 时留空的 snippet 里）
 * ⇒ 所有字符串断言全过，而脚本一执行就
 * `ReferenceError: tryPrefillPromo is not defined`。
 *
 * 而且后果不止「优惠码没填上」：那个 throw 在轮询回调里、在 `clearInterval`
 * 之前 ⇒ 定时器永不清除；凭据回传只因 `trySend()` 排在前面才侥幸活着。
 *
 * ⇒ 只有**执行**才能拦住这一类。字符串断言拦不住，编译器也管不到。
 *
 * ## 为什么不用 jsdom
 *
 * 只需要「这段代码能跑完、且碰到我们关心的那几个 DOM 点」，用 `vm` + 手搓的
 * 极小 stub 更可控：哪个 API 被调了一目了然，不会因为 jsdom 自己的实现差异
 * 让失败变得难读。
 */

const REPO = resolve(__dirname, "../..");
const SCRIPT_DIR = resolve(REPO, "src-tauri/target");

/**
 * 生成脚本素材。
 *
 * 走 `cargo test --ignored export_login_scripts`（`login.rs` 里那个导出用例）
 * 而不是在这里重写一份生成逻辑 —— 后者等于让测试自带一份可能与生产不一致的
 * 副本，那正是这条测试要避免的东西。
 */
function ensureScripts(): void {
  const needed = [
    ...["with-promo", "no-promo"].map((n) => `login-script-${n}.js`),
    "api-fetch-script.js",
    "api-fetch-script-post.js",
  ].map((n) => resolve(SCRIPT_DIR, n));
  if (needed.every(existsSync)) return;
  execFileSync(
    "cargo",
    [
      "test",
      "--manifest-path",
      resolve(REPO, "src-tauri/Cargo.toml"),
      "--lib",
      "--",
      "--ignored",
      "export_login_scripts",
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

/**
 * 素材备得出来吗。
 *
 * **为什么要这个判断，而不是让 `ensureScripts()` 直接抛**：这条测试要能编 Rust
 * （`cargo test --lib` 会编整个 crate），而 CI 的 `Frontend Checks` 跑在 ubuntu 上、
 * 只装 Node —— Linux 下编这个 crate 需要 GTK / glib / webkit2gtk 全套系统库
 * （那些只装在 `Backend Checks` 里）⇒ 前端那个 job 里必然编不过。
 *
 * 2026-08-04 实测到的正是这个：`The system library glib-2.0 required by crate
 * glib-sys was not found` ⇒ 这条测试的 10 个用例在 CI 全红，而**本机一直是绿的**
 * —— 因为本机 `src-tauri/target/` 里躺着之前生成的素材，每次都走上面那个 `return`
 * 短路、从不真的调 cargo。典型的「本机有状态、CI 干净」。
 *
 * ⇒ 备不出素材时**显式 skip 并把原因打出来**，不静默也不误报成失败。
 * 真正跑它的是 CI 里 `Backend Checks` 那个 job（有 Rust 环境，见 ci.yml），
 * 以及维护者本机的六道闸。
 */
function scriptsAvailable(): boolean {
  try {
    ensureScripts();
    return true;
  } catch (e) {
    const why = e instanceof Error ? e.message.split("\n")[0] : String(e);
    console.warn(
      `⚠️ 跳过「登录注入脚本能真的执行」——素材备不出来（需要能编 Rust）：${why}\n` +
        `   这条测试由 CI 的 Backend Checks 与本机六道闸负责，见本文件顶部说明。`,
    );
    return false;
  }
}

/** 记录脚本碰了哪些 DOM 点，供断言检查。 */
interface Trace {
  promoValue: string | null;
  emailValue: string | null;
  events: string[];
  storage: Record<string, string>;
  navigatedTo: string | null;
  intervalCallback: (() => void) | null;
  /** 优惠码那个框本身 —— 「用户清空」那条测试要直接操作它。 */
  promoEl: { value: string } | null;
  /**
   * 换一个**全新的**优惠码框（模拟 Vue remount）。
   *
   * `/login` ↔ `/register` 是两个独立路由组件，来回跳会卸载旧元素、挂载新的 ——
   * 而「按元素身份记」的全部意义就是那时还能再填一次。
   */
  remountPromoField: () => void;
}

/**
 * 在一个极小的 stub 环境里跑脚本，返回它碰过的东西。
 *
 * stub 只提供脚本真正用到的那几样。`origin` 故意与脚本里的
 * `ALLOWED_ORIGIN` 一致 —— 否则那道 origin 守卫会让脚本直接早退，
 * 测试就什么都没验到（那是个很容易自欺的失败模式）。
 */
function runScript(js: string, opts: { promoFieldExists: boolean }): Trace {
  const trace: Trace = {
    promoValue: null,
    emailValue: null,
    events: [],
    storage: {},
    navigatedTo: null,
    intervalCallback: null,
    promoEl: null,
    remountPromoField: () => {},
  };

  const makeInput = (onSet: (v: string) => void) => ({
    _v: "",
    get value() {
      return this._v;
    },
    set value(v: string) {
      this._v = v;
      onSet(v);
    },
    dispatchEvent(e: { type: string }) {
      trace.events.push(e.type);
    },
  });

  // 用一个可替换的引用：remount 时换成一个**全新对象**（新的元素身份）。
  let promoEl = makeInput((v) => (trace.promoValue = v));
  const emailEl = makeInput((v) => (trace.emailValue = v));
  trace.promoEl = promoEl;
  trace.remountPromoField = () => {
    promoEl = makeInput((v) => (trace.promoValue = v));
    trace.promoEl = promoEl;
  };

  const sandbox = {
    window: {} as Record<string, unknown>,
    document: {
      querySelector(sel: string) {
        if (sel === "#promo_code")
          return opts.promoFieldExists ? promoEl : null;
        if (sel === "#email") return emailEl;
        return null;
      },
    },
    Event: class {
      type: string;
      constructor(type: string) {
        this.type = type;
      }
    },
    setInterval: (fn: () => void) => {
      trace.intervalCallback = fn;
      return 1;
    },
    clearInterval: () => {},
    setTimeout: (fn: () => void) => {
      fn();
      return 1;
    },
    TextEncoder,
    btoa: (s: string) => Buffer.from(s, "binary").toString("base64"),
    Date,
    JSON,
    Number,
    WeakSet,
    console,
  };
  // `window.top === window.self` 才让脚本跑（它防同源 iframe 里重复执行）。
  sandbox.window.top = sandbox.window;
  sandbox.window.self = sandbox.window;
  sandbox.window.location = {
    origin: "https://bestapi.store",
    get href() {
      return trace.navigatedTo ?? "https://bestapi.store/register";
    },
    set href(v: string) {
      trace.navigatedTo = v;
    },
  };
  sandbox.window.localStorage = {
    getItem: (k: string) => trace.storage[k] ?? null,
    setItem: (k: string, v: string) => {
      trace.storage[k] = v;
    },
  };

  vm.runInNewContext(js, sandbox, { timeout: 5000 });
  return trace;
}

describe.runIf(scriptsAvailable())("登录注入脚本能真的执行", () => {
  const read = (name: string) =>
    readFileSync(resolve(SCRIPT_DIR, `login-script-${name}.js`), "utf8");

  /**
   * ⭐⭐ **这条就是那个 P0 的回归闸。**
   *
   * `promo_code = None` 是**常见情况**（编译期表里只有一个站，其余全走这条），
   * 所以这一条比 with-promo 那条更要紧。
   */
  it("没有优惠码的站：脚本跑完不抛异常", () => {
    const js = read("no-promo");
    expect(() => runScript(js, { promoFieldExists: false })).not.toThrow();
  });

  /**
   * ⭐ **轮询回调也必须跑得起来。**
   *
   * 那个 P0 最坏的一半在这里：throw 发生在轮询回调里、在 `clearInterval` 之前
   * ⇒ 定时器永不清除。只测「顶层跑得完」验不到它。
   */
  it("没有优惠码的站：轮询回调也不抛异常", () => {
    const js = read("no-promo");
    const trace = runScript(js, { promoFieldExists: false });
    expect(trace.intervalCallback, "脚本该注册了一个轮询").not.toBeNull();
    expect(() => trace.intervalCallback!()).not.toThrow();
  });

  it("有优惠码的站：把码填进那个框并派 input 事件", () => {
    const js = read("with-promo");
    const trace = runScript(js, { promoFieldExists: true });
    expect(trace.promoValue).toBe("LOONGPORT");
    // v-model 靠 input 同步 formData；站点的实时校验也挂在它上面。
    expect(trace.events).toContain("input");
    expect(trace.events).toContain("change");
  });

  it("有优惠码的站：轮询回调也不抛异常", () => {
    const js = read("with-promo");
    const trace = runScript(js, { promoFieldExists: true });
    expect(() => trace.intervalCallback!()).not.toThrow();
  });

  /**
   * ⭐ **用户清空后不该被填回** —— 那条 `WeakSet` 判据的行为验证。
   *
   * 前面那条 Rust 侧的闸断言的是「代码里有 `promoFilled.has(el)`」；
   * 这条验的是**实际行为**：清空 → 再跑一轮 → 仍然是空。
   */
  it("用户清空优惠码后，下一轮轮询不会填回去", () => {
    const js = read("with-promo");
    const trace = runScript(js, { promoFieldExists: true });
    expect(trace.promoValue).toBe("LOONGPORT");

    // **真的清空那个框**（模拟用户按删除键），并把 trace 归零。
    trace.promoEl!.value = "";
    trace.promoValue = null;

    // 再跑一轮轮询。脚本内部已把这个元素记进 WeakSet ⇒ 不该再填。
    trace.intervalCallback!();

    expect(
      trace.promoEl!.value,
      "填过的框即使被清空也不该再填 —— 否则用户清不掉这个码",
    ).toBe("");
    expect(trace.promoValue, "不该发生第二次写入").toBeNull();
  });

  /**
   * ⭐⭐ **重新挂载的框还要能填** —— 这是「按元素身份记」相对「一次性布尔标志」
   * 的全部意义所在。
   *
   * 真实路径：重登的行落 `/login`（那儿没有优惠码框）→ 用户点页脚「去注册」
   * → Vue 卸载 `LoginView`、挂载 `RegisterView`，那个框是**新元素**。
   * 而站内路由跳转**不重跑** `initialization_script`，靠的就是同一个轮询继续跑。
   *
   * 用布尔标志的话这一步永远填不上；Rust 侧那条结构断言验不到这个行为
   * （它只能验「用了 WeakSet、拿元素当键」），所以必须在这里跑一遍。
   */
  it("框被重新挂载（换成新元素）后仍然会填", () => {
    const js = read("with-promo");
    const trace = runScript(js, { promoFieldExists: true });
    expect(trace.promoValue).toBe("LOONGPORT");

    // 换一个全新的框（模拟 /login → /register 的 remount）。
    trace.remountPromoField();
    trace.promoValue = null;
    expect(trace.promoEl!.value, "新元素初始是空的").toBe("");

    trace.intervalCallback!();

    expect(
      trace.promoEl!.value,
      "重挂出来的新框该被填上 —— 否则从登录页跳去注册页的用户永远拿不到这个码",
    ).toBe("LOONGPORT");
  });

  /** 两种变体都要照旧完成凭据回传那半（优惠码不该影响它）。 */
  it("两种变体都注入了凭据回传逻辑", () => {
    for (const name of ["with-promo", "no-promo"]) {
      const trace = runScript(read(name), {
        promoFieldExists: name === "with-promo",
      });
      // 没有 token 时不该跳转（那才是正常的「还没登录完」）。
      expect(trace.navigatedTo, `${name}: 没 token 不该回传`).toBeNull();
      // 重登标识要填进邮箱框（导出时传的是 me@x.com）。
      expect(trace.emailValue, `${name}: 该预填登录标识`).toBe("me@x.com");
    }
  });

  /** aff 码那段（两种变体都带）要真的写进 localStorage。 */
  it("邀请码被种进 localStorage", () => {
    const trace = runScript(read("no-promo"), { promoFieldExists: false });
    const raw = trace.storage["affiliate_referral_code"];
    expect(raw, "aff 码该落进那个键").toBeTruthy();
    const parsed = JSON.parse(raw);
    expect(parsed.code).toBe("AFF12345678");
    expect(parsed.expiresAt).toBeGreaterThan(Date.now());
  });
});

/**
 * 「登录窗在页面上下文里代拉 API 请求」的脚本**真的跑一遍**（不是字符串断言）。
 *
 * 与上面登录脚本同一条理由：`api_fetch_script` 也是 Rust 生成的另一门语言代码，
 * 字符串断言验不了「跑得起来、重试逻辑真的会回传」。这条专门守那段最险的分支：
 * **403 HTML 只重试、API 自己的错误不重试** —— 写错一次的表现就是「请求多等
 * 3 秒然后报超时」，纯看字符串看不出来。
 *
 * 定时器用「可手动推进的假时钟」而不是真睡：重试间隔 1s、兜底 5s，真跑会拖慢测试，
 * 而且**真 setTimeout 会让 5s 兜底和 1s 重试并发抢跑**（假时钟下谁先谁后由测试说了算，
 * 不会出现 flaky）。
 */
describe.runIf(scriptsAvailable())("浏览器代拉脚本能真的执行", () => {
  const read = (name: string) =>
    readFileSync(resolve(SCRIPT_DIR, name), "utf8");
  const readGet = () => read("api-fetch-script.js");
  const readPost = () => read("api-fetch-script-post.js");

  /** 解出回传导航 URL 里的 {status, body, ct} 载荷（与 Rust 侧 decode 同构）。 */
  function decodePayload(url: string): {
    status: number;
    body: string;
    ct: string;
  } {
    const b64 = url
      .slice(url.indexOf("?d=") + 3)
      .replace(/-/g, "+")
      .replace(/_/g, "/");
    return JSON.parse(Buffer.from(b64, "base64").toString("utf8"));
  }

  /**
   * 极小的 stub 环境：fetch 可编程（按调用顺序出响应）、定时器进队列可手动推进。
   *
   * `origin` 与导出脚本里的 `ALLOWED_ORIGIN`（`https://bestapi.store`）一致，
   * 否则 origin 守卫会让脚本直接早退、什么也验不到。
   */
  function runScript(
    source: string,
    responses: Array<
      { status: number; ct: string; body: string } | { never: true }
    >,
  ) {
    const navigations: string[] = [];
    const fetchCalls: Array<{ url: string; method: string; body?: unknown }> =
      [];
    let now = 0;
    const timers: Array<{ at: number; fire: () => void }> = [];

    const sandbox = {
      window: {} as Record<string, unknown>,
      TextEncoder,
      btoa: (s: string) => Buffer.from(s, "binary").toString("base64"),
      setTimeout: (fn: () => void, ms: number) => {
        timers.push({ at: now + ms, fire: fn });
        return timers.length;
      },
      fetch: (url: string, init?: { method?: string; body?: string }) => {
        const spec =
          responses[Math.min(fetchCalls.length, responses.length - 1)];
        fetchCalls.push({
          url,
          method: init?.method ?? "GET",
          body: init?.body,
        });
        if ("never" in spec) return new Promise(() => {});
        return Promise.resolve({
          status: spec.status,
          headers: { get: () => spec.ct },
          text: () => Promise.resolve(spec.body),
        });
      },
      console,
    };
    sandbox.window.top = sandbox.window;
    sandbox.window.self = sandbox.window;
    sandbox.window.location = {
      origin: "https://bestapi.store",
      get href() {
        return "https://bestapi.store/login";
      },
      set href(v: string) {
        navigations.push(v);
      },
    };

    vm.runInNewContext(source, sandbox, { timeout: 5000 });

    return {
      navigations,
      // **getter 而不是值快照**：`{ fetchCalls }` 字面量会在创建时把当时的数值拷贝进去，
      // 之后闭包里再 ++ 也读不到（这次实测踩过：重试明明跑了 3 次，harness 一直显示 1）。
      get fetchCount() {
        return fetchCalls.length;
      },
      get fetchMethods() {
        return fetchCalls.map((c) => c.method);
      },
      /** 代拉打的是哪个 URL —— 脚本里那个字面量必须是**完整 URL（含 query）**，
          相对路径会在页面已跳转到别的路由时打错端点。 */
      get fetchUrls() {
        return fetchCalls.map((c) => c.url);
      },
      get fetchBodies() {
        return fetchCalls.map((c) => c.body);
      },
      /** 把假时钟往前拨，触发到期的定时器（按到期顺序）。 */
      advance(ms: number) {
        now += ms;
        const due = timers
          .filter((t) => t.at <= now)
          .sort((a, b) => a.at - b.at);
        timers.splice(0, timers.length, ...timers.filter((t) => t.at > now));
        for (const t of due) t.fire();
      },
    };
  }

  /** 让 promise 链跑完（fetch → text() → then），再做断言。 */
  const flush = () => new Promise<void>((r) => setImmediate(r));

  it("GET 200 一次回传，不重试", async () => {
    const h = runScript(readGet(), [
      {
        status: 200,
        ct: "application/json",
        body: '{"code":0,"message":"success","data":{"id":7,"email":"a@b.c","username":"nicky"}}',
      },
    ]);
    await flush();
    expect(h.fetchCount, "200 不该重试").toBe(1);
    expect(h.navigations).toHaveLength(1);
    const payload = decodePayload(h.navigations[0]);
    expect(payload.status).toBe(200);
    const account = JSON.parse(payload.body);
    expect(account.data.id).toBe(7);
  });

  it("403 HTML 重试到成功（最多 3 次），不被 5s 兜底抢跑", async () => {
    const h = runScript(readGet(), [
      {
        status: 403,
        ct: "text/html; charset=utf-8",
        body: "<html>challenge</html>",
      },
      {
        status: 403,
        ct: "text/html; charset=utf-8",
        body: "<html>challenge</html>",
      },
      {
        status: 200,
        ct: "application/json",
        body: '{"code":0,"message":"success","data":{"id":9}}',
      },
    ]);
    // 第一次 403：不能当场回传，要等重试。
    await flush();
    expect(h.navigations).toHaveLength(0);
    // 第二次 403（t=1000）：还是不回传。
    h.advance(1000);
    await flush();
    expect(h.navigations).toHaveLength(0);
    // 第三次 200（t=2000）：回传成功，且 fetch 正好 3 次。
    h.advance(1000);
    await flush();
    expect(h.fetchCount, "403 该重试到 3 次").toBe(3);
    expect(h.navigations).toHaveLength(1);
    expect(decodePayload(h.navigations[0]).status).toBe(200);
    // 5s 兜底还躺着没触发 —— 重试成功时它不该再发第二条。
    h.advance(3000);
    await flush();
    expect(h.navigations, "兜底不该覆盖已经成功的回传").toHaveLength(1);
  });

  it("403 连着重 3 次仍失败：把最后一次 403 原样回传", async () => {
    const h = runScript(readGet(), [
      { status: 403, ct: "text/html", body: "<html>1</html>" },
      { status: 403, ct: "text/html", body: "<html>2</html>" },
      { status: 403, ct: "text/html", body: "<html>3</html>" },
    ]);
    // 先让第一次 fetch 的 promise 链跑完（重试定时器是它排的，不等就拨时钟会扑空）。
    await flush();
    h.advance(1000);
    await flush();
    h.advance(1000);
    await flush();
    expect(h.fetchCount, "最多 3 次").toBe(3);
    expect(h.navigations).toHaveLength(1);
    expect(decodePayload(h.navigations[0]).status).toBe(403);
  });

  it("401（API 自己的错误）不重试，原样回传", async () => {
    const h = runScript(readGet(), [
      { status: 401, ct: "application/json", body: '{"code":401}' },
    ]);
    await flush();
    expect(h.fetchCount, "401 不该重试").toBe(1);
    expect(h.navigations).toHaveLength(1);
    expect(decodePayload(h.navigations[0]).status).toBe(401);
  });

  it("页面 fetch 挂起：5 秒兜底回传超时（-3）", async () => {
    const h = runScript(readGet(), [{ never: true }]);
    await flush();
    expect(h.navigations).toHaveLength(0);
    h.advance(5000);
    await flush();
    expect(h.navigations).toHaveLength(1);
    expect(decodePayload(h.navigations[0]).status).toBe(-3);
  });

  it("POST 带 body 与幂等头，重试也保持同一份请求", async () => {
    const h = runScript(readPost(), [
      { status: 403, ct: "text/html", body: "<html>challenge</html>" },
      {
        status: 200,
        ct: "application/json",
        body: '{"code":0,"data":{"id":1}}',
      },
    ]);
    await flush();
    h.advance(1000);
    await flush();
    expect(h.fetchCount).toBe(2);
    expect(h.fetchMethods).toEqual(["POST", "POST"]);
    // 每次重试都带着同一份 body（建 Key 的 JSON + 幂等键都原样重放）。
    expect(h.fetchBodies[0]).toContain("provision:test");
    expect(h.fetchBodies[1]).toBe(h.fetchBodies[0]);
    expect(h.navigations).toHaveLength(1);
    expect(decodePayload(h.navigations[0]).status).toBe(200);
  });
});
