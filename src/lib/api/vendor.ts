import { invoke } from "@tauri-apps/api/core";

import type { AppId } from "./types";
import type { CatalogPreview, ProviderCatalogDecision } from "./providers";
import type {
  RefreshResult,
  RowBalanceResult,
  SwitchTierCommandResult,
} from "./relay";

/** Native command input for an explicit connection operation. */
export interface VendorPrepareConnectionInput {
  rowId: number;
  plan: string;
  app: AppId;
  operationId: string;
  catalogDecision: ProviderCatalogDecision | null;
}

/** Only these receipt states have a defined meaning at this boundary. */
type VendorReceiptFields = {
  operationId: string;
  rowId: number;
  plan: string;
  app: AppId;
  providerId: string;
  receiptRevision: number;
  completedConfigurationRevision: string | null;
};
export type VendorConnectionUnknown = {
  status: "unknown";
  operationId?: string;
  app?: AppId;
  receiptRevision?: number;
};
export type VendorPreparedReceipt = VendorReceiptFields & {
  status: "prepared";
  operationState: "prepared";
};
export type VendorCompletedReceipt = VendorReceiptFields & {
  status: "completed";
  operationState: "completed";
};
type VendorUnconfirmedReceipt = VendorReceiptFields & {
  status: "unknown";
  operationState:
    "prepared" | "applying" | "unknown" | "failedBeforeWrite" | "reviewed";
};
export type VendorQueryConnectionResult =
  | VendorConnectionUnknown
  | VendorUnconfirmedReceipt
  | VendorCompletedReceipt
  | { status: "none" };
export type VendorPrepareConnectionResult =
  | VendorPreparedReceipt
  | VendorCompletedReceipt
  | VendorUnconfirmedReceipt
  | VendorConnectionUnknown;

/** A switched response is feedback, not a persistent receipt or proof of client adoption. */
export type VendorSwitchPreparedConnectionResult =
  SwitchTierCommandResult | VendorCompletedReceipt | VendorConnectionUnknown;

export interface VendorSwitchPreparedConnectionInput {
  rowId: number;
  plan: string;
  app: AppId;
  quitChatgpt: boolean | null;
  operationId: string;
  expectedRevision: number;
}

const isNonemptyString = (value: unknown): value is string =>
  typeof value === "string" && value.length > 0;
const isRevision = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > 0;

// Preserve the original text: the native database uses it as the journal key.
const isSupportedUuid = (value: unknown): value is string => {
  if (typeof value !== "string") return false;
  const hex = "[0-9a-fA-F]";
  const hyphenated = `${hex}{8}-${hex}{4}-${hex}{4}-${hex}{4}-${hex}{12}`;
  return new RegExp(
    `^(?:${hex}{32}|${hyphenated}|\\{${hyphenated}\\}|urn:uuid:${hyphenated})$`,
  ).test(value);
};

