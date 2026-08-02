import { invoke } from "@tauri-apps/api/core";

/** 当前运营商状态，决定首启显示哪一屏。 */
export interface OperatorStatus {
  /** 域名输入框的底纹词。 */
  defaultSite: string;
  /** 已选定的站点；null 表示还没选过（该弹域名输入框）。 */
  siteOrigin: string | null;
  siteName: string | null;
  /**
   * 当前站登录的账号名（昵称优先，回落邮箱）。空串 = 还没登录。
   *
   * 同一个站可以挂多个账号，所以「登录了」不够 —— 要说清是哪个账号。
   * 内部去重认的是服务端数值 id，不是这个标签。
   */
  accountLabel: string;
  /** 是否已有可用凭据。 */
  loggedIn: boolean;
  /**
   * **登录过、但凭据已经不能用了** —— 据此提示「登录已过期，请重新登录」，
   * 而不是像从没登录过一样只摆一个「登录」按钮。
   *
   * 与 `!loggedIn` 不同：那个把「从没登录」与「登录过但过期」混成一件事，而对用户是两种
   * 处境。还有 refresh token 时下一次请求会自动续期、用户不必管，那种情况**不算过期**。
   */
  sessionExpired: boolean;
  /**
   * 重新登录时预填进登录框的值（空串 = 没有，让用户自己输）。
   *
   * 是登录标识本身而不是 `accountLabel`：后者昵称优先，设了昵称的用户拿它填登录框是错的。
   */
  loginIdentifier: string;
  /** 已备好密钥的档位数。 */
  tierCount: number;
  /**
   * 切换分组前要不要先提示用户处理 ChatGPT。
   *
   * 不是「装了没有」—— 非 macOS 平台查不到那个事实，那边恒为 true。
   */
  chatgptNeedsAttention: boolean;
}

/** 一个已添加的站点（给站点切换器用）。 */
export interface SiteInfo {
  id: number;
  siteOrigin: string;
  siteName: string;
  /** 登录后的账号名（昵称优先，回落邮箱），未登录为空串。 */
  accountLabel: string;
  /** 展示名：站名 +（有账号时）邮箱。 */
  label: string;
  loggedIn: boolean;
  isCurrent: boolean;
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

  /**
   * 探一次凭据是不是真的还能用（会先尝试静默续期）。
   *
   * 返回 false 表示凭据已失效且本地记录已清掉 —— 不是错误，引导用户重新登录即可。
   * status 里的 loggedIn 只看本地过期时间，凭据在网页端被撤销时它仍是 true。
   */
  checkSession: (): Promise<boolean> => invoke("operator_check_session"),

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

  /** 登出当前站（别的站的登录态不动）。 */
  logout: (): Promise<void> => invoke("operator_logout"),

  listSites: (): Promise<SiteInfo[]> => invoke("operator_list_sites"),

  /** 切换当前站点。 */
  switchSite: (id: number): Promise<void> =>
    invoke("operator_switch_site", { id }),

  /**
   * 删掉一个站点。
   *
   * **不会删它已生成的 provider 记录** —— 那些可能是用户正在用的配置。
   */
  removeSite: (id: number): Promise<void> =>
    invoke("operator_remove_site", { id }),

  balance: (): Promise<OperatorBalance> => invoke("operator_balance"),
};
