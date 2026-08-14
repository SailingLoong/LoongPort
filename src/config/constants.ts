/**
 * LoongPort 官网。站点已建好（源码在另一个仓），域名来自 `tauri.conf.json` 的
 * `identifier`（`dev.loongport.desktop` 反过来就是它），不是随手编的。
 *
 * ⚠️ **发布前必须确认它对匿名访客可达** —— 2026-08-04 实测它挂着 Cloudflare Access
 * 登录墙（`302` → `cloudflareaccess.com`），那时任何用户点这个按钮看到的都是一个
 * 要求登录的陌生页面，比 404 更糟（404 至少一眼看出「还没建好」）。
 * 判据是**匿名**请求：`curl -sSL -o /dev/null -w '%{http_code} %{url_effective}\n'
 * https://loongport.dev` —— 最终 URL 必须还在自己域名上。
 * 用浏览器点不算数：你自己的会话已经过了 Access。
 *
 * 后端有一份等价常量（`src-tauri/src/tray.rs` 的 `OFFICIAL_WEBSITE`）——
 * 托盘菜单在 Rust 侧，跨语言没法共享常量。改这里记得同时改那边。
 */
export const OFFICIAL_WEBSITE = "https://loongport.dev";

/**
 * LoongPort 的 GitHub 仓库。
 *
 * ⚠️ **别改成指上游**（`farion1231/cc-switch`）：那边的 release notes 是**另一份
 * 内容**，用户会以为那就是 LoongPort 的更新说明。这个入口要么指本仓，要么去掉。
 */
export const GITHUB_REPO = "https://github.com/SailingLoong/LoongPort";

/** Rust 后台成功写入某个 VeriDrop 榜单缓存后的刷新通知。 */
export const RELAY_DIRECTORY_UPDATED_EVENT = "relay-directory-updated";

/** Rust 后台完成 models.dev 定价自动刷新后的通知。 */
export const MODELS_DEV_PRICING_UPDATED_EVENT = "models-dev-pricing-updated";

/** Rust 后台完成应用更新检查后的通知（有更新和已是最新都会发送）。 */
export const APP_UPDATE_CHECKED_EVENT = "app-update-checked";

/**
 * LoongPort 托管 provider 的 id 前缀 —— 「这条 provider 是托管档位」的判据。
 *
 * ⚠️ **改它等于所有已生成的 provider 记录当场脱管**（判据失配 ⇒ 过滤全线失效，
 * 且 provision 会为同一分组再插一条新 id）。与 Key 命名契约同属不可逆决定。
 *
 * Rust 侧的权威定义在 `src-tauri/src/relay/managed.rs` 的 `MANAGED_ID_PREFIX`，
 * 那边还有 `is_managed` / `reject_if_managed` 两个守卫。跨语言没法共享常量，
 * 改一边必须改另一边。
 */
export const MANAGED_PROVIDER_ID_PREFIX = "loongport-";

/**
 * localStorage 的两个 key —— 「打开时看到哪一屏」。
 *
 * ## 为什么加 fork 前缀
 *
 * WebView 的存储按 app identifier 隔离，理论上 fork 拿不到上游的值。但这两个 key 决定
 * 用户打开时落在哪一屏，一旦因为任何原因读到了上游留下的值（同一台机器上装过 cc-switch、
 * 或将来 identifier 有变动），用户就会绕过 LoongPort 主面板直接落在 provider 列表上。
 * 换个 key 是一行的事，比事后排查便宜。
 *
 * ## ⚠️ 为什么必须放在这里，而不是各组件本地
 *
 * **这两个 key 曾经分叉过，且分叉时功能静默失效了整段时间。** 上游把它们各写一份字面量
 * （`App.tsx` 一份、`AppSwitcher.tsx` 一份），改名时只改到 `App.tsx` ⇒ 写入端还是
 * `cc-switch-last-app`、读取端已是 `loongport-last-app` ⇒ 「记住上次用的 app」
 * 整体不工作，每次启动都回落 `claude`，**不报错、不崩、无日志**。
 *
 * 漏的原因值得记：`LAST_APP_KEY` 在 `App.tsx` 里**只有读没有写**（写在 `AppSwitcher`），
 * 改的人在那个文件里搜不到 `setItem`，自然以为改完了。
 *
 * 所以这里不只是「集中管理」的偏好 —— 提到一处**消掉了分叉的物理条件**，
 * 比再加一道 `include_str!` 比对闸便宜（见 `CLAUDE.md` §三点六）。
 */
export const LAST_APP_STORAGE_KEY = "loongport-last-app";
/** 见 {@link LAST_APP_STORAGE_KEY} —— 同一组，读写都在 `App.tsx`。 */
export const LAST_VIEW_STORAGE_KEY = "loongport-last-view";

// Provider 类型常量
export const PROVIDER_TYPES = {
  GITHUB_COPILOT: "github_copilot",
  CODEX_OAUTH: "codex_oauth",
  XAI_OAUTH: "xai_oauth",
} as const;

// 托管 OAuth 供应商类型：真实凭据由本地代理按请求注入，因此无论上游是否
// 需要格式转换，都必须开启路由接管才能通过认证。新增此类预设时只需把
// providerType 加进本数组，needsRouting 判定即自动覆盖，无需逐个特判。
export const OAUTH_PROVIDER_TYPES: readonly string[] = [
  PROVIDER_TYPES.GITHUB_COPILOT,
  PROVIDER_TYPES.CODEX_OAUTH,
  PROVIDER_TYPES.XAI_OAUTH,
];

/** 判断某 providerType 是否为托管 OAuth（凭据由代理注入、必须开启路由）。 */
export function isOAuthProviderType(
  providerType: string | null | undefined,
): boolean {
  return providerType != null && OAUTH_PROVIDER_TYPES.includes(providerType);
}

// 用量脚本模板类型常量
export const TEMPLATE_TYPES = {
  CUSTOM: "custom",
  GENERAL: "general",
  NEW_API: "newapi",
  GITHUB_COPILOT: "github_copilot",
  TOKEN_PLAN: "token_plan",
  BALANCE: "balance",
  OFFICIAL_SUBSCRIPTION: "official_subscription",
} as const;

export type TemplateType = (typeof TEMPLATE_TYPES)[keyof typeof TEMPLATE_TYPES];
