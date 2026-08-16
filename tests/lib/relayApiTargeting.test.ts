import { describe, it, expect, vi, beforeEach } from "vitest";

/**
 * 闸：**中转站相关的命令必须把 `relayId` 真的传到 invoke 那一层。**
 *
 * ## 这道闸守的是什么（2026-08-04 重写）
 *
 * 曾经这四个参数是 `Option<i64>` / `relayId?: number`，**不传就回落到「当前站」**
 * （`creds::load()` 读 `is_current = 1` 的那一行）。那让「漏传」成为一个**静默错误**：
 * 类型合法、运行时不抛、只是作用到了另一个账号上 —— 用户看到「给 A 点获取密钥，
 * B 的档位变了」，极难归因。前端一度靠 `switchSite(id)` 先改全局当前站再调无参命令，
 * 两个中转站并发操作就互相串目标。
 *
 * **`is_current` 整个概念已删**（见后端 `relay/creds.rs` 模块文档），四个参数
 * 都收成必填 ⇒ 漏传现在是**编译错误**，那半边由 `tsc` 守着，不需要测试。
 *
 * 这道闸剩下的职责是另一半：**参数别在封装层被丢掉或改名**。
 * `invoke` 的第二个参数是个对象字面量，写错键名（`relay_id` vs `relayId`）
 * 或忘了透传，`tsc` 一样过 —— 而后果仍是那条「作用到错的账号」。
 */

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

// 动态 import：mock 必须在模块求值前就位。
const { relayApi } = await import("@/lib/api/relay");

describe("relayApi 的中转站定位参数", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
  });

  it("importSite 走 Manual 来源的保守导入命令（手填域名兜底直连）", async () => {
    // 白名单是展示集合的所有者，但白名单外的站也要能连（2026-08-16 恢复）；
    // 与目录导入分开成两条命令：Manual 不认目录声明的专属入口。
    await relayApi.importSite("https://my-own-relay.example");
    expect(invokeMock).toHaveBeenCalledWith("relay_import_site", {
      site: "https://my-own-relay.example",
    });
  });

  it("importDirectorySite 走后端重新核验签名目录的专用命令", async () => {
    await relayApi.importDirectorySite("https://790053500.com/keys");
    expect(invokeMock).toHaveBeenCalledWith("relay_import_directory_site", {
      site: "https://790053500.com/keys",
    });
  });

  it("不再暴露旧的 sub2api-only probeSite 入口", () => {
    expect("probeSite" in relayApi).toBe(false);
  });

  it("login 把 relayId 透传给后端", async () => {
    await relayApi.login(7, "claude");
    expect(invokeMock).toHaveBeenCalledWith("relay_login", {
      relayId: 7,
      app: "claude",
    });
  });

  it("refresh 把 relayId 与 app 透传给后端", async () => {
    await relayApi.refresh(42, "codex");
    expect(invokeMock).toHaveBeenCalledWith("relay_refresh", {
      relayId: 42,
      app: "codex",
    });
  });

  /**
   * `balance` 传错的后果与 login / provision **不同类，但更难发现**。
   *
   * 那两个是「动作作用到错的账号」，用户迟早看到反常（档位变了）。余额是**纯显示**：
   * 键名写错时每一行都显示同一个数字，界面看起来完全正常。
   */
  it("balance 把 relayId 透传（传错会让每行显示同一个数字）", async () => {
    await relayApi.balance(3);
    expect(invokeMock).toHaveBeenCalledWith("relay_balance", {
      relayId: 3,
    });
  });

  /**
   * ⭐ `purchase` 传错是这组里**后果最严重**的一条：会让钱进错账号。
   *
   * 打开的充值页上显示的是那个账号自己的信息（我们注入的就是它的登录态）⇒
   * 用户不会察觉，钱**真的**充进别的账号。
   */
  it("purchase 把 relayId 透传（传错会让钱充进别的账号）", async () => {
    await relayApi.purchase(5);
    expect(invokeMock).toHaveBeenCalledWith("relay_purchase", {
      relayId: 5,
    });
  });

  /**
   * ⭐ **`0` 必须照原样传下去，不能被当成「没给」。**
   *
   * 这条守的是回落时代残留的写法：`{ relayId: relayId ?? null }` 换成
   * `{ relayId }` 之后语义就对了，但若有人「顺手」写成 `relayId || null`，
   * `0` 会变成 `null` ⇒ Rust 侧 `i64` 参数缺失 ⇒ serde 报 missing field。
   *
   * SQLite 的 `AUTOINCREMENT` 从 1 起，所以 0 不是真实行 id ——
   * 但那是**库的约定，不是这一层的约定**，这一层的职责是原样透传。
   */
  it("relayId = 0 也照原样传（别被 falsy 判据吞掉）", async () => {
    await relayApi.refresh(0, "codex");
    expect(invokeMock).toHaveBeenCalledWith("relay_refresh", {
      relayId: 0,
      app: "codex",
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
    await relayApi.checkSession();
    expect(invokeMock).toHaveBeenCalledWith("relay_check_session");
  });

  it("switchTier 首次调用不替后端决定 ChatGPT 行为", async () => {
    await relayApi.switchTier("provider-1", "codex");
    expect(invokeMock).toHaveBeenCalledWith("relay_switch_tier", {
      providerId: "provider-1",
      app: "codex",
      quitChatgpt: null,
    });
  });

  it("switchTierModel 首次调用同样把确认裁决留给后端", async () => {
    await relayApi.switchTierModel("provider-1", "codex", "gpt-5");
    expect(invokeMock).toHaveBeenCalledWith("relay_switch_tier_model", {
      providerId: "provider-1",
      app: "codex",
      model: "gpt-5",
      quitChatgpt: null,
    });
  });
});
