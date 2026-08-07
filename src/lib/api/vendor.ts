import { invoke } from "@tauri-apps/api/core";

import type { AppId } from "./types";

/**
 * 一个已添加的官网直连账号（`loongport_vendor` 的一行）。
 *
 * **只读本地、不发网络**（与 `listOperators` 同一条契约）—— 首屏不卡在网络上。
 * 余额走 `vendorApi.balance`，由前端渲染完再异步填。
 *
 * ⚠️ **不含 `authToken` 与 `apiKey`** —— 凭据不出 Rust 侧。给前端明文 sk 只会让
 * 那把 key 出现在 devtools 的网络面板与前端状态里，而前端并不需要它。
 */
export interface VendorAccountRow {
  id: number;
  /** 稳定标识（`"deepseek"`）。传给 `openLogin`。 */
  vendorId: string;
  /** 厂商展示名（`"DeepSeek"`）。**服务端给什么就显示什么**，不翻译。 */
  vendorName: string;
  /** 给人看的账号名（手机号，空则回落 account_id）。 */
  accountLabel: string;
  /** 有可用登录态。 */
  loggedIn: boolean;
  /**
   * **登录过、但登录态已经不能用了** —— 据此提示「登录已过期，请重新登录」，
   * 而不是像从没登录过一样只摆一个「登录」按钮（那是两种处境）。
   */
  sessionExpired: boolean;
  /**
   * **本地已有这个账号的 sk**。
   *
   * ⚠️ 判据是「sk 非空」，**不是「六个平台的 provider 记录都就绪」** ——
   * 别用它断言配置完整（后端字段文档写了完整理由）。展开六条记录时中途失败会让
   * `provision` 整条返回错误，而这一行确实已经有 sk 了；补救手段是行内那个「刷新」。
   *
   * ⚠️ **与 `loggedIn` 独立**：登录态过期时它仍可以是 `true`，那种情况下用户的
   * CLI **照样能用**（sk 是厂商侧的独立凭据，网页登录态过期不影响它）——
   * 所以那种行不该被催去重新登录。
   */
  keyReady: boolean;
  /**
   * 这一行名下那六条 provider 记录的 id（六个平台共用一个）。
   *
   * 由后端派生（`sha256(vendorId + "/" + accountId)`）——
   * **前端算不出来**：DTO 有意不给 accountId，也没有 sha256。
   *
   * ⚠️ **不再用它判「当前在用」** —— 那件事改由下面的 `isCurrent`（后端现算）
   * 表达。它只在「编辑 / 恢复默认 / 切换」时用（这几条命令吃 providerId）。
   *
   * 空串 = 还没登录过（没有 accountId 就派生不出 id）。
   */
  providerId: string;
  /**
   * **当前 tab 那个 app** 下，这一行是不是正在用的那个。
   *
   * 由后端按 `appId` 现算（判据与中转站档位的 `isCurrent` 同源 —— 都是
   * `providers` 表的 `is_current`）。**前端不自己维护、也不拿它跟别的值比较**。
   * 所以 DeepSeek 官网组与中转站档位 / 手工 provider 共享同一份互斥：
   * 一个 app 下永远只有一个「在用」。
   */
  isCurrent: boolean;
  /**
   * **当前 tab 那个平台**的配置是不是被用户改过（`vendorApi.list` 的 `appId`）。
   *
   * ⚠️ **按平台算，不是整行一个值** —— 一行背后六条 provider 记录各自能被独立编辑。
   *
   * `null` = 判不了（没 provision 过 / 这个平台不适用）。**`null` 时不显示标记** ——
   * 与 operator 的 `TierInfo.userEdited` 同一条原则：不知道就别断言。
   *
   * 后端不存这个标记，靠与默认配置整份比对现算 ⇒ 用户把配置改回默认，标记会自动消失。
   */
  userEdited: boolean | null;
}

/** `vendorApi.provision` 的结果。 */
export interface VendorProvisionSummary {
  /** 这个账号的 provider id。**六个平台共用一个**（不含 app_type）。 */
  providerId: string;
  /** 实际写成功的平台（kebab-case 的 app_type）。 */
  platforms: string[];
  /**
   * 这一轮有没有真的去官网建了一把新 key。
   *
   * `false` = 本地已有明文，零请求就完事（**这是正常路径**）。
   * ⚠️ 只在 `true` 时提示「已在官网新建密钥」—— 每次刷新都提示会让用户以为在重复建 key。
   */
  keyCreated: boolean;
}

/**
 * 登录窗凭据回传解析失败的事件名。**与 Rust 侧
 * `commands::vendor::LOGIN_ERROR_EVENT` 必须逐字一致**，payload 是错误字符串。
 *
 * ⚠️ 与 operator 那条 `"operator-login-error"` **有意不同**（两条链路的登录窗各自独立，
 * 混用会让一边的错误弹在另一边的界面上）。定在这一层的理由同 `PURCHASE_CLOSED_EVENT`：
 * 对不上的后果完全静默 —— 编译过、测试绿、只是登录失败永远没有提示。
 */
export const VENDOR_LOGIN_ERROR_EVENT = "vendor-login-error";

