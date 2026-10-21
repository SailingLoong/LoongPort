import { isProxyAppId } from "@/config/appConfig";
import { invoke } from "@tauri-apps/api/core";
import type { ProviderCatalogDecision } from "./providers";
import type { SwitchTierCommandResult } from "./relay";
import type { TierBoardModelOption, TierBoardTier } from "./autoMode";

export interface ApplicationRoutingTier extends TierBoardTier {
  /** Backend routing exclusion; a recorded error alone is not an exclusion. */
  skipReason: string | null;
  /** Failed / observed forwarding attempts over seven days; no historical backfill, null without samples. */
  errorRate: number | null;
  /** Tier capability, independent of current selection, model, circuit, toggle and position. */
  canFailover: boolean;
  /** 模型验证资格（后端唯源：app 类型支持 ∧ LoongPort 托管档位）。false = 行内不出验证入口。 */
  canVerifyModels: boolean;
  /** 档位模型目录（provision 嗅探落库）。模型筛选按它命中「分组支持」；
   * 空 = 无目录（非 Codex 系/未嗅探），筛选回落单模型（effectiveModel）语义。 */
  models: string[];
  /** 订阅限额的重置窗口（非订阅档位为空数组）。与账号详情的窗口表同源。 */
  subscriptionWindows: SubscriptionWindow[];
  /** 所有窗口里最早的重置时刻（epoch 秒）——重置列的排序键；无可算窗口为 null。 */
  nextResetAt: number | null;
}

/** 一条订阅限额的时间窗（后端 `relay/tier_windows.rs` 的同形投影）。 */
export interface SubscriptionWindow {
  kind: "fiveHour" | "daily" | "weekly" | "monthly";
  limitUsd: number;
  usedUsd: number | null;
  resetAt: number | null;
}

/** Read-only facts from the existing mode owner; not a serialized operation journal. */
export interface ApplicationModeState {
  status: "ready" | "pending" | "unknown";
  mode: "direct" | "proxy" | null;
  attached: boolean | null;
  currentProviderId: string | null;
  directProviderId: string | null;
  publicationStarted: boolean | null;
  canWrite: boolean;
  writeProtocol: "legacy" | "native" | null;
  blockedReason: string | null;
  canRecheck: boolean;
  legacyCommonConfigWritable?: boolean;
}

/** Missing ownership facts fail closed only for the four managed applications. */
export function isApplicationRoutingWriteBlocked(
  appType: string,
  state: ApplicationModeState | null | undefined,
): boolean {
  if (!isProxyAppId(appType)) return state?.canWrite === false;
  return (
    state?.status !== "ready" ||
    state.canWrite !== true ||
    state.blockedReason != null ||
    (state.writeProtocol !== "native" && state.writeProtocol !== "legacy")
  );
}

export function isApplicationCommonConfigWritable(
  appType: string,
  state: ApplicationModeState | null | undefined,
): boolean {
  return (
    !isApplicationRoutingWriteBlocked(appType, state) &&
    (state?.writeProtocol === "legacy" ||
      state?.legacyCommonConfigWritable === true)
  );
}

export interface ApplicationRouting {
  modeState?: ApplicationModeState | null;
  autoFailoverEnabled: boolean;
  /** This application is currently served by a running proxy with takeover. */
  routingActive: boolean;
  model: string | null;
  modelOptions: TierBoardModelOption[];
  /**
   * 故障切换链的原始 id 序（后端唯源；可能含上游已删除的幽灵）。
   * 「应用此顺序」的差异比较以它为参照；未初始化时后端已回落全量显示序。
   */
  chainIds: string[];
  tiers: ApplicationRoutingTier[];
}

export interface ApplicationRoutingChange {
  catalogDecision?: ProviderCatalogDecision;
  catalogPreviewOnly?: boolean;
  order?: { profileName: string; providerIds: string[] };
  selection?: { providerId: string; model?: string };
}

export interface ApplicationRoutingRequest {
  id: string;
  scope: "applicationRouting";
  draftDigest: string;
  revision: string;
}
/** A short-lived review, never a durable save receipt. */
export interface ApplicationRoutingLegacyReview {
  draftDigest: string;
  revision: string;
}
export type ApplicationRoutingPreparedRequest =
  | { protocol: "native"; request: ApplicationRoutingRequest }
  | { protocol: "legacy"; review: ApplicationRoutingLegacyReview };

