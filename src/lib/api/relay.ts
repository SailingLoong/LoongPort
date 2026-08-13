import { invoke } from "@tauri-apps/api/core";

import type { UsageResult } from "@/types";

import type { AppId } from "./types";

/**
 * 「加站弹窗要什么」+「切档位前要不要提醒处理 ChatGPT」。
 *
 * 2026-08-04 从 9 个字段收缩到 2 个 —— 那 7 个服务的是已删的 LoongPort 独立页
 * 那个「当前站」单站视图。中转站行现在每行各显示自己的状态、数据走 `listRelays`。
 * 后端 `RelayStatus` 的文档写了完整理由（含为什么不留着当预留）。
 */
export interface RelayStatus {
  /** 域名输入框的底纹词。 */
  defaultSite: string;
  /**
   * 切换分组前要不要先提示用户处理 ChatGPT。
   *
   * 不是「装了没有」—— 非 macOS 平台查不到那个事实，那边恒为 true。
   */
  chatgptNeedsAttention: boolean;
}

/**
 * 一个已添加的站点。
 *
 * 当前消费者是「一个站都没有吗」的自动引导判据（只数条数）。新增站点只有在
 * 注册或登录成功后才会写入；启动建表路径也会清理旧版遗留的未认证占位行。
 */
export interface SiteInfo {
  siteOrigin: string;
  /** 登录后的账号名（昵称优先，回落邮箱）。 */
  accountLabel: string;
}

export type BackendKind = "sub2api" | "newapi";

export type DiscoveryErrorKind =
  | "unsupported_site"
  | "protocol_conflict"
  | "transport"
  | "cancelled";

export interface RelayImportError {
  kind?: DiscoveryErrorKind;
  message: string;
}

export interface ProbeResult {
  /** 探测成功后这个站在本地的行 id（已存在则是原来那行，后端会收口）。 */
  relayId: number;
  siteOrigin: string;
  siteName: string;
  readonly backendKind: BackendKind;
}

/** 站点发现与同一浏览器会话认证均成功后的结果。 */
export type ImportResult = ProbeResult;

/**
 * 一个推荐中转站（首启屏那几个按钮）。
 *
 * 来自远端配置（Ed25519 验签过），不是编译期常量 —— 谈成新赞助商不用发版。
 */
export interface Sponsor {
  /** 站点 origin（如 `https://bestapi.store`）。直接喂给 `importSite`。 */
  siteOrigin: string;
  /** 展示名。**服务端给什么就显示什么** —— 不翻译、不美化。 */
  displayName: string;
  /** 一句话介绍，可能是空串。 */
  tagline: string;
}

export type LeaderboardKind = "overall" | "claude" | "openai" | "gemini";

export interface ProtocolScore {
  protocol: string;
  score: number;
  samples: number;
  verdict: string | null;
  reportUrl: string | null;
}

export interface RelayDirectoryItem {
  siteHost: string;
  veridropHost: string;
  displayName: string;
  rank: number;
  score: number;
  samples: number;
  latestDate: string;
  detailUrl: string;
  protocolScores: ProtocolScore[];
  claudeSignatureRate: number | null;
  scenarios: string[];
  issues: string[];
  entryUrl: string;
}

export interface RelayLeaderboard {
  kind: LeaderboardKind;
  items: RelayDirectoryItem[];
  syncedAt: number;
  fromCache: boolean;
}

