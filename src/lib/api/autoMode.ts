/**
 * 自动模式 API：系统按全局策略（价格最低/响应最快）自动挑托管档位。
 * 后端实现见 `src-tauri/src/commands/auto_mode.rs`。
 */
import { invoke } from "@tauri-apps/api/core";

export type AutoModeStrategy = "cheapest" | "fastest";

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
};
