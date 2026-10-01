import type { ProxyStatus } from "@/types/proxy";
// Isolated UI documentation fixtures. No external services or filesystem access.
const firstRun =
  new URLSearchParams(window.location.search).get("scenario") === "first-run";
let connected = !firstRun;
let onboardingCompleted = !firstRun;
export let enabled = !firstRun;
export const settings = {
  language: "zh",
  showInTray: true,
  minimizeToTrayOnClose: false,
  enableLocalProxy: true,
  proxyConfirmed: true,
  plazaVisible: true,
  preserveCodexOfficialAuthOnSwitch: true,
  unifyCodexSessionHistory: true,
  visibleApps: {
    claude: true,
    codex: true,
    "claude-desktop": false,
    "codex-image": true,
    grokbuild: true,
    gemini: false,
    opencode: false,
    openclaw: false,
    hermes: false,
    pi: false,
  },
  webdavSync: { enabled: false },
  s3Sync: { enabled: false },
};
export const ids = ["relay-1-codex-standard", "relay-2-codex-backup"];
export const names = ["演示服务 · 标准档", "备用服务 · 标准档"];
export let current = ids[0];
export const providers = () =>
  Object.fromEntries(
    ids.map((id, i) => [
      id,
      {
        id,
        name: names[i],
        settingsConfig: {},
        category: "custom",
        sortIndex: i,
        presentation: {
          isOfficial: false,
          isCurrent: id === current,
          isInConfig: true,
          isDefaultModel: false,
          isManaged: true,
          isReadOnly: true,
          canDelete: false,
          canEdit: true,
          canTestConnectivity: true,
          canConfigureUsage: false,
          usesOfficialSubscriptionUsage: false,
          canSetAsDefault: false,
          routingBadge: "proxy",
          routingReason: null,
          switchBlockedReason: null,
        },
      },
    ]),
  );
export const configurations = () =>
  ids.map((providerId, i) => ({
    providerId,
    name: names[i],
    source: "relay",
    account: { kind: "relay", id: i + 1 },
    serviceName: i ? "备用服务" : "演示服务",
    accountLabel: "demo@example.com",
    configurationName: "标准档",
    model: "gpt-5.4",
    presentation: providers()[providerId].presentation,
    selection: { kind: "relay" },
    canSelect: true,
  }));
export const tiers = () =>
  ids.map((providerId, i) => ({
    providerId,
    name: names[i],
    position: i + 1,
    isCurrent: providerId === current,
    rateMultiplier: i ? 1 : 0.8,
    unitPricePerMillion: 12,
    effectiveModel: "gpt-5.4",
    avgFirstTokenMs: i ? 850 : 620,
    balanceUsd: 25,
    verificationVerdict: null,
    isHealthy: true,
    consecutiveFailures: 0,
    lastError: null,
    todayCostUsd: 0.12,
    todayRequests: 3,
    cacheHitRate: 0.65,
    recentActivity: null,
    breakerState: null,
    breakerReopenInSecs: null,
    affinityRemainingSecs: null,
    skipReason: null,
    errorRate: 0,
    canFailover: true,
    canVerifyModels: true,
    models: ["gpt-5.4", "gpt-5.4-mini"],
    subscriptionWindows: [],
    nextResetAt: null,
  }));
export const relayRows = () =>
  ids.map((id, i) => ({
    id: i + 1,
    siteOrigin: i ? "https://backup.example.com" : "https://example.com",
    siteName: i ? "备用服务" : "演示服务",
    accountLabel: "demo@example.com",
    status: "ready",
    isCurrent: id === current,
    canQueryBalance: false,
    canPurchase: false,
    canViewUsage: false,
    canRefresh: true,
    usageBlockers: [],
    removeConfirmation: "configured",
    tiers: [
      {
        providerId: id,
        name: "标准档",
        displayName: "标准档",
        groupName: "标准档",
        groupId: i + 1,
        isCurrent: id === current,
        rateMultiplier: 1,
        platform: "openai",
        userEdited: false,
        allowImageGeneration: false,
        siteDeclaredOrigin: null,
      },
    ],
  }));