export interface TierInfo {
  providerId: string;
  /**
   * 这个档位落在哪个 CLI 上（`"codex"` / `"claude"` / …）。
   *
   * ⚠️ **`provision` 返回的 tiers 是全平台的** —— 它一次探全部平台，每个分组按自己的
   * `platform` 落到对应 CLI。所以读这个字段筛出「属于当前那一屏的」，别假设全都是。
   * `listRelays` / `listTiers` 那两条路按 app 查，结果天然同质。
   */
  appId: AppId;
  groupName: string;
  displayName: string;
  /** The model currently written into this provider's config. */
  model: string;
  /**
   * Codex conversation models discovered from this tier's `/v1/models` endpoint.
   * Empty for other apps or when no complete remote catalog was fetched.
   */
  models: string[];
  /**
   * 计费倍率（分组默认 × 用户专属），越小越便宜。`null` = 未知，**不要当 0 显示**。
   *
   * 值在 provision 时算好落库，`listRelays` 直接读本地 —— 所以**首屏就有**，
   * 而刷新它就等于重新拉分组（顶部「刷新」/ 行上「更新可用分组」/ 登录成功）。
   * 中间任何一次 reload 都不会为它发网络请求：倍率是服务端定价，不是实时量。
   *
   * ⚠️ **不含高峰时段因子** —— 与 sub2api 自己面板的口径一致（它把高峰窗口
   * 单独标出来，不乘进这个数字）。
   */
  rateMultiplier: number | null;
  isCurrent: boolean;
  /**
   * 用户在 cc-switch 编辑页改过这个档位的配置吗。
   *
   * 判据是「当前配置 ≠ 我们会生成的默认配置（密钥除外）」，后端每次现算 ——
   * 不存标记，所以用户把配置改回默认后它会自动消失。
   *
   * `null` = **判不了**（读不出密钥 / 这个 CLI 没有默认形状）。
   * UI 在 `null` 时什么标记都不显示：`false` 是在断言「刷新不会覆盖你的改动」，
   * 而事实是「不知道」—— 让用户误信比不说更糟。
   *
   * 只有 `listRelays` 会给出真值 —— 判据要站点的 `api_base_url`，
   * 而那是按站点存的，只有分组到中转站之后才拿得到。
   */
  userEdited: boolean | null;
  /**
   * 服务端说这个分组允许生图（`allow_image_generation`）。
   *
   * ⚠️ **纯生图分组不靠这个字段识别** —— 它们在「生图」那个 tab 下（后端
   * `AppType::CodexImage`），所在的列表本身就说明了这件事。这个字段的价值在
   * **混合分组**：实测 `pro池` 这类有文本模型的分组也是 `true`，它们留在 codex tab 里
   * 而同时支持生图。
   *
   * `null` = **判不了**（只有 provision 那条路拿得到这个字段，`listRelays`
   * 是只读本地的）。`null` 时不显示任何标记 —— 与 `userEdited` 同一条处理原则：
   * 不知道就别断言。
   */
  allowImageGeneration: boolean | null;
}

/**
 * 「中转站 × 分组」页的一行中转站，连带它在当前 app 下的档位。
 *
 * 数据来自 `relay_list_relays`，**只读本地不发网络** —— 倍率也在本地
 * （provision 时写下的值），所以首屏一次渲染就是完整的。
 */
export interface RelayRow {
  id: number;
  siteOrigin: string;
  siteName: string;
  /** 登录后的账号名，未登录为空串。同一个站挂多个账号时靠它分辨。 */
  accountLabel: string;
  status:
    | "notLoggedIn"
    | "sessionExpired"
    | "sessionExpiredUsable"
    | "noTiers"
    | "ready";
  isCurrent: boolean;
  canQueryBalance: boolean;
  canRefresh: boolean;
  canDelete: boolean;
  tiers: TierInfo[];
}

