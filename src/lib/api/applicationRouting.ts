import { invoke } from "@tauri-apps/api/core";
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

export interface ApplicationRouting {
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
  order?: { profileName: string; providerIds: string[] };
  selection?: { providerId: string; model?: string };
}

export const applicationRoutingApi = {
  apply: (
    appType: string,
    change: ApplicationRoutingChange,
    quitChatgpt?: boolean,
  ): Promise<SwitchTierCommandResult> =>
    invoke("apply_application_routing", { appType, change, quitChatgpt }),
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