const proxy = (): ProxyStatus => ({
  running: enabled,
  address: "127.0.0.1",
  port: 15721,
  active_connections: 0,
  total_requests: 3,
  success_requests: 3,
  failed_requests: 0,
  success_rate: 100,
  uptime_seconds: 120,
  current_provider: names[0],
  current_provider_id: current,
  last_request_at: null,
  last_error: null,
  failover_count: 0,
  active_targets: [
    { app_type: "codex", provider_name: names[0], provider_id: current },
  ],
});
const seen = new Set<string>();
export async function fixture(command: string, args: any = {}) {
  if (command.startsWith("plugin:event|")) return 1;
  if (command.startsWith("plugin:window|"))
    return command.endsWith("scale_factor") ? 1 : false;
  if (command.startsWith("plugin:app|"))
    return command.endsWith("version") ? "6.26.2" : "LoongPort";
  if (command.startsWith("plugin:log|")) return null;
  if (command === "plugin:path|resolve_directory") return "~";
  if (command === "plugin:path|join") return args.paths.join("/");
  switch (command) {
    case "get_pending_announcements":
    case "get_active_model_mismatches":
    case "relay_check_session":
      return [];
    case "get_cc_switch_import_preview":
      return {
        available: false,
        alreadyImported: false,
        providers: 0,
        mcpServers: 0,
        skills: 0,
      };
    case "get_skills_migration_result":
      return null;
    case "auth_get_status":
      return { authenticated: false, accounts: [] };
    case "get_common_config_snippet":
      return "";
    case "preset_referral_urls":
      return {};
    case "service_onboarding_complete":
      onboardingCompleted = true;
      return { shouldPrompt: false, completed: true, plazaVisible: true };
    case "relay_import_site":
      connected = true;
      return {
        relayId: 1,
        siteOrigin: "https://example.com",
        siteName: "演示服务",
        backendKind: "sub2api",
      };
    case "relay_refresh":
      return {
        summary: {
          notice: "updated",
          refreshedAccounts: 1,
          tiers: 1,
          keysCreated: 0,
          otherPlatformTiers: 0,
          mergedProviders: 0,
          failures: [],
        },
        balances: [],
      };
    case "list_db_backups":
      return [];
    case "get_settings":
      return settings;
    case "save_settings":
      Object.assign(settings, args.settings);
      return true;
    case "get_providers":
      return connected ? providers() : {};
    case "get_current_provider":
      return current;
    case "get_application_overview":
      return {
        isAdditive: false,
        recentProviderIds: [],
        configurations: connected ? configurations() : [],
      };
    case "get_application_routing":
      return {
        autoFailoverEnabled: enabled,
        routingActive: enabled,
        model: "gpt-5.4",
        modelOptions: [
          { model: "gpt-5.4", tierCount: 2, cheapestPricePerMillion: 12 },
        ],
        chainIds: ids,
        tiers: connected ? tiers() : [],
      };
    case "set_application_failover":
      enabled = args.enabled;
      return null;
    case "get_order_profiles":
      return {
        current: "日常使用",
        profiles: [{ name: "日常使用", providerIds: ids }],
      };
    case "service_onboarding_status":
      return { shouldPrompt: false, completed: true, plazaVisible: true };
    case "relay_status":
      return { defaultSite: "https://example.com", shouldPromptAddSite: false };
    case "relay_list_sites":
      return [{ siteOrigin: "https://example.com", accountCount: 1 }];
    case "relay_list_relays":
      return connected &&
        ["codex", "claude"].includes(args.appType || args.appId || args.app)
        ? relayRows()
        : [];
    case "vendor_list":
    case "vendor_list_accounts":
      return { accounts: [], vendors: [] };
    case "relay_list_directory":
    case "relay_directory":
    case "relay_directory_list":
      return {
        items: [],
        total: 0,
        updatedAt: null,
        source: "demo",
        error: null,
      };
    case "get_proxy_status":
      return proxy();
    case "get_proxy_takeover_status":
      return {
        claude: false,
        codex: enabled,
        gemini: false,
        grokbuild: false,
        opencode: false,
        openclaw: false,
      };
    case "get_global_proxy_config":
      return {
        listenAddress: "127.0.0.1",
        listenPort: 15721,
        enableLogging: true,
        enabled: enabled,
      };
    case "get_proxy_config_for_app":
      return {
        appType: args.appType,
        enabled: enabled,
        autoFailoverEnabled: enabled,
        maxRetries: 2,
        firstByteTimeout: 60,
        streamIdleTimeout: 120,
        nonStreamingTimeout: 600,
        circuitFailureThreshold: 3,
        circuitSuccessThreshold: 2,
        circuitTimeout: 60,
        circuitErrorRateThreshold: 0.5,
        circuitMinRequests: 5,
      };
    case "check_env_conflicts":
      return [];
    case "check_all_env_conflicts":
      return {};
    case "get_config_dir":
      return `~/.${args.app}`;
    case "get_app_config_path":
      return "~/.loongport/settings.json";
    case "get_claude_code_config_path":
      return "~/.claude/settings.json";
    case "get_resolved_config_dirs":
      return {};
    case "is_portable_mode":
    case "get_migration_result":
    case "has_codex_unify_history_backup":
      return false;
    case "get_star_reward_offer":
      return null;
    case "get_app_version":
      return "6.26.2";
    case "get_secret_protection_status":
      return { mode: "system", unlocked: true, passwordConfigured: false };
    case "model_verification_summaries":
    case "get_model_verification_summaries":
      return [];
    case "get_init_error":
    case "get_dismissed_update_version":
    case "get_app_config_dir_override":
      return null;
    case "get_mcp_config":
      return { servers: {} };
    case "get_skills_config":
      return { repositories: [], installed: [] };
    case "get_skills_settings":
      return { storagePath: "~/.loongport/skills", syncMode: "copy" };
    case "get_auto_launch":
      return false;
    case "get_crowd_metrics_enabled":
      return false;
    case "get_usage_telemetry_enabled":
      return false;
    case "get_current_profile":
      return null;
  }
  if (
    /^(list_|scan_|get_.*(?:list|profiles|backups|repositories|sessions|prompts|agents)|.*_list$)/.test(
      command,
    )
  )
    return [];
  if (/^(ensure_|update_tray|log_|set_|save_)/.test(command)) return true;
  if (!seen.has(command)) {
    console.info("UNHANDLED_FIXTURE", command, JSON.stringify(args));
    seen.add(command);
  }
  return null;
}
