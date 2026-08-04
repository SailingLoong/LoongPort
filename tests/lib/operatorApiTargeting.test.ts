import { describe, it, expect, vi, beforeEach } from "vitest";

/**
 * 闸：**运营商相关的命令必须把 `operatorId` 真的传到 invoke 那一层。**
 *
 * ## 这道闸守的是什么（2026-08-04 重写）
 *
 * 曾经这四个参数是 `Option<i64>` / `operatorId?: number`，**不传就回落到「当前站」**
 * （`creds::load()` 读 `is_current = 1` 的那一行）。那让「漏传」成为一个**静默错误**：
 * 类型合法、运行时不抛、只是作用到了另一个账号上 —— 用户看到「给 A 点获取密钥，
 * B 的档位变了」，极难归因。前端一度靠 `switchSite(id)` 先改全局当前站再调无参命令，
 * 两个运营商并发操作就互相串目标。
 *
 * **`is_current` 整个概念已删**（见后端 `operator/creds.rs` 模块文档），四个参数
 * 都收成必填 ⇒ 漏传现在是**编译错误**，那半边由 `tsc` 守着，不需要测试。
 *
 * 这道闸剩下的职责是另一半：**参数别在封装层被丢掉或改名**。
 * `invoke` 的第二个参数是个对象字面量，写错键名（`operator_id` vs `operatorId`）
 * 或忘了透传，`tsc` 一样过 —— 而后果仍是那条「作用到错的账号」。
 */

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

// 动态 import：mock 必须在模块求值前就位。
const { operatorApi } = await import("@/lib/api/operator");

describe("operatorApi 的运营商定位参数", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
  });

  it("login 把 operatorId 透传给后端", async () => {
    await operatorApi.login(7);
    expect(invokeMock).toHaveBeenCalledWith("operator_login", {
      operatorId: 7,
    });
  });

  it("provision 把 operatorId 透传给后端", async () => {
    await operatorApi.provision(42);
    expect(invokeMock).toHaveBeenCalledWith("operator_provision", {
      operatorId: 42,
    });
  });

  /**
   * `balance` 传错的后果与 login / provision **不同类，但更难发现**。
   *
   * 那两个是「动作作用到错的账号」，用户迟早看到反常（档位变了）。余额是**纯显示**：
   * 键名写错时每一行都显示同一个数字，界面看起来完全正常。
   */
  it("balance 把 operatorId 透传（传错会让每行显示同一个数字）", async () => {
    await operatorApi.balance(3);
    expect(invokeMock).toHaveBeenCalledWith("operator_balance", {
      operatorId: 3,
    });
  });

  /**
   * ⭐ `purchase` 传错是这组里**后果最严重**的一条：会让钱进错账号。
   *
   * 打开的充值页上显示的是那个账号自己的信息（我们注入的就是它的登录态）⇒
   * 用户不会察觉，钱**真的**充进别的账号。
   */
  it("purchase 把 operatorId 透传（传错会让钱充进别的账号）", async () => {
    await operatorApi.purchase(5);
    expect(invokeMock).toHaveBeenCalledWith("operator_purchase", {
      operatorId: 5,
    });
  });

  /**
   * ⭐ **`0` 必须照原样传下去，不能被当成「没给」。**
   *
   * 这条守的是回落时代残留的写法：`{ operatorId: operatorId ?? null }` 换成
   * `{ operatorId }` 之后语义就对了，但若有人「顺手」写成 `operatorId || null`，
   * `0` 会变成 `null` ⇒ Rust 侧 `i64` 参数缺失 ⇒ serde 报 missing field。
   *
   * SQLite 的 `AUTOINCREMENT` 从 1 起，所以 0 不是真实行 id ——
   * 但那是**库的约定，不是这一层的约定**，这一层的职责是原样透传。
   */
  it("operatorId = 0 也照原样传（别被 falsy 判据吞掉）", async () => {
    await operatorApi.provision(0);
    expect(invokeMock).toHaveBeenCalledWith("operator_provision", {
      operatorId: 0,
    });
  });

  /**
   * `checkSession` 不吃参数 —— 它**探每一行**。
   *
   * 曾经它探「当前站」那一行、返回 bool。那个形状只对单站界面成立：
   * 多行并列时探一行等于让另外 N-1 行继续显示错的登录态，而用户看不出区别。
   */
  it("checkSession 不带参数（它逐行探活）", async () => {
    invokeMock.mockResolvedValue([]);
    await operatorApi.checkSession();
    expect(invokeMock).toHaveBeenCalledWith("operator_check_session");
  });

  /**
   * `listTierRates` 的 `siteOrigin` 同理 —— 它决定「只查这个站的倍率」。
   *
   * 漏传的后果不是错账号而是**浪费请求**：每个档位一次 HTTP，用户给账号 A
   * 获取密钥时会把 B / C 的倍率也全重查一遍（他明确指出过这个现象）。
   */
  it("listTierRates 把 siteOrigin 透传，不传时发 null", async () => {
    invokeMock.mockResolvedValue([]);
    await operatorApi.listTierRates("codex", "https://bestapi.store");
    expect(invokeMock).toHaveBeenCalledWith("operator_list_tier_rates", {
      app: "codex",
      siteOrigin: "https://bestapi.store",
    });

    invokeMock.mockClear();
    await operatorApi.listTierRates("codex");
    expect(invokeMock).toHaveBeenCalledWith("operator_list_tier_rates", {
      app: "codex",
      siteOrigin: null,
    });
  });
});
