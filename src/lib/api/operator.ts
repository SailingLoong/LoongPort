import { invoke } from "@tauri-apps/api/core";

/** 当前运营商状态，决定首启显示哪一屏。 */
export interface OperatorStatus {
  /** 域名输入框的底纹词。 */
  defaultSite: string;
  /** 已选定的站点；null 表示还没选过（该弹域名输入框）。 */
  siteOrigin: string | null;
  siteName: string | null;
  /** 是否已有可用凭据。 */
  loggedIn: boolean;
  /** 已备好密钥的档位数。 */
  tierCount: number;
  /** ChatGPT 桌面版装了没有。没装则切换时不做退出/重开。 */
  chatgptInstalled: boolean;
}

export interface ProbeResult {
  siteOrigin: string;
  siteName: string;
  apiBaseUrl: string;
  registrationEnabled: boolean;
}

export interface TierInfo {
  providerId: string;
  /** 只有刚 provision 过才有值；列表命令返回 null（倍率不在本地存）。 */
  groupId: number | null;
  groupName: string;
  displayName: string;
  /** 计费倍率，越小越便宜。null = 未知，不要当 0 显示。 */
  rateMultiplier: number | null;
  isCurrent: boolean;
}

export interface ProvisionSummary {
  tiers: TierInfo[];
  /** 失败的分组。**不为空也不代表整体失败** —— 成功的那些照样能用。 */
  failures: Array<{ groupName: string; reason: string }>;
  /** 这次新建了几把密钥（其余是复用已有的）。 */
  keysCreated: number;
}

export interface SwitchTierResult {
  providerName: string;
  /** ChatGPT 退出前是不是在跑（决定切换后要不要替用户重开）。 */
  chatgptWasRunning: boolean;
  chatgptRelaunched: boolean;
  /**
   * 非致命问题（如重开失败）。
   *
   * 「退不掉 ChatGPT」不在这里 —— 那种情况 switchTier 会 reject 且配置未改动。
   */
  warnings: string[];
}

export interface OperatorBalance {
  balance: number;
  frozenBalance: number;
}

export const operatorApi = {
  status: (): Promise<OperatorStatus> => invoke("operator_status"),

  /** 探测域名并存为当前站点。空串走默认域名。 */
  probeSite: (site: string): Promise<ProbeResult> =>
    invoke("operator_probe_site", { site }),

  /** 开登录窗。返回 true 表示拿到凭据，false 表示用户关窗或超时。 */
  login: (): Promise<boolean> => invoke("operator_login"),

  /** 拉分组并为每组备好密钥。 */
  provision: (): Promise<ProvisionSummary> => invoke("operator_provision"),

  listTiers: (): Promise<TierInfo[]> => invoke("operator_list_tiers"),

  /** 切换档位。quitChatgpt 由用户在确认弹窗里同意后传 true。 */
  switchTier: (
    providerId: string,
    quitChatgpt: boolean,
  ): Promise<SwitchTierResult> =>
    invoke("operator_switch_tier", { providerId, quitChatgpt }),

  logout: (): Promise<void> => invoke("operator_logout"),

  balance: (): Promise<OperatorBalance> => invoke("operator_balance"),
};