type VendorReceiptIntent = {
  app: AppId;
  operationId: string | null;
  rowId?: number;
  plan?: string;
};
function decodeVendorConnection(
  value: unknown,
  intent: VendorReceiptIntent,
  kind: "prepare",
): VendorPrepareConnectionResult;
function decodeVendorConnection(
  value: unknown,
  intent: VendorReceiptIntent,
  kind: "query",
): VendorQueryConnectionResult;
function decodeVendorConnection(
  value: unknown,
  intent: VendorReceiptIntent,
  kind: "prepare" | "query",
): VendorPrepareConnectionResult | VendorQueryConnectionResult {
  const unknown: VendorConnectionUnknown = { status: "unknown" };
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return unknown;
  const v = value as Record<string, unknown>;
  if (v.status === "none")
    return kind === "query" &&
      intent.operationId === null &&
      Object.keys(v).length === 1
      ? { status: "none" }
      : unknown;
  if (
    v.operationId !== undefined &&
    (!isSupportedUuid(v.operationId) ||
      (intent.operationId !== null && v.operationId !== intent.operationId))
  )
    return unknown;
  if (v.app !== undefined && v.app !== intent.app) return unknown;

  // Do not drop an unknown field that could carry a confirmation or review gate.
  const receiptKeys = [
    "operationId",
    "rowId",
    "plan",
    "app",
    "providerId",
    "receiptRevision",
    "status",
    "operationState",
    "completedConfigurationRevision",
  ];
  const full =
    Object.keys(v).every((key) => receiptKeys.includes(key)) &&
    isSupportedUuid(v.operationId) &&
    typeof v.rowId === "number" &&
    Number.isSafeInteger(v.rowId) &&
    v.rowId >= 0 &&
    isNonemptyString(v.plan) &&
    v.app === intent.app &&
    isNonemptyString(v.providerId) &&
    isRevision(v.receiptRevision) &&
    (v.completedConfigurationRevision === null ||
      isNonemptyString(v.completedConfigurationRevision));
  if (
    full &&
    (intent.rowId === undefined || v.rowId === intent.rowId) &&
    (intent.plan === undefined || v.plan === intent.plan)
  ) {
    const fields: VendorReceiptFields = {
      operationId: v.operationId as string,
      rowId: v.rowId as number,
      plan: v.plan as string,
      app: intent.app,
      providerId: v.providerId as string,
      receiptRevision: v.receiptRevision as number,
      completedConfigurationRevision: v.completedConfigurationRevision as
        string | null,
    };
    if (
      kind === "prepare" &&
      v.status === "prepared" &&
      v.operationState === "prepared"
    ) {
      return { ...fields, status: "prepared", operationState: "prepared" };
    }
    if (v.status === "completed" && v.operationState === "completed") {
      return { ...fields, status: "completed", operationState: "completed" };
    }
    const pendingState =
      v.operationState === "prepared" ||
      v.operationState === "applying" ||
      v.operationState === "unknown";
    const otherKnownState =
      v.operationState === "failedBeforeWrite" ||
      v.operationState === "reviewed";
    if (
      (pendingState &&
        (kind === "query"
          ? v.status === "unknown"
          : v.status === v.operationState)) ||
      (otherKnownState && v.status === v.operationState)
    ) {
      return {
        ...fields,
        status: "unknown",
        operationState:
          v.operationState as VendorUnconfirmedReceipt["operationState"],
      };
    }
    return unknown;
  }
  // Only the documented redacted unknown shape may retain any identity fields.
  if (
    kind === "query" &&
    v.status === "unknown" &&
    Object.keys(v).every((key) =>
      ["status", "operationId", "app", "receiptRevision"].includes(key),
    ) &&
    (v.receiptRevision === undefined || isRevision(v.receiptRevision))
  ) {
    return {
      status: "unknown",
      ...(v.operationId !== undefined
        ? { operationId: v.operationId as string }
        : {}),
      ...(v.app !== undefined ? { app: intent.app } : {}),
      ...(v.receiptRevision !== undefined
        ? { receiptRevision: v.receiptRevision as number }
        : {}),
    };
  }
  return unknown;
}

function decodeCatalogPreview(value: unknown): CatalogPreview | null {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return null;
  const v = value as Record<string, unknown>;
  const keys = [
    "ownership",
    "choice",
    "canManage",
    "canRestore",
    "revision",
    "beforePointer",
    "afterPointer",
  ];
  if (
    !Object.keys(v).every((key) => keys.includes(key)) ||
    !["unclaimedExternal", "configuredExternal", "managed", "none"].includes(
      v.ownership as string,
    ) ||
    !(v.choice === "keep" || v.choice === "manage") ||
    typeof v.canManage !== "boolean" ||
    !(v.canRestore === undefined || typeof v.canRestore === "boolean") ||
    typeof v.revision !== "string" ||
    !(v.beforePointer === null || typeof v.beforePointer === "string") ||
    !(v.afterPointer === null || typeof v.afterPointer === "string")
  )
    return null;
  return {
    ownership: v.ownership as CatalogPreview["ownership"],
    choice: v.choice,
    canManage: v.canManage,
    ...(v.canRestore !== undefined
      ? { canRestore: v.canRestore as boolean }
      : {}),
    revision: v.revision,
    beforePointer: v.beforePointer,
    afterPointer: v.afterPointer,
  };
}

