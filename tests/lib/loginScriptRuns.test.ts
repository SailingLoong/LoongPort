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
 * `src-tauri/src/operator/login.rs` 生成的是**另一门语言的代码**，而 Rust 侧
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
  const both = ["with-promo", "no-promo"].map((n) =>
    resolve(SCRIPT_DIR, `login-script-${n}.js`),
  );
  if (both.every(existsSync)) return;
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

describe("登录注入脚本能真的执行", () => {
  const read = (name: string) => {
    ensureScripts();
    return readFileSync(resolve(SCRIPT_DIR, `login-script-${name}.js`), "utf8");
  };

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