export interface ApplicationRoutingRequestResult {
  request: ApplicationRoutingRequest;
  status:
    | "completed"
    | "pending"
    | "verificationRequired"
    | "discarded"
    | "abandoned"
    | "notRecorded"
    | "unknown"
    | "conflict";
}
export type ApplicationRoutingCommandResult =
  | SwitchTierCommandResult
  | { status: "requestResolved"; result: ApplicationRoutingRequestResult };

const prepareRoutingRequest = async (
  appType: string,
  change: ApplicationRoutingChange,
  requestId: string,
): Promise<ApplicationRoutingPreparedRequest> => {
  const prepared = await invoke<ApplicationRoutingPreparedRequest>(
    "prepare_application_routing_request",
    { appType, change, requestId },
  );
  const credential =
    prepared?.protocol === "native"
      ? prepared.request
      : prepared?.protocol === "legacy"
        ? prepared.review
        : undefined;
  if (
    !credential ||
    typeof credential.draftDigest !== "string" ||
    typeof credential.revision !== "string" ||
    (prepared.protocol === "native" &&
      (typeof prepared.request.id !== "string" ||
        prepared.request.scope !== "applicationRouting"))
  )
    throw new Error("mode.invalid_save_request");
  return prepared;
};

function applyRouting(
  appType: string,
  change: ApplicationRoutingChange,
  quitChatgpt?: boolean,
): Promise<SwitchTierCommandResult>;
function applyRouting(
  appType: string,
  change: ApplicationRoutingChange,
  quitChatgpt: boolean | undefined,
  request: ApplicationRoutingRequest | ApplicationRoutingPreparedRequest,
): Promise<ApplicationRoutingCommandResult>;
function applyRouting(
  appType: string,
  change: ApplicationRoutingChange,
  quitChatgpt: boolean | undefined,
  request: undefined,
  legacyReview: ApplicationRoutingLegacyReview,
): Promise<SwitchTierCommandResult>;
async function applyRouting(
  appType: string,
  change: ApplicationRoutingChange,
  quitChatgpt?: boolean,
  credential?: ApplicationRoutingRequest | ApplicationRoutingPreparedRequest,
  legacyReview?: ApplicationRoutingLegacyReview,
): Promise<ApplicationRoutingCommandResult> {
  if (credential && legacyReview) throw new Error("mode.invalid_save_request");
  const implicit = !credential && !legacyReview && !change.catalogPreviewOnly;
  // Old consumers already have an explicit confirmation. Prepare that exact
  // commit once; never prepare a preview or replace an existing reviewed token.
  if (implicit)
    credential = await prepareRoutingRequest(
      appType,
      change,
      crypto.randomUUID(),
    );
  let request: ApplicationRoutingRequest | undefined;
  if (credential) {
    if ("protocol" in credential) {
      if (credential.protocol === "native") request = credential.request;
      else if (credential.protocol === "legacy")
        legacyReview = credential.review;
      else throw new Error("mode.invalid_save_request");
    } else request = credential;
  }
  if (!change.catalogPreviewOnly && !request && !legacyReview)
    throw new Error("mode.invalid_save_request");
  const result = await invoke<ApplicationRoutingCommandResult>(
    "apply_application_routing",
    {
      appType,
      change,
      quitChatgpt,
      ...(request ? { request } : {}),
      ...(legacyReview ? { legacyReview } : {}),
    },
  );
  if (implicit && result.status === "requestResolved") {
    if (
      !request ||
      result.result.request.id !== request.id ||
      result.result.request.draftDigest !== request.draftDigest ||
      result.result.request.revision !== request.revision ||
      result.result.request.scope !== request.scope ||
      result.result.status !== "completed"
    )
      throw new Error("mode.verification_required");
    return {
      status: "switched",
      providerName: "",
      warnings: [],
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    };
  }
  return result;
}
export const applicationRoutingApi = {
  apply: applyRouting,
  prepareRequest: prepareRoutingRequest,
  queryRequest: (
    appType: string,
    request: ApplicationRoutingRequest,
  ): Promise<ApplicationRoutingRequestResult> =>
    invoke("get_application_routing_request_result", { appType, request }),
  get: (appType: string): Promise<ApplicationRouting> =>
    invoke("get_application_routing", { appType }),
  setOrder: (appType: string, orderedIds: string[]): Promise<void> =>
    invoke("set_application_priority", { appType, orderedIds }),
  setTierBlocked: (
    appType: string,
    providerId: string,
    blocked: boolean,
  ): Promise<void> =>
    invoke("set_application_tier_blocked", { appType, providerId, blocked }),
  setFailover: (appType: string, enabled: boolean): Promise<void> =>
    invoke("set_application_failover", { appType, enabled }),
};
