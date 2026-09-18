import { invoke } from "@tauri-apps/api/core";
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

export const applicationRoutingApi = {
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
