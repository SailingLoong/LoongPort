import { invoke } from "@tauri-apps/api/core";

import type { AppId } from "./types";
import type {
  RefreshResult,
  RowBalanceResult,
  SwitchTierCommandResult,
} from "./relay";

/**
 * 一个已添加的官网直连账号（`loongport_vendor` 的一行）。
 *
 * **只读本地、不发网络**（与 `listRelays` 同一条契约）—— 首屏不卡在网络上。
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
  status:
    | "notLoggedIn"
    | "sessionExpired"
    | "sessionExpiredUsable"
    | "ready"
    | "noKey";
  canQueryBalance: boolean;
  canRefresh: boolean;
  canEditConfig: boolean;
  canSwitch: boolean;
  canDelete: boolean;
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
   * 与 relay 的 `TierInfo.userEdited` 同一条原则：不知道就别断言。
   *
   * 后端不存这个标记，靠与默认配置整份比对现算 ⇒ 用户把配置改回默认，标记会自动消失。
   */
  userEdited: boolean | null;
}

export interface VendorAccountList {
  supported: boolean;
  accounts: VendorAccountRow[];
}

export interface VendorActionError {
  kind?: "key_limit";
  message: string;
  helpUrl?: string;
}

export interface VendorLoginResult {
  rowId: number;
  refresh: RefreshResult;
}

/**
 * 官网厂商的稳定标识。
 *
 * ⚠️ 与 Rust 侧 `Vendor::vendor_id()` 必须一致 —— 它是**稳定标识**
 * （进数据库、进 provider id 的哈希），后端认不出会直接报「不认识的厂商」。
 */
export const DEEPSEEK_VENDOR_ID = "deepseek";
export const BIGMODEL_VENDOR_ID = "bigmodel";

/** 「官方 API」页的厂商目录（展示名/说明与 Rust 侧 `display_name` 对应）。 */
export const VENDOR_CATALOG: ReadonlyArray<{
  id: string;
  displayName: string;
  descriptionKey: string;
}> = [
  {
    id: DEEPSEEK_VENDOR_ID,
    displayName: "DeepSeek",
    descriptionKey: "loongport.officialApi.deepseekDesc",
  },
  {
    id: BIGMODEL_VENDOR_ID,
    displayName: "智谱 BigModel",
    descriptionKey: "loongport.officialApi.bigmodelDesc",
  },
];

export const vendorApi = {
  /**
   * 列出已添加的官网账号。
   *
   * 后端同时返回当前平台是否支持官网账号，以及可直接展示的账号行。
   */
  list: (appId: AppId): Promise<VendorAccountList> =>
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
   * 返回保存后的行 id；`null` = 用户关窗或超时（**都不是错误**，别为它弹 toast）。
   *
   * 预填值由后端取「这个厂商下最近登录过的那个标识」，前端不必传。
   */
  openLogin: (
    vendorId: string,
    app: AppId,
  ): Promise<VendorLoginResult | null> =>
    invoke("vendor_open_login", { vendorId, app }),

  /**
   * 查一行的余额。**与 `relayApi.balance` 同一契约**（上游那个 `UsageResult`）。
   *
   * 曾经这条回后端拼好的字符串 `"¥547.08"`、中转站那条回 `{balance, frozenBalance}`
   * 数字 —— 同一个事实两套形状，前端因此长出两份 state、两个 effect、两处渲染。
   * 统一之后两类行共用一个 hook（`useRowBalanceQuery`）与一个呈现件（`InlineUsage`）。
   * 币种现在在 `unit` 字段里，由呈现件显示。
   *
   * 失败**不 reject**：查不到时回 `success:false`，用量条渲染失败态并留住刷新按钮。
   *
   * ⚠️ **登录态过期也查得到**：后端优先用 sk 查（`services::balance` 认得
   * `api.deepseek.com`），网页登录态只是兜底。别在调用方按 `sessionExpired` 关掉它。
   */
  balance: (rowId: number): Promise<RowBalanceResult> =>
    invoke("vendor_balance", { rowId }),

  refresh: (rowId: number, app: AppId): Promise<RefreshResult> =>
    invoke("vendor_refresh", { rowId, app }),

  switch: (
    rowId: number,
    app: AppId,
    quitChatgpt?: boolean,
  ): Promise<SwitchTierCommandResult> =>
    invoke("vendor_switch", {
      rowId,
      app,
      quitChatgpt: quitChatgpt ?? null,
    }),

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
