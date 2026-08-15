/**
 * 自动模式 API：系统按全局策略（价格最低/响应最快）自动挑托管档位。
 * 后端实现见 `src-tauri/src/commands/auto_mode.rs`。
 */
import { invoke } from "@tauri-apps/api/core";

export type AutoModeStrategy = "cheapest" | "fastest";

export interface AutoModeStatus {
  enabled: boolean;
  strategy: AutoModeStrategy;
}

export const autoModeApi = {
  getStatus: (appType: string): Promise<AutoModeStatus> =>
    invoke("get_auto_mode_status", { appType }),

  setEnabled: (appType: string, enabled: boolean): Promise<void> =>
    invoke("set_auto_mode_enabled", { appType, enabled }),

  setStrategy: (strategy: AutoModeStrategy): Promise<void> =>
    invoke("set_auto_mode_strategy", { strategy }),
};