/**
 * 官网行会出现在哪些 tab。
 *
 * ⚠️ **`gemini` 与 `grokbuild` 不在里面** —— 上游没有 DeepSeek preset，协议不兼容
 * （Rust 侧 `vendor::provision::DEEPSEEK_APPS` 就是这六个，多写一个这边也生不出记录）。
 *
 * 过滤放在前端是因为 `vendor_list_accounts` **有意不吃 app 参数**：一个官网账号一把 sk
 * 展开到全部平台，「这一行在哪些 tab 出现」是纯展示判断，不该让后端为它多一条查询。
 */
export const VENDOR_APPS: readonly AppId[] = [
  "codex",
  "claude",
  "claude-desktop",
  "hermes",
  "openclaw",
  "opencode",
];

export function vendorSupportsApp(appId: AppId): boolean {
  return VENDOR_APPS.includes(appId);
}

/**
 * 目前唯一支持的厂商。
 *
 * ⚠️ 与 Rust 侧 `Vendor::DeepSeek.vendor_id()` 必须一致 —— 它是**稳定标识**
 * （进数据库、进 provider id 的哈希），后端认不出会直接报「不认识的厂商」。
 *
 * 摆成常量而不是把 `"deepseek"` 散在组件里：加第二家厂商时这里会变成一个选择器，
 * 而散落的字面量得逐个找出来。
 */
export const DEEPSEEK_VENDOR_ID = "deepseek";

/**
 * DeepSeek 的 key 管理页。**超上限（官网 100 把）时引导用户去这里删**——
 * 指路而不是只说不允许。
 *
 * Rust 侧有一份等价常量（`vendor/deepseek.rs` 的 `API_KEYS_URL`，注释里就写着
 * 消费者是这个 UI）。跨语言没法共享常量 —— 但**这一处不加一致性闸**：
 * 它只是个「点开去看看」的引导链接，两边分叉的后果是用户跳到一个稍旧的页面，
 * 不是静默失效（与 deeplink scheme / 事件名那类不同，那些对不上功能会悄悄消失）。
 */
export const DEEPSEEK_API_KEYS_URL = "https://platform.deepseek.com/api_keys";

export const vendorApi = {
  /**
   * 列出已添加的官网账号。
   *
   * `appId` **只用来算 `userEdited`**（一行背后六条 provider 记录，「改过没有」
   * 必须按平台问）。**不是用它过滤行** —— 官网账号在 `VENDOR_APPS` 那六个 tab
   * 都出现，那个判断仍由前端 `vendorSupportsApp` 做。
   */
  list: (appId: AppId): Promise<VendorAccountRow[]> =>
    invoke("vendor_list_accounts", { appId }),

  /**
   * 把**一个平台**的配置恢复成 LoongPort 的默认值。**密钥保留不变。**
   *
   * 只动 `appId` 那一条 —— 一行背后六条记录，用户点的是当前 tab 那个平台的恢复
   * （`userEdited` 也是按平台算的）。一次恢复六条会把他在别的 tab 里的编辑一起
   * 冲掉，而界面上没有任何地方告诉过他这一点。
   */
  resetTierConfig: (providerId: string, appId: AppId): Promise<void> =>
    invoke("vendor_reset_tier_config", { providerId, appId }),

  /**
   * 开登录窗，等凭据回来，存成一行账号。
   *
   * 返回 `true` = 拿到凭据并已入库；`false` = 用户关窗或超时（**都不是错误**，
   * 别为它弹 toast —— 用户知道自己关了窗）。
   *
   * 预填值由后端取「这个厂商下最近登录过的那个标识」，前端不必传。
   */
  openLogin: (vendorId: string): Promise<boolean> =>
    invoke("vendor_open_login", { vendorId }),

  /**
   * 备好这个账号的密钥并展开成六个平台的 provider 记录。
   *
   * **本地已有明文时零请求**（`keyCreated: false`，这是正常路径）；没有才去官网
   * 「删旧建新」。所以它同时是「获取密钥」与「刷新」两个入口的实现。
   */
  provision: (rowId: number): Promise<VendorProvisionSummary> =>
    invoke("vendor_provision", { rowId }),

  /**
   * 查一行的余额。
   *
   * 返回的是**已格式化的字符串**（`"¥547.08"`，币种符号在里面）——
   * ⚠️ 前端**不做任何数值转换或格式化**，直接渲染。`null` = 拿不到
   * （没有钱包 / 金额解不动），**不是 0**。
   *
   * 失败要 catch 掉并把那一行留空，不要弹 toast —— 余额是附加信息
   * （与 operator 侧同一条纪律）。
   */
  balance: (rowId: number): Promise<string | null> =>
    invoke("vendor_balance", { rowId }),

  /** 删一行，连带清掉它名下六个平台的 provider 记录。 */
  remove: (rowId: number): Promise<void> => invoke("vendor_remove", { rowId }),

  /**
   * 保存官网账号行的手工顺序。`ids` 是拖动后的完整顺序，下标即 sort_index。
   *
   * ⚠️ **只传官网行的 id** —— 两类行不可跨类拖动，各自的 `sort_index` 存在
   * 自己的表里，本来就没有一个共同的序。
   */
  reorder: (ids: number[]): Promise<void> => invoke("vendor_reorder", { ids }),
};