export interface ProvisionSummary {
  tiers: TierInfo[];
  /** 失败的分组。**不为空也不代表整体失败** —— 成功的那些照样能用。 */
  failures: Array<{ groupName: string; reason: string }>;
  /** 这次新建了几把密钥（其余是复用已有的）。 */
  keysCreated: number;
  /** Imported non-managed providers removed because LoongPort now owns the same credential. */
  mergedProviders: Array<{ name: string; appId: AppId }>;
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

/** 「切回官方登录」的结果。字段名与 Rust 侧 `RestoreOfficialLoginResult` 一一对应。 */
export interface RestoreOfficialLoginResult {
  /**
   * 备份文件的完整路径。`null` 表示本来就没有 `auth.json`（没登录过 ChatGPT）。
   *
   * 要显示给用户：那里面是 OAuth refresh token，手滑点了确认时得知道去哪儿捞回来。
   */
  backupPath: string | null;
  /** ChatGPT 退出前是不是在跑（决定操作后要不要替用户重开）。 */
  chatgptWasRunning: boolean;
  /**
   * 非致命问题（如重开失败、删 auth.json 失败）。
   *
   * 「用户在退出确认框点了取消」不在这里 —— 那种情况会 reject 且一个文件都没碰。
   */
  warnings: string[];
}

export const relayApi = {
  status: (): Promise<RelayStatus> => invoke("relay_status"),

  /**
   * 匿名统计的上报端点配好了没。
   *
   * `false` 时**同意与不同意的实际后果完全相同**（后端上报任务第一道闸就是这个），
   * 所以首启告知那一屏不该弹 —— 见 `StatsNoticeDialog` 的文档。
   *
   * 有意不并进 `status()`：那条命令在首屏关键路径上，这个事实只有告知那屏要用。
   */
  statsEndpointConfigured: (): Promise<boolean> =>
    invoke("relay_stats_endpoint_configured"),

  /**
   * 推荐中转站（首启屏那几个按钮）。
   *
   * **空数组是正常结果**，不是错误 —— 三种情形都会空：首启那几秒还没拉到、
   * 没网、维护者临时撤空了列表。UI 拿到空就只显示手动输入框。
   *
   * 读的是本地缓存（后端不发网络请求），所以调它不会让界面等。
   */
  listSponsors: (): Promise<Sponsor[]> => invoke("relay_list_sponsors"),

  /** 每次打开或切换榜单都实时拉取；后端仅在实时失败时返回上次成功缓存。 */
  listDirectory: (kind: LeaderboardKind): Promise<RelayLeaderboard> =>
    invoke("relay_list_directory", { kind }),

  /**
   * 导入第三方站点。原生发现失败时由后端打开可见网页，让用户自行完成验证，
   * 并在同一个浏览器会话中识别协议、注册或登录。输入必须原样透传以保留邀请链接。
   */
  importSite: (site: string): Promise<ImportResult> =>
    invoke("relay_import_site", { site }),

  /**
   * 探一遍每一行凭据是不是真的还活着，返回**这次被清掉凭据的行 id**（空 = 全都好）。
   *
   * 曾经它探「当前站」一行、返回 bool —— 那个形状只对单站界面成立。
   * 中转站区是多行并列的，探一行等于让另外 N-1 行继续显示错的状态。
   */
  checkSession: (): Promise<number[]> => invoke("relay_check_session"),

  /**
   * 开登录窗。返回 true 表示拿到凭据，false 表示用户关窗或超时。
   *
   * `relayId` 指定重新登录**哪一行**，**必填**。新增站点走 `importSite`；
   * 本命令保留给已有站点行的独立重登录。
   *
   * **每次都是全新登录态**（登录窗用 incognito，见后端注释）：同一个站可以
   * 挂多个账号，删掉再加也不会复用旧 token。
   */
  login: (relayId: number): Promise<boolean> =>
    invoke("relay_login", { relayId }),

  /**
   * 拉分组并为每组备好密钥。**一次探全部平台，各归各的 tab。**
   *
   * 不吃 app 参数 —— 每个分组落到哪个 CLI 由它自己的 platform 决定
   * （openai→codex、anthropic→claude、gemini→gemini、grok→grokbuild）。
   * 用户在任何一个 tab 登录一次，全部平台的档位都备好了。
   *
   * `relayId` 指定作用于**哪一行**，**必填**。曾经可省略、回落到「当前站」
   * 那个全局单例状态，于是两个中转站同时 provision 会互相串目标
   * （原来靠「任一操作进行中就禁用所有行」兜住 —— 那是拿全局禁用换正确性）。
   *
   * 认不出配置形状的 CLI 会计入 `failures`，不让整批失败。
   */
  provision: (relayId: number): Promise<ProvisionSummary> =>
    invoke("relay_provision", { relayId }),

  /**
   * 「中转站 × 分组」页的数据源：一次拿到全部中转站 + 各自在该 app 下的档位。
   *
   * `app` 传当前 tab 的 app_type（如 `"codex"`）。
   *
   * **只读本地、不发网络**（首屏不卡在网络上）。倍率也在返回值里 —— 它由
   * provision 落库，所以这条命令**不需要**任何后续的异步补齐；
   * 曾经那条「每个档位一次 HTTP」的 `listTierRates` 已经删掉。
   */
  listRelays: (app: string): Promise<RelayRow[]> =>
    invoke("relay_list_relays", { app }),

  /**
   * 保存中转站行的手工顺序。`relayIds` 是拖动后的完整顺序，下标即 sort_index。
   *
   * 为什么行序要落库而不是存 localStorage：它是用户对「哪个中转站常用」的表达，
   * 换台机器也该一致。折叠状态才是纯 UI 偏好（那个存 localStorage）。
   */
  reorder: (relayIds: number[]): Promise<void> =>
    invoke("relay_reorder", { relayIds }),

  /**
   * 切换档位。`quitChatgpt` 由用户在确认弹窗里同意后传 true。
   *
   * `app` 是**必需的**，不能让后端从 providerId 反推 —— 那个 id 是
   * `sha256(siteOrigin + groupId)`，不含 platform 且单向不可逆，
   * 同一个 id 可以合法地存在于多个 app_type 下。
   *
   * 注意 `quitChatgpt` 只在 `app === "codex"` 时真的生效（ChatGPT 桌面版只读
   * `~/.codex`，切别的平台去退它纯属扰民）—— 那个判断在后端，前端照实传即可。
   */
  switchTier: (
    providerId: string,
    app: string,
    quitChatgpt: boolean,
  ): Promise<SwitchTierResult> =>
    invoke("relay_switch_tier", { providerId, app, quitChatgpt }),

  /**
   * Select one of the models advertised by a managed Codex tier. The backend
   * validates membership in the persisted catalog before switching, so a
   * stale UI cannot point Codex at an unsupported model.
   */
  switchTierModel: (
    providerId: string,
    app: string,
    model: string,
    quitChatgpt: boolean,
  ): Promise<SwitchTierResult> =>
    invoke("relay_switch_tier_model", {
      providerId,
      app,
      model,
      quitChatgpt,
    }),

  listSites: (): Promise<SiteInfo[]> => invoke("relay_list_sites"),

  /**
   * 删掉一个站点，**连带它名下已生成的托管档位**。
   *
   * 判据是 `website_url == site_origin` + 账号维度，用户自建的 provider 一律不碰
   * （后端 `relay_remove_site` 的文档写了完整理由）。
   */
  removeSite: (id: number): Promise<void> =>
    invoke("relay_remove_site", { id }),

  /**
   * 查某个中转站的余额。
   *
   * `relayId` 指定查**哪一行**，**必填** —— 中转站行各显示自己的余额。
   * （曾经可省略、回落到「当前站」，那会让每一行都显示同一个数字而且是别人的；
   * `is_current` 整个概念已删，见后端 `creds` 模块文档。）
   *
   * ## 返回上游那个 [`UsageResult`]，与官网行的 `vendorApi.balance` **同一契约**
   *
   * 曾经这条回 `{balance, frozenBalance}` 数字、官网那条回后端拼好的字符串
   * `"¥547.08"` —— 同一个事实两套形状，前端因此长出两份 state、两个 effect、
   * 两处渲染。统一之后两类行共用一个 hook（`useRowBalanceQuery`）与一个呈现件
   * （`InlineUsage`，即 provider 页那条用量条）。
   *
   * 失败**不 reject**：后端三条路都查不到时回 `success:false`（见
   * `src-tauri/src/relay/balance.rs` 模块文档）。那样用量条才能渲染失败态并留住
   * 刷新按钮 —— reject 会让整块余额区消失，用户无从重查。
   *
   * ⚠️ **登录态过期也查得到**：后端前两步走 sk，不需要网页登录态。别在调用方
   * 按 `sessionExpired` 把这条路关掉。
   */
  balance: (relayId: number): Promise<UsageResult> =>
    invoke("relay_balance", { relayId }),

  /**
   * 带登录态打开某个中转站的充值页。
   *
   * resolve 只表示**窗口开出来了**，不表示用户付了钱 —— 我们有意不做支付成功感知，
   * 关窗时刷一次余额就够（充完钱余额自然会涨）。关窗事件是
   * `relay-purchase-closed`，payload 是 `relayId`。
   *
   * `relayId` **必填**。曾经可省略、回落到「当前站」——那意味着用户在第 3 行点充值
   * 会打开第 1 行的充值页，钱充进**别的账号**。类型层面堵掉它比靠注释提醒可靠。
   */
  purchase: (relayId: number): Promise<void> =>
    invoke("relay_purchase", { relayId }),

  /**
   * 把某个托管档位的配置恢复成默认值。
   *
   * 用户能在 cc-switch 现成的编辑页里改托管档位的全部字段（我们不重做那一页），代价是
   * 可能改坏 —— 改错 `base_url`、删掉 `disable_response_storage`、把 `model_provider`
   * 从 `custom` 改成 `OpenAI`（会让会话历史分家）。这些都**不报错**，只让调用静默失败。
   * 这条命令是那条回头路。
   *
   * sk 保留不变（后端 `extract_api_key` 读出来再塞回去）。只对托管档位有效。
   */
  resetTierConfig: (providerId: string, app: string): Promise<void> =>
    invoke("relay_reset_tier_config", { providerId, app }),

  /**
   * 一键「切回官方登录」：清 codex 的第三方路由与登录态。
   *
   * **为什么需要它**：LoongPort 把 codex 配成 provider auth 模式
   * （`experimental_bearer_token` 在 `config.toml` 里），鉴权压根不看 `auth.json` ⇒
   * 用户在 ChatGPT 里点「注销」没有任何反应，请求照样带 sk 打到中转站。
   *
   * 后端会原子地做四件事（退 ChatGPT → 备份 `auth.json` → 切 `codex-official` → 删
   * `auth.json`）。用户在 ChatGPT 的退出确认框里点取消 ⇒ reject 且**一个文件都没碰**。
   */
  restoreOfficialLogin: (): Promise<RestoreOfficialLoginResult> =>
    invoke("relay_restore_official_login"),
};