function decodeVendorSwitch(
  value: unknown,
  intent: VendorSwitchPreparedConnectionInput,
): VendorSwitchPreparedConnectionResult {
  const unknown: VendorConnectionUnknown = { status: "unknown" };
  if (
    !isSupportedUuid(intent.operationId) ||
    !isRevision(intent.expectedRevision) ||
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value)
  )
    return unknown;
  const v = value as Record<string, unknown>;
  if (v.status === "completed") {
    const result = decodeVendorConnection(value, intent, "prepare");
    return result.status === "completed" ? result : unknown;
  }
  if (
    v.status === "unknown" &&
    Object.keys(v).every((key) => ["status", "operationId"].includes(key)) &&
    v.operationId === intent.operationId
  ) {
    return { status: "unknown", operationId: intent.operationId };
  }
  if (
    v.status === "switched" &&
    Object.keys(v).every((key) =>
      [
        "status",
        "providerName",
        "chatgptWasRunning",
        "chatgptRelaunched",
        "warnings",
      ].includes(key),
    ) &&
    typeof v.providerName === "string" &&
    typeof v.chatgptWasRunning === "boolean" &&
    typeof v.chatgptRelaunched === "boolean" &&
    Array.isArray(v.warnings) &&
    v.warnings.every((warning) => typeof warning === "string")
  ) {
    return {
      status: "switched",
      providerName: v.providerName,
      chatgptWasRunning: v.chatgptWasRunning,
      chatgptRelaunched: v.chatgptRelaunched,
      warnings: v.warnings,
    };
  }
  if (
    v.status === "confirmationRequired" &&
    Object.keys(v).every((key) =>
      ["status", "targetName", "quitRequired", "catalog"].includes(key),
    ) &&
    typeof v.targetName === "string" &&
    typeof v.quitRequired === "boolean"
  ) {
    if (v.catalog === undefined)
      return {
        status: "confirmationRequired",
        targetName: v.targetName,
        quitRequired: v.quitRequired,
      };
    const catalog = decodeCatalogPreview(v.catalog);
    if (catalog)
      return {
        status: "confirmationRequired",
        targetName: v.targetName,
        quitRequired: v.quitRequired,
        catalog,
      };
  }
  // Includes requestResolved: callers query the original operation instead of guessing.
  return unknown;
}

export interface VendorOperationIdentity {
  app: AppId;
  operationId: string;
}
export type VendorConnectionReview = VendorOperationIdentity & {
  status: "reviewRequired";
  receiptRevision: number;
  observedRevision: string;
};
export type VendorReviewedReceipt = VendorReceiptFields & {
  status: "reviewed" | "failedBeforeWrite";
  operationState: "reviewed" | "failedBeforeWrite";
};
export interface VendorNewConnectionIntentInput extends VendorOperationIdentity {
  targetRowId: number;
  targetPlan: string;
}
export type VendorNewConnectionIntentReview = VendorNewConnectionIntentInput & {
  status: "newIntentReviewRequired";
  receiptRevision: number;
  originalRowId: number;
  originalPlan: string;
  targetProviderId: string;
  reviewToken: string;
};
export type VendorConnectionCatalogResult =
  | Extract<SwitchTierCommandResult, { status: "confirmationRequired" }>
  | VendorConnectionUnknown;
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const onlyKeys = (value: Record<string, unknown>, keys: string[]) =>
  Object.keys(value).every((key) => keys.includes(key));
