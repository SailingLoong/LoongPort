/**
 * 自动模式 API：系统按全局策略（价格最低/响应最快）自动挑托管档位。
 * 后端实现见 `src-tauri/src/commands/auto_mode.rs`。
 */
import { invoke } from "@tauri-apps/api/core";

export type AutoModeStrategy = "cheapest" | "fastest";
/** 省心选路模式：自动按策略 / 用户手动定序。 */
export type EasyModeMode = "auto" | "manual";

export interface AutoModeStatus {
  /** 按 app 的开关。 */
  enabled: boolean;
  /** 全局策略（所有 app 共享一份）。 */
  strategy: AutoModeStrategy;
  /** 模型偏好；`null` = 不限。 */
  model: string | null;
  /** 可选模型清单（该 app 全部托管档位模型目录的并集；空 = 没有目录）。 */
  availableModels: string[];
  /** 有没有可用的托管档位（与开启判据同源）；总开关只对 true 的 app 生效。 */
  hasCandidates: boolean;
  /** 该 CLI 的配置文件是否存在（= CLI 装过/初始化过）；接管必依赖它。 */
  cliInstalled: boolean;
}

/** 档位看板的一行：展示序即选路优先级序（后端唯源，前端只渲染）。 */
export interface TierBoardTier {
  providerId: string;
  name: string;
  position: number;
  isCurrent: boolean;
  /** `null` = 倍率未知。 */
  rateMultiplier: number | null;
  /** 每百万 token 输入+输出之和（美元）；`null` = 价格未知。 */
  unitPricePerMillion: number | null;
  effectiveModel: string | null;
  avgFirstTokenMs: number | null;
  /** 站点钱包余额（美元）；`null` = 该站不可查。 */
  balanceUsd: number | null;
  /** 模型验真合并判定，只上异常（`'anomaly' | 'suspicious'`）；`null` = 无异常（不背书）。 */
  verificationVerdict: "anomaly" | "suspicious" | null;
}

export interface TierBoard {
  mode: EasyModeMode;
  strategy: AutoModeStrategy;
  model: string | null;
  availableModels: string[];
  currentProviderId: string | null;
  tiers: TierBoardTier[];
}

export const autoModeApi = {
  getStatus: (appType: string): Promise<AutoModeStatus> =>
    invoke("get_auto_mode_status", { appType }),

  setEnabled: (appType: string, enabled: boolean): Promise<void> =>
    invoke("set_auto_mode_enabled", { appType, enabled }),

  setStrategy: (strategy: AutoModeStrategy): Promise<void> =>
    invoke("set_auto_mode_strategy", { strategy }),

  /** 设模型偏好（`null` = 不限）。后端会绕过会话亲和立即切到最优档位。 */
  setModel: (appType: string, model: string | null): Promise<void> =>
    invoke("set_auto_mode_model", { appType, model }),

  /** 档位看板（首页省心视图数据源，一次拉全展示事实）。 */
  getTierBoard: (appType: string): Promise<TierBoard> =>
    invoke("easy_mode_tier_board", { appType }),

  /** 切选路模式；后端在首次切手动时快照当前序为初始清单。 */
  setEasyModeMode: (appType: string, mode: EasyModeMode): Promise<void> =>
    invoke("set_easy_mode_mode", { appType, mode }),

  /** 写手动档位顺序（拖拽落定后整份提交）。 */
  setEasyModeManualOrder: (
    appType: string,
    orderedIds: string[],
  ): Promise<void> =>
    invoke("set_easy_mode_manual_order", { appType, orderedIds }),
};