const matchesOperation = (
  value: Record<string, unknown>,
  input: VendorOperationIdentity,
) =>
  isSupportedUuid(input.operationId) &&
  value.operationId === input.operationId &&
  value.app === input.app;
const isRowId = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const isDigest = (value: unknown): value is string =>
  typeof value === "string" && /^[a-f0-9]{64}$/.test(value);

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
  authMode?: "personal-key" | "web-session";
  id: number;
  /** 稳定标识（`"deepseek"`）。传给 `openLogin`。 */
  vendorId: string;
  /** 厂商家族展示名（`"DeepSeek"` / `"opencode"`）。**服务端给什么就显示什么**，不翻译。 */
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
  canDelete: boolean;
  /**
   * 这一行展开出的接入变体（plan）。单 plan 厂商恒一个元素（DeepSeek / BigModel），
   * opencode 是 Zen / Go / Free —— 同 app 下互斥可切，形状对齐 relay 的档位。
   *
   * 按 plan 算的事实（providerId / isCurrent / userEdited / can*）全在这里；
   * 行级只剩账号级事实（登录态 / key / 删除资格）。
   */
  plans: VendorPlanInfo[];
}

export interface VendorPlanInfo {
  /** plan 稳定标识（= provider id 哈希里的段）。传给 `vendorApi.switch` 的 `plan`。 */
  planId: string;
  /** 展示名（`opencode Zen` / `opencode Go` / `opencode Free`）。服务端给定，不翻译。 */
  planName: string;
  /**
   * 这个 plan 那六条 provider 记录的 id（六个平台共用一个）。
   *
   * 由后端派生（`sha256(planId + "/" + accountId)`）——
   * **前端算不出来**：DTO 有意不给 accountId，也没有 sha256。
   *
   * ⚠️ **不再用它判「当前在用」** —— 那件事改由 `isCurrent`（后端现算）表达。
   * 它只在「编辑 / 恢复默认」时用（这两条命令吃 providerId）。
   *
   * 空串 = 还没登录过（没有 accountId 就派生不出 id）。
   */
  providerId: string;
  /**
   * **当前 tab 那个 app** 下，这个 plan 是不是正在用的那个。
   *
   * 由后端按 `appId` 现算（判据与中转站档位的 `isCurrent` 同源 —— 都是
   * `providers` 表的 `is_current`）。**前端不自己维护、也不拿它跟别的值比较**。
   * 所以官网档、中转站档位 / 手工 provider 共享同一份互斥：
   * 一个 app 下永远只有一个「在用」。
   */
  isCurrent: boolean;
  /**
   * **当前 tab 那个平台**的这个 plan 配置是不是被用户改过。
   *
   * ⚠️ **按平台 × plan 算** —— 每个 plan 的六条 provider 记录各自能被独立编辑。
   *
   * `null` = 判不了（没 provision 过 / 这个平台不适用）。**`null` 时不显示标记** ——
   * 与 relay 的 `TierInfo.userEdited` 同一条原则：不知道就别断言。
   */
  userEdited: boolean | null;
  /** 当前 tab 下这个 plan 是否已有可编辑的接入配置。 */
  canEditConfig: boolean;
  /** 当前 tab 下这个 plan 是否已有可切换的接入配置。 */
  canSwitch: boolean;
  switchBlockedReason?: "opencodeFreeExternalUnavailable" | null;
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
export const OPENCODE_VENDOR_ID = "opencode";

/** 「官方 API」页的厂商目录（展示名与 Rust 侧 `display_name` 对应）。 */
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
  {
    // 厂商家族名；登录后账号行下按 plan 展开（Zen 按量 / Go 订阅）。
    id: OPENCODE_VENDOR_ID,
    displayName: "opencode",
    descriptionKey: "loongport.officialApi.opencodeDesc",
  },
];

export interface OpenCodeLocalPreview {
  previewId: string;
  entries: { id: string; label: string; isActive: boolean }[];
  notices: string[];
}
export interface OpenCodeKeyImportResult {
  rowId: number;
  configurationReady: boolean;
}

export const vendorApi = {
  previewOpenCodeLocal: (consent: boolean): Promise<OpenCodeLocalPreview> =>
    invoke("vendor_preview_opencode_local", { consent }),
  importOpenCodeLocal: (
    previewId: string,
    entryId: string,
    consent: boolean,
  ): Promise<OpenCodeKeyImportResult> =>
    invoke("vendor_import_opencode_local", { previewId, entryId, consent }),
  cancelOpenCodeLocal: (previewId: string): Promise<void> =>
    invoke("vendor_cancel_opencode_local", { previewId }),
  importOpenCodeKey: (
    apiKey: string,
    label: string,
  ): Promise<OpenCodeKeyImportResult> =>
    invoke("vendor_import_opencode_key", { apiKey, label }),
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
    invoke("vendor_reset_plan_config", { providerId, appId }),

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

  /** Preview does not mutate the operation or authorize a later write by itself. */
  previewConnectionCatalog: async (
    input: VendorOperationIdentity & {
      catalogDecision: ProviderCatalogDecision;
      quitChatgpt: boolean | null;
    },
  ): Promise<VendorConnectionCatalogResult> => {
    const { app, operationId, catalogDecision, quitChatgpt } = input;
    if (app !== "codex" || !isSupportedUuid(operationId))
      return { status: "unknown" };
    const value: unknown = await invoke("vendor_preview_connection_catalog", {
      app,
      operationId,
      catalogDecision,
      quitChatgpt,
    });
    const result = decodeVendorSwitch(value, {
      app,
      operationId,
      rowId: 0,
      plan: "preview",
      expectedRevision: 1,
      quitChatgpt,
    });
    return result.status === "confirmationRequired"
      ? result
      : { status: "unknown" };
  },
  previewConnectionReview: async (
    input: VendorOperationIdentity,
  ): Promise<VendorConnectionReview | VendorConnectionUnknown> => {
    const { app, operationId } = input;
    const value: unknown = await invoke("vendor_preview_connection_review", {
      app,
      operationId,
    });
    if (
      !isRecord(value) ||
      !matchesOperation(value, input) ||
      value.status !== "reviewRequired" ||
      !onlyKeys(value, [
        "status",
        "operationId",
        "app",
        "receiptRevision",
        "observedRevision",
      ]) ||
      !isRevision(value.receiptRevision) ||
      !isNonemptyString(value.observedRevision)
    )
      return { status: "unknown" };
    return {
      status: "reviewRequired",
      app,
      operationId,
      receiptRevision: value.receiptRevision,
      observedRevision: value.observedRevision,
    };
  },
  reviewConnection: async (
    input: VendorOperationIdentity & {
      expectedRevision: number;
      observedRevision: string;
    },
  ): Promise<VendorReviewedReceipt | VendorConnectionUnknown> => {
    const { app, operationId, expectedRevision, observedRevision } = input;
    const value: unknown = await invoke("vendor_review_connection", {
      app,
      operationId,
      expectedRevision,
      observedRevision,
    });
    const result = decodeVendorConnection(value, input, "query");
    if (
      result.status === "unknown" &&
      "operationState" in result &&
      (result.operationState === "reviewed" ||
        result.operationState === "failedBeforeWrite") &&
      result.receiptRevision > expectedRevision
    ) {
      return {
        ...result,
        status: result.operationState,
        operationState: result.operationState,
      };
    }
    return { status: "unknown" };
  },
  previewNewConnectionIntent: async (
    input: VendorNewConnectionIntentInput,
  ): Promise<VendorNewConnectionIntentReview | VendorConnectionUnknown> => {
    const { app, operationId, targetRowId, targetPlan } = input;
    const value: unknown = await invoke(
      "vendor_preview_new_connection_intent",
      { app, operationId, targetRowId, targetPlan },
    );
    if (
      !isRecord(value) ||
      !matchesOperation(value, input) ||
      value.status !== "newIntentReviewRequired" ||
      !onlyKeys(value, [
        "status",
        "operationId",
        "app",
        "receiptRevision",
        "originalRowId",
        "originalPlan",
        "targetRowId",
        "targetPlan",
        "targetProviderId",
        "reviewToken",
      ]) ||
      !isRevision(value.receiptRevision) ||
      !isRowId(value.originalRowId) ||
      !isNonemptyString(value.originalPlan) ||
      !isRowId(targetRowId) ||
      value.targetRowId !== targetRowId ||
      !isNonemptyString(targetPlan) ||
      value.targetPlan !== targetPlan ||
      !isNonemptyString(value.targetProviderId) ||
      !isDigest(value.reviewToken)
    )
      return { status: "unknown" };
    return {
      status: "newIntentReviewRequired",
      app,
      operationId,
      receiptRevision: value.receiptRevision,
      originalRowId: value.originalRowId,
      originalPlan: value.originalPlan,
      targetRowId,
      targetPlan,
      targetProviderId: value.targetProviderId,
      reviewToken: value.reviewToken,
    };
  },
  /** Retires a reviewed old intent only. A new UUID must still be explicitly prepared. */
  confirmNewConnectionIntent: async (
    input: VendorNewConnectionIntentInput & { reviewToken: string },
  ): Promise<
    { status: "newIntentPermitted"; app: AppId } | VendorConnectionUnknown
  > => {
    const { app, operationId, targetRowId, targetPlan, reviewToken } = input;
    const value: unknown = await invoke(
      "vendor_confirm_new_connection_intent",
      { app, operationId, targetRowId, targetPlan, reviewToken },
    );
    return isSupportedUuid(operationId) &&
      isDigest(reviewToken) &&
      isRecord(value) &&
      value.status === "newIntentPermitted" &&
      value.app === app &&
      onlyKeys(value, ["status", "app"])
      ? { status: "newIntentPermitted", app }
      : { status: "unknown" };
  },

  /** Prepare one explicit operation; undecodable replies remain uncertain. */
  prepareConnection: async (
    input: VendorPrepareConnectionInput,
  ): Promise<VendorPrepareConnectionResult> => {
    const { rowId, plan, app, operationId, catalogDecision } = input;
    const result: unknown = await invoke("vendor_prepare_connection", {
      rowId,
      plan,
      app,
      operationId,
      catalogDecision,
    });
    return decodeVendorConnection(result, input, "prepare");
  },

  /** Read an original operation, or the native app-scoped result for null. */
  queryConnection: async (
    app: AppId,
    operationId: string | null,
  ): Promise<VendorQueryConnectionResult> => {
    const result: unknown = await invoke("vendor_query_connection", {
      app,
      operationId,
    });
    return decodeVendorConnection(result, { app, operationId }, "query");
  },

  /** Unrecognized response is uncertain: callers must query the same operation. */
  switchPreparedConnection: async (
    input: VendorSwitchPreparedConnectionInput,
  ): Promise<VendorSwitchPreparedConnectionResult> => {
    const { rowId, plan, app, quitChatgpt, operationId, expectedRevision } =
      input;
    const result: unknown = await invoke("vendor_switch", {
      rowId,
      plan,
      app,
      quitChatgpt,
      operationId,
      expectedRevision,
    });
    return decodeVendorSwitch(result, input);
  },

  switch: (
    rowId: number,
    /** 要切到的 plan（`plans[].planId`；单 plan 厂商也显式传）。 */
    plan: string,
    app: AppId,
    quitChatgpt?: boolean,
  ): Promise<SwitchTierCommandResult> =>
    invoke("vendor_switch", {
      rowId,
      plan,
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
