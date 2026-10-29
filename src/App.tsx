import type { ProviderUpdateInput } from "@/lib/api/providers";
import {
  applicationRoutingApi,
  isApplicationRoutingWriteBlocked,
} from "@/lib/api/applicationRouting";
import { ZCodeProviderPanel } from "@/components/zcode/ZCodeProviderPanel";
import { useCallback, useEffect, useMemo, useState, useRef } from "react";
import { useTranslation } from "react-i18next";
import { motion, AnimatePresence } from "framer-motion";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Plus,
  ArrowLeft,
  Minus,
  Maximize2,
  Minimize2,
  X,
  History,
  Download,
  FolderArchive,
  Search,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Provider, VisibleApps } from "@/types";
import type { EnvConflict } from "@/types/env";
import { proxyKeys, useProvidersQuery, useSettingsQuery } from "@/lib/query";
import {
  providersApi,
  settingsApi,
  PROFILE_APPLIED,
  S3_SYNC_STATUS_UPDATED,
  UNIVERSAL_PROVIDER_SYNCED,
  WEBDAV_SYNC_STATUS_UPDATED,
  type AppId,
  type ProviderSwitchEvent,
} from "@/lib/api";
import { checkAllEnvConflicts, checkEnvConflicts } from "@/lib/api/env";
import {
  GITHUB_REPO,
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
  OFFICIAL_WEBSITE,
} from "@/config/constants";
import { useProviderActions } from "@/hooks/useProviderActions";
import { openclawKeys, useOpenClawHealth } from "@/hooks/useOpenClaw";
import { useOpenHermesWebUI } from "@/hooks/useHermes";
import { hermesApi } from "@/lib/api/hermes";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import { useUsageCacheBridge } from "@/hooks/useUsageCacheBridge";
import { useRelayDirectoryCacheBridge } from "@/hooks/useRelayDirectoryCacheBridge";
import { useProviderModelsCacheBridge } from "@/hooks/useProviderModelsCacheBridge";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useLastValidValue } from "@/hooks/useLastValidValue";
import { useScanUnmanagedSkills } from "@/hooks/useSkills";
import {
  extractErrorMessage,
  translatePiProviderMutationError,
} from "@/utils/errorUtils";
import { isTextEditableTarget } from "@/utils/domUtils";
import { cn } from "@/lib/utils";
import {
  isWindows,
  isLinux,
  DRAG_REGION_ATTR,
  DRAG_REGION_STYLE,
} from "@/lib/platform";
import { PreservedView } from "@/components/ui/PreservedView";
import { AppSwitcher } from "@/components/AppSwitcher";
import { ClientSidebar } from "@/components/shell/ClientSidebar";
import {
  PAGE_HEADER_HEIGHT,
  SERVICES_CONTENT_CLASS_NAME,
} from "@/components/shell/layout";
import { FeatureHub } from "@/components/shell/FeatureHub";
import {
  CLIENT_VIEWS,
  useClientNavigation,
  type ClientView as View,
} from "@/components/shell/navigation";
import { ApplicationRouteWorkspace } from "@/components/applications/ApplicationRouteWorkspace";
import { SharedGroupsPage } from "@/components/applications/SharedGroupsPage";
import { serviceSourceDestination } from "@/components/applications/serviceSourceNavigation";
import {
  ApplicationWorkspace,
  type ApplicationWorkspaceHandle,
} from "@/components/applications/ApplicationWorkspace";
import { ServicesPage } from "@/components/relay/accounts/ServicesPage";
import {
  OPEN_ACCOUNT_MANAGER,
  isAccountPlatform,
} from "@/components/relay/accounts/accountPlatforms";
import {
  ManagedAccountApplicationPanel,
  type ManagedAccountApplicationHandle,
} from "@/components/relay/accounts/ManagedAccountApplicationPanel";
import { useAccountSessionStartup } from "@/components/relay/accounts/useAccountSessionStartup";
import { RelayDirectoryConnectionPage } from "@/components/relay/onboarding/RelayDirectoryConnectionPage";
import { useServiceOnboardingStatus } from "@/components/relay/onboarding/useServiceOnboarding";
import { ProfileSwitcher } from "@/components/profiles/ProfileSwitcher";
import { McodeProviderPanel } from "@/components/providers/McodeProviderPanel";
import { ProviderList } from "@/components/providers/ProviderList";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  SettingsPage,
  type SettingsPageHandle,
} from "@/components/settings/SettingsPage";
import { UsagePage } from "@/components/usage/UsagePage";
import { ThemeSettings } from "@/components/settings/ThemeSettings";
import { CredentialProtectionEntry } from "@/components/relay/CredentialProtectionEntry";
import {
  credentialProtectionApi,
  credentialProtectionKey,
} from "@/lib/api/credentialProtection";
import { UpdateBadge } from "@/components/UpdateBadge";
import { GitHubStarButton } from "@/components/GitHubStarButton";
import type { ServiceEntryIntent } from "@/components/relay/accounts/serviceEntryIntent";
import { AddHubPage, type AddHubTab } from "@/components/relay/AddHubPage";
import { EnvWarningBanner } from "@/components/env/EnvWarningBanner";
import { ProxyToggle } from "@/components/proxy/ProxyToggle";
import { ClaudeDesktopRouteToggle } from "@/components/proxy/ClaudeDesktopRouteToggle";
import UsageScriptModal from "@/components/UsageScriptModal";
import UnifiedMcpPanel from "@/components/mcp/UnifiedMcpPanel";
import PromptPanel, {
  type PromptPanelHandle,
  type PromptPrimaryAction,
} from "@/components/prompts/PromptPanel";
import {
  SkillsPage,
  getSkillsPageHeaderActions,
  type SkillsPageSource,
} from "@/components/skills/SkillsPage";
import UnifiedSkillsPanel, {
  type SkillsCheckUpdatesState,
} from "@/components/skills/UnifiedSkillsPanel";
import { AnnouncementDialog } from "@/components/AnnouncementDialog";
import { ModelMismatchBanner } from "@/components/ModelMismatchBanner";
import { DeepLinkImportDialog } from "@/components/DeepLinkImportDialog";
import { CcSwitchImportEntry } from "@/components/settings/CcSwitchImportEntry";
import { ImageTabPage } from "@/components/relay/ImageTabPage";
import { useCodexSwitchGuard } from "@/components/relay/useCodexSwitchGuard";
import { ClientsPanel } from "@/components/clients/ClientsPanel";
import { AgentsPanel } from "@/components/agents/AgentsPanel";
import { UniversalProviderPanel } from "@/components/universal";
import { Button } from "@/components/ui/button";
import { SessionManagerPage } from "@/components/sessions/SessionManagerPage";
import {
  useDisableCurrentOmo,
  useDisableCurrentOmoSlim,
} from "@/lib/query/omo";
import { invalidatePiProviderCaches, usePiCurrentState } from "@/lib/query/pi";
import WorkspaceFilesPanel from "@/components/workspace/WorkspaceFilesPanel";
import EnvPanel from "@/components/openclaw/EnvPanel";
import ToolsPanel from "@/components/openclaw/ToolsPanel";
import AgentsDefaultsPanel from "@/components/openclaw/AgentsDefaultsPanel";
import OpenClawHealthBanner from "@/components/openclaw/OpenClawHealthBanner";
import HermesMemoryPanel from "@/components/hermes/HermesMemoryPanel";
import {
  APP_IDS,
  PROVIDER_STORE_APP_IDS,
  usesProviderStore,
  DEFAULT_VISIBLE_APPS,
  isProxyAppId,
} from "@/config/appConfig";

interface SyncStatusUpdatedPayload {
  source?: string;
  status?: string;
  error?: string;
}

const DEFAULT_DRAG_BAR_HEIGHT = isWindows() || isLinux() ? 0 : 28; // px

// 两个 localStorage key 的定义在 `@/config/constants` —— 别在这里重新写字面量。
// activeApp 的保存统一在下方 effect，切换请求尚待草稿确认时不提前写入。
const getInitialApp = (): AppId => {
  const saved = localStorage.getItem(LAST_APP_STORAGE_KEY) as AppId | null;
  if (saved && APP_IDS.includes(saved)) {
    return saved;
  }
  return "claude";
};

const VALID_VIEWS: View[] = CLIENT_VIEWS.filter((view) => view !== "addHub");

const getInitialView = (): View => {
  const saved = localStorage.getItem(LAST_VIEW_STORAGE_KEY) as View | null;
  if (saved && VALID_VIEWS.includes(saved)) {
    return saved;
  }
  // 首启落 providers —— 中转站行就在它顶部（加站 / 登录 / 获取密钥 / 切档位
  // 全在那儿）。2026-08-04 之前默认落已删除的 LoongPort 独立页。
  return "providers";
};

function App() {
  const runtime = useQuery({
    queryKey: ["runtimeAvailability"],
    queryFn: () =>
      invoke<{ credentialRuntimeAvailable: boolean }>(
        "get_runtime_availability",
      ),
    retry: false,
  });
  const credentialRuntimeAvailable =
    runtime.data?.credentialRuntimeAvailable === true && !runtime.isError;
  const protection = useQuery({
    queryKey: credentialProtectionKey,
    queryFn: credentialProtectionApi.status,
    enabled: !runtime.isPending && !credentialRuntimeAvailable,
    retry: false,
  });
  const restartRequired =
    !credentialRuntimeAvailable &&
    !protection.isError &&
    protection.data?.restartRequired === true;
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const workspaceRef = useRef<ApplicationWorkspaceHandle>(null);
  const settingsRef = useRef<SettingsPageHandle>(null);
  const managedAccountRef = useRef<ManagedAccountApplicationHandle>(null);
  const editingSessionApp = useRef<AppId | null>(null);
  const pendingNavigation = useRef<(() => void) | null>(null);
  const [leaveDraftOpen, setLeaveDraftOpen] = useState(false);
  const beforeNavigate = useCallback(
    (proceed: () => void) => {
      if (
        workspaceRef.current?.submitting ||
        settingsRef.current?.submitting ||
        managedAccountRef.current?.submitting
      ) {
        toast.info(t("common.saving"));
        return;
      }
      if (pendingNavigation.current) return;
      if (
        !workspaceRef.current?.hasPendingChanges &&
        !settingsRef.current?.hasPendingChanges &&
        !managedAccountRef.current?.hasPendingChanges
      ) {
        proceed();
        return;
      }
      // First destination wins until the user explicitly keeps or discards this draft.
      pendingNavigation.current = proceed;
      setLeaveDraftOpen(true);
    },
    [t],
  );
  const cancelLeaveDraft = () => {
    pendingNavigation.current = null;
    setLeaveDraftOpen(false);
  };
  const navigation = useClientNavigation(
    getInitialView,
    getInitialApp,
    beforeNavigate,
  );
  const activeApp = navigation.app;
  // Observe the existing workspace query; do not mirror its facts in local state.
  const routingAdmission = useQuery({
    queryKey: ["applicationRouting", activeApp],
    queryFn: () => applicationRoutingApi.get(activeApp),
    enabled: false,
  });
  const providerWritesBlocked =
    !credentialRuntimeAvailable ||
    (isProxyAppId(activeApp) &&
      (!routingAdmission.data ||
        Boolean(routingAdmission.error) ||
        isApplicationRoutingWriteBlocked(
          activeApp,
          routingAdmission.data.modeState,
        )));
  const setActiveApp = useCallback(
    (app: AppId) => {
      if (editingSessionApp.current && app !== editingSessionApp.current)
        return;
      navigation.setApp(app);
    },
    [navigation.setApp],
  );
  const usesStore = usesProviderStore(activeApp);
  const sharedFeatureApp: AppId =
    activeApp === "claude-desktop" ? "claude" : activeApp;
  const currentView = navigation.view;
  const isC2View = currentView === "providers" && isProxyAppId(activeApp);
  const setCurrentView = useCallback(
    (...args: Parameters<typeof navigation.navigate>) => {
      if (
        editingSessionApp.current &&
        args[1] &&
        args[1] !== editingSessionApp.current
      )
        return;
      navigation.navigate(...args);
    },
    [navigation.navigate],
  );
  const goBack = navigation.back;
  useEffect(() => {
    const open = (event: Event) => {
      const platform = (event as CustomEvent<unknown>).detail;
      if (isAccountPlatform(platform))
        setCurrentView("services", undefined, undefined, platform);
    };
    window.addEventListener(OPEN_ACCOUNT_MANAGER, open);
    return () => window.removeEventListener(OPEN_ACCOUNT_MANAGER, open);
  }, [setCurrentView]);
  const isApplicationView =
    currentView === "providers" || currentView === "image";
  const [addHubEntries, setAddHubEntries] = useState<
    Partial<
      Record<
        AppId,
        {
          id: number;
          tab: AddHubTab;
          firstVisit: boolean;
          intent?: ServiceEntryIntent;
        }
      >
    >
  >({});
  const [skillsDiscoverySource, setSkillsDiscoverySource] =
    useState<SkillsPageSource>("repos");
  const [settingsDefaultTab, setSettingsDefaultTab] = useState("general");
  const [isWindowMaximized, setIsWindowMaximized] = useState(false);
  const [mcpManagementBusy, setMcpManagementBusy] = useState(false);
  const [skillsManagementBusy, setSkillsManagementBusy] = useState(false);
  const [skillsNavigationBusy, setSkillsNavigationBusy] = useState(false);
  const [promptManagementBusy, setPromptManagementBusy] = useState(false);
  const [promptNavigationBusy, setPromptNavigationBusy] = useState(false);
  const [zcodeNavigationBusy, setZcodeNavigationBusy] = useState(false);
  const [mcodeNavigationBusy, setMcodeNavigationBusy] = useState(false);
  const [skillsCheckUpdatesState, setSkillsCheckUpdatesState] =
    useState<SkillsCheckUpdatesState>({
      isChecking: false,
      hasSkills: false,
    });

  useEffect(() => {
    // 广场/官方 API 页是从供应商页临时进入的子流程，不应成为下次启动的落点。
    if (currentView !== "addHub") {
      localStorage.setItem(LAST_VIEW_STORAGE_KEY, currentView);
    }
  }, [currentView]);

  const { data: settingsData } = useSettingsQuery({
    bootstrap: !credentialRuntimeAvailable,
  });
  const useAppWindowControls =
    isLinux() && (settingsData?.useAppWindowControls ?? false);
  const dragBarHeight = useAppWindowControls ? 32 : DEFAULT_DRAG_BAR_HEIGHT;
  const contentTopOffset = dragBarHeight + PAGE_HEADER_HEIGHT;
  const visibleApps = useMemo<VisibleApps>(
    () => ({
      ...DEFAULT_VISIBLE_APPS,
      ...settingsData?.visibleApps,
    }),
    [settingsData?.visibleApps],
  );

  useEffect(() => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, activeApp);
  }, [activeApp]);

  // Fallback from sessions view when switching to an app without session support
  useEffect(() => {
    if (
      sharedFeatureApp === "mcode" &&
      [
        "prompts",
        "skills",
        "skillsDiscovery",
        "mcp",
        "sessions",
        "agents",
        "workspace",
      ].includes(currentView)
    ) {
      navigation.replace("providers");
      return;
    }
    if (currentView === "mcp" && sharedFeatureApp === "pi") {
      navigation.replace("providers");
      return;
    }
    if (
      currentView === "sessions" &&
      sharedFeatureApp !== "claude" &&
      sharedFeatureApp !== "codex" &&
      sharedFeatureApp !== "grokbuild" &&
      sharedFeatureApp !== "opencode" &&
      sharedFeatureApp !== "openclaw" &&
      sharedFeatureApp !== "gemini" &&
      sharedFeatureApp !== "hermes" &&
      sharedFeatureApp !== "pi"
    ) {
      navigation.replace("providers");
    }
  }, [sharedFeatureApp, currentView]);

  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [usageProvider, setUsageProvider] = useState<Provider | null>(null);
  const [confirmAction, setConfirmAction] = useState<{
    provider: Provider;
    action: "remove" | "delete";
  } | null>(null);
  const [envConflicts, setEnvConflicts] = useState<EnvConflict[]>([]);
  const [showEnvBanner, setShowEnvBanner] = useState(false);

  const effectiveEditingProvider = useLastValidValue(editingProvider);
  const effectiveUsageProvider = useLastValidValue(usageProvider);

  useAccountSessionStartup(undefined, credentialRuntimeAvailable);
  useUsageCacheBridge();
  useRelayDirectoryCacheBridge();
  useProviderModelsCacheBridge();

  const promptPanelRef = useRef<PromptPanelHandle>(null);
  const [promptPrimaryAction, setPromptPrimaryAction] =
    useState<PromptPrimaryAction>("prompt");
  const mcpPanelRef = useRef<any>(null);
  const skillsPageRef = useRef<any>(null);
  const unifiedSkillsPanelRef = useRef<any>(null);
  // 订阅未管理 Skill 的共享缓存（实际扫描由 UnifiedSkillsPanel 进入页面时触发）。
  // 这里 enabled 默认 false，仅用于「导入」按钮的绿点提示，不主动发起扫描。
  const { data: unmanagedSkills } = useScanUnmanagedSkills();
  const hasUnmanagedSkills = (unmanagedSkills?.length ?? 0) > 0;

  const {
    isRunning: isProxyRunning,
    takeoverStatus,
    status: proxyStatus,
  } = useProxyStatus(credentialRuntimeAvailable);
  const proxyAppId = isProxyAppId(activeApp) ? activeApp : null;
  const currentAppUsesProxy =
    proxyAppId !== null || activeApp === "claude-desktop";
  const isCurrentAppTakeoverActive = proxyAppId
    ? takeoverStatus?.[proxyAppId] || false
    : false;
  const activeProviderId = useMemo(() => {
    if (!proxyAppId) return undefined;
    const target = proxyStatus?.active_targets?.find(
      (t) => t.app_type === proxyAppId,
    );
    return target?.provider_id;
  }, [proxyStatus?.active_targets, proxyAppId]);

  const {
    data,
    isLoading,
    isError: providersReadFailed,
    refetch,
  } = useProvidersQuery(activeApp, {
    isProxyRunning: currentAppUsesProxy && isProxyRunning,
    enabled: usesStore && credentialRuntimeAvailable,
  });
  const { data: piCurrentState } = usePiCurrentState(
    credentialRuntimeAvailable && activeApp === "pi",
  );
  const providers = useMemo(() => data?.providers ?? {}, [data]);
  const isOpenClawView =
    activeApp === "openclaw" &&
    (currentView === "providers" ||
      currentView === "workspace" ||
      currentView === "sessions" ||
      currentView === "openclawEnv" ||
      currentView === "openclawTools" ||
      currentView === "openclawAgents");
  const { data: openclawHealthWarnings = [] } = useOpenClawHealth(
    credentialRuntimeAvailable && isOpenClawView,
  );

  const {
    addProvider,
    updateProvider,
    switchProvider,
    deleteProvider,
    saveUsageScript,
    setAsDefaultModel,
  } = useProviderActions(activeApp);

  // 切 codex 的供应商时也走「退 ChatGPT → 切 → 重开」——**包括 cc-switch 自带的那些**。
  // ChatGPT 桌面版与命令行 codex 共用同一个 `~/.codex`，它在跑的时候切任何 codex 供应商
  // 都会撞上「它启动时读的旧配置还在生效，而且它退出时会回写 config.toml」。
  // 判据与 LoongPort 档位切换完全一致（同一个页面两种行为不该不一样）。
  const { guardedSwitch, switchDialog } = useCodexSwitchGuard(
    switchProvider,
    providerWritesBlocked,
  );

  const handleEnablePiProvider = async (provider: Provider) => {
    try {
      await providersApi.switch(provider.id, "pi");
      await invalidatePiProviderCaches(queryClient);
      await providersApi.updateTrayMenu().catch((error) => {
        console.error(
          "Failed to update tray menu after enabling Pi provider",
          error,
        );
      });
      toast.success(
        t("pi.provider.enabled", {
          defaultValue: "已在 Pi 中启用",
        }),
        { closeButton: true },
      );
    } catch (error) {
      const detail = extractErrorMessage(error);
      toast.error(
        t("pi.provider.enableFailed", {
          defaultValue: "无法在 Pi 中启用此供应商",
        }),
        {
          description:
            translatePiProviderMutationError(detail, t) || detail || undefined,
          closeButton: true,
        },
      );
    }
  };

  const disableOmoMutation = useDisableCurrentOmo();
  const handleDisableOmo = () => {
    disableOmoMutation.mutate(undefined, {
      onSuccess: () => {
        toast.success(t("omo.disabled", { defaultValue: "OMO 已停用" }));
      },
      onError: (error: Error) => {
        toast.error(
          t("omo.disableFailed", {
            defaultValue: "停用 OMO 失败: {{error}}",
            error: extractErrorMessage(error),
          }),
        );
      },
    });
  };

  const disableOmoSlimMutation = useDisableCurrentOmoSlim();
  const handleDisableOmoSlim = () => {
    disableOmoSlimMutation.mutate(undefined, {
      onSuccess: () => {
        toast.success(t("omo.disabled", { defaultValue: "OMO 已停用" }));
      },
      onError: (error: Error) => {
        toast.error(
          t("omo.disableFailed", {
            defaultValue: "停用 OMO 失败: {{error}}",
            error: extractErrorMessage(error),
          }),
        );
      },
    });
  };

  useEffect(() => {
    let unsubscribe: (() => void) | undefined;
    let active = true;

    if (!credentialRuntimeAvailable) return;
    const setupListener = async () => {
      try {
        const off = await providersApi.onSwitched(
          async (event: ProviderSwitchEvent) => {
            if (event.appType === activeApp) {
              await refetch();
            }
            if (event.appType === "pi") {
              await invalidatePiProviderCaches(queryClient);
            }
          },
        );
        if (!active) {
          off();
          return;
        }
        unsubscribe = off;
      } catch (error) {
        console.error("[App] Failed to subscribe provider switch event", error);
      }
    };

    void setupListener();
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [credentialRuntimeAvailable, activeApp, queryClient, refetch]);

  useTauriEvent(UNIVERSAL_PROVIDER_SYNCED, async () => {
    if (!credentialRuntimeAvailable) return;
    await queryClient.invalidateQueries({ queryKey: ["providers"] });
    try {
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.error("[App] Failed to update tray menu", error);
    }
  });

  // 应用项目后刷新相关缓存（providers 由既有 provider-switched 监听承接；
  // proxy 状态由后端直接改 DB，不走 mutation，必须显式刷新）
  useTauriEvent(PROFILE_APPLIED, async () => {
    await queryClient.invalidateQueries({ queryKey: ["profiles"] });
    await queryClient.invalidateQueries({ queryKey: ["mcp", "all"] });
    await queryClient.invalidateQueries({ queryKey: ["skills"] });
    await queryClient.invalidateQueries({
      queryKey: proxyKeys.takeoverStatus,
    });
    await queryClient.invalidateQueries({ queryKey: proxyKeys.status });
    await queryClient.invalidateQueries({
      queryKey: ["providers", "claude-desktop"],
    });
  });

  useTauriEvent<SyncStatusUpdatedPayload | null | undefined>(
    WEBDAV_SYNC_STATUS_UPDATED,
    async (payload) => {
      const statusPayload = payload ?? {};
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      if (statusPayload.source !== "auto" || statusPayload.status !== "error") {
        return;
      }
      toast.error(
        t("settings.webdavSync.autoSyncFailedToast", {
          error: statusPayload.error || t("common.unknown"),
        }),
      );
    },
  );

  useTauriEvent<SyncStatusUpdatedPayload | null | undefined>(
    S3_SYNC_STATUS_UPDATED,
    async (payload) => {
      const statusPayload = payload ?? {};
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      if (statusPayload.source !== "auto" || statusPayload.status !== "error") {
        return;
      }
      toast.error(
        t("settings.s3Sync.autoSyncFailedToast", {
          error: statusPayload.error || t("common.unknown"),
        }),
      );
    },
  );

  useTauriEvent<{ appType: string; providerName: string }>(
    "proxy-official-warning",
    (payload) => {
      toast.warning(
        t("notifications.proxyOfficialWarning", {
          name: payload.providerName,
          defaultValue: `当前供应商 ${payload.providerName} 是官方供应商，建议切换到第三方供应商后再使用代理接管`,
        }),
        { duration: 8000 },
      );
    },
  );

  useEffect(() => {
    let active = true;
    let unlistenResize: (() => void) | undefined;

    const setupWindowStateSync = async () => {
      try {
        const currentWindow = getCurrentWindow();
        const syncWindowMaximizedState = async () => {
          const maximized = await currentWindow.isMaximized();
          if (active) {
            setIsWindowMaximized(maximized);
          }
        };

        await syncWindowMaximizedState();
        unlistenResize = await currentWindow.onResized(() => {
          void syncWindowMaximizedState();
        });
      } catch (error) {
        console.error("[App] Failed to sync window maximized state", error);
      }
    };

    void setupWindowStateSync();
    return () => {
      active = false;
      unlistenResize?.();
    };
  }, []);

  useEffect(() => {
    // settingsData 未加载时跳过，避免用 fallback false 覆盖 Rust 侧已设好的装饰状态
    if (!settingsData) return;

    const syncWindowDecorations = async () => {
      try {
        await getCurrentWindow().setDecorations(!useAppWindowControls);
      } catch (error) {
        console.error("[App] Failed to update window decorations", error);
      }
    };

    void syncWindowDecorations();
  }, [useAppWindowControls, settingsData]);

  useEffect(() => {
    if (!credentialRuntimeAvailable) return;
    const checkEnvOnStartup = async () => {
      try {
        const allConflicts = await checkAllEnvConflicts();
        const flatConflicts = Object.values(allConflicts).flat();

        if (flatConflicts.length > 0) {
          setEnvConflicts(flatConflicts);
          const dismissed = sessionStorage.getItem("env_banner_dismissed");
          if (!dismissed) {
            setShowEnvBanner(true);
          }
        }
      } catch (error) {
        console.error(
          "[App] Failed to check environment conflicts on startup:",
          error,
        );
      }
    };

    checkEnvOnStartup();
  }, [credentialRuntimeAvailable]);

  useEffect(() => {
    if (!credentialRuntimeAvailable) return;
    const checkMigration = async () => {
      try {
        const migrated = await invoke<boolean>("get_migration_result");
        if (migrated) {
          toast.success(
            t("migration.success", { defaultValue: "配置迁移成功" }),
            { closeButton: true },
          );
        }
      } catch (error) {
        console.error("[App] Failed to check migration result:", error);
      }
    };

    checkMigration();
  }, [t, credentialRuntimeAvailable]);

  useEffect(() => {
    if (!credentialRuntimeAvailable) return;
    const checkSkillsMigration = async () => {
      try {
        const result = await invoke<{ count: number; error?: string } | null>(
          "get_skills_migration_result",
        );
        if (result?.error) {
          toast.error(t("migration.skillsFailed"), {
            description: t("migration.skillsFailedDescription"),
            closeButton: true,
          });
          console.error("[App] Skills SSOT migration failed:", result.error);
          return;
        }
        if (result && result.count > 0) {
          toast.success(t("migration.skillsSuccess", { count: result.count }), {
            closeButton: true,
          });
          await queryClient.invalidateQueries({ queryKey: ["skills"] });
        }
      } catch (error) {
        console.error("[App] Failed to check skills migration result:", error);
      }
    };

    checkSkillsMigration();
  }, [t, queryClient, credentialRuntimeAvailable]);

  useEffect(() => {
    if (!credentialRuntimeAvailable) return;
    const checkEnvOnSwitch = async () => {
      try {
        const conflicts = await checkEnvConflicts(activeApp);

        if (conflicts.length > 0) {
          setEnvConflicts((prev) => {
            const existingKeys = new Set(
              prev.map((c) => `${c.varName}:${c.sourcePath}`),
            );
            const newConflicts = conflicts.filter(
              (c) => !existingKeys.has(`${c.varName}:${c.sourcePath}`),
            );
            return [...prev, ...newConflicts];
          });
          const dismissed = sessionStorage.getItem("env_banner_dismissed");
          if (!dismissed) {
            setShowEnvBanner(true);
          }
        }
      } catch (error) {
        console.error(
          "[App] Failed to check environment conflicts on app switch:",
          error,
        );
      }
    };

    if (usesStore) void checkEnvOnSwitch();
  }, [activeApp, usesStore, credentialRuntimeAvailable]);

  const canGoBackRef = useRef(navigation.canGoBack);
  const managementBusy =
    mcpManagementBusy ||
    skillsNavigationBusy ||
    promptNavigationBusy ||
    zcodeNavigationBusy ||
    mcodeNavigationBusy;
  const managementBusyRef = useRef(false);
  managementBusyRef.current = managementBusy;

  useEffect(() => {
    canGoBackRef.current = navigation.canGoBack;
  }, [navigation.canGoBack]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "," && (event.metaKey || event.ctrlKey)) {
        if (managementBusyRef.current) {
          event.preventDefault();
          return;
        }
        event.preventDefault();
        setCurrentView("settings");
        return;
      }

      if (event.key !== "Escape" || event.defaultPrevented) return;

      if (document.body.style.overflow === "hidden") return;

      if (!canGoBackRef.current) return;
      if (managementBusyRef.current) return;

      if (isTextEditableTarget(event.target)) return;

      event.preventDefault();
      goBack();
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  const [launchDashboardOpen, setLaunchDashboardOpen] = useState(false);
  const openHermesWebUI = useOpenHermesWebUI(() =>
    setLaunchDashboardOpen(true),
  );

  const handleOpenWebsite = async (url: string) => {
    try {
      await settingsApi.openExternal(url);
    } catch (error) {
      const detail =
        extractErrorMessage(error) ||
        t("notifications.openLinkFailed", {
          defaultValue: "链接打开失败",
        });
      toast.error(detail);
    }
  };

  const handleEditProvider = async ({
    provider,
    originalId,
    edit,
    credentialWrite,
  }: ProviderUpdateInput) => {
    if (providerWritesBlocked && !edit?.queryOnly) {
      if (edit)
        return {
          app: activeApp,
          request: edit.request,
          status: "blocked" as const,
          code: "pendingOperation" as const,
        };
      return;
    }
    if (credentialWrite === true)
      return updateProvider(provider, originalId, edit, true);
    return edit
      ? updateProvider(provider, originalId, edit)
      : updateProvider(provider, originalId);
  };

  const handleConfirmAction = async () => {
    if (!confirmAction || providerWritesBlocked) return;
    const { provider, action } = confirmAction;

    if (action === "remove") {
      // Remove from live config only (for additive mode apps like OpenCode/OpenClaw)
      // Does NOT delete from database - provider remains in the list
      try {
        await providersApi.removeFromLiveConfig(provider.id, activeApp);
      } catch (error) {
        const detail = extractErrorMessage(error);
        const description =
          activeApp === "pi"
            ? translatePiProviderMutationError(detail, t) || detail
            : detail;
        if (activeApp === "pi") {
          void invalidatePiProviderCaches(queryClient).catch(() => undefined);
        }
        toast.error(t("notifications.removeFromConfigFailed"), {
          description: description || t("common.unknown"),
          closeButton: true,
        });
        return;
      }
      if (activeApp === "pi") {
        await invalidatePiProviderCaches(queryClient);
      }
      if (activeApp === "mcode") {
        await queryClient.invalidateQueries({
          queryKey: ["providers", "mcode"],
        });
      }
      // Invalidate queries to refresh the isInConfig state
      if (activeApp === "opencode") {
        await queryClient.invalidateQueries({
          queryKey: ["opencodeLiveProviderIds"],
        });
      } else if (activeApp === "openclaw") {
        await queryClient.invalidateQueries({
          queryKey: openclawKeys.health,
        });
      }
      toast.success(
        activeApp === "pi"
          ? t("pi.provider.removed", {
              defaultValue: "已从 Pi 移除",
            })
          : t("notifications.removeFromConfigSuccess", {
              defaultValue: "已从配置移除",
            }),
        { closeButton: true },
      );
    } else {
      await deleteProvider(provider.id);
    }
    setConfirmAction(null);
  };

  const handleDuplicateProvider = async (provider: Provider) => {
    try {
      await providersApi.duplicate(provider.id, activeApp);
      await queryClient.invalidateQueries({
        queryKey: ["providers", activeApp],
      });
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.error("[App] Failed to duplicate provider", error);
      const detail = extractErrorMessage(error);
      toast.error(
        t("provider.duplicateFailed", {
          defaultValue: "复制供应商失败",
        }) + (detail ? `: ${detail}` : ""),
      );
    }
  };

  const confirmActionMessage = useMemo(() => {
    if (!confirmAction) return "";

    const message =
      confirmAction.action === "remove"
        ? t("confirm.removeProviderMessage", {
            name: confirmAction.provider.name,
          })
        : t("confirm.deleteProviderMessage", {
            name: confirmAction.provider.name,
          });
    const isPiGlobalDefault =
      activeApp === "pi" &&
      piCurrentState?.defaultProviderId === confirmAction.provider.id;

    return isPiGlobalDefault
      ? `${message}\n\n${t("confirm.piDefaultProviderWarning")}`
      : message;
  }, [activeApp, confirmAction, piCurrentState?.defaultProviderId, t]);

  const handleOpenTerminal = async (provider: Provider) => {
    try {
      const selectedDir = await settingsApi.pickDirectory();
      if (!selectedDir) {
        return;
      }

      await providersApi.openTerminal(provider.id, activeApp, {
        cwd: selectedDir,
      });
      toast.success(
        t("provider.terminalOpened", {
          defaultValue: "终端已打开",
        }),
      );
    } catch (error) {
      console.error("[App] Failed to open terminal", error);
      const errorMessage = extractErrorMessage(error);
      toast.error(
        t("provider.terminalOpenFailed", {
          defaultValue: "打开终端失败",
        }) + (errorMessage ? `: ${errorMessage}` : ""),
      );
    }
  };

  const handleImportSuccess = async () => {
    try {
      await queryClient.invalidateQueries({
        queryKey: ["providers"],
        refetchType: "all",
      });
      await queryClient.refetchQueries({
        queryKey: ["providers"],
        type: "all",
      });
    } catch (error) {
      console.error("[App] Failed to refresh providers after import", error);
      await refetch();
    }
    try {
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.error("[App] Failed to refresh tray menu", error);
    }
  };

  const notifyWindowControlError = (error: unknown) => {
    toast.error(
      t("notifications.windowControlFailed", {
        defaultValue: "窗口控制失败：{{error}}",
        error: extractErrorMessage(error),
      }),
    );
  };

  const handleWindowMinimize = async () => {
    try {
      await getCurrentWindow().minimize();
    } catch (error) {
      console.error("[App] Failed to minimize window", error);
      notifyWindowControlError(error);
    }
  };

  const handleWindowToggleMaximize = async () => {
    try {
      const currentWindow = getCurrentWindow();
      await currentWindow.toggleMaximize();
      setIsWindowMaximized(await currentWindow.isMaximized());
    } catch (error) {
      console.error("[App] Failed to toggle maximize", error);
      notifyWindowControlError(error);
    }
  };

  const handleWindowClose = async () => {
    try {
      await getCurrentWindow().close();
    } catch (error) {
      console.error("[App] Failed to close window", error);
      notifyWindowControlError(error);
    }
  };

  const handleOpenSkillsDiscovery = () => {
    setSkillsDiscoverySource("repos");
    setCurrentView("skillsDiscovery");
  };

  /** 打开聚合页；默认先选平台，首启引导显式进入中转站。 */
  const handleOpenAddHub = useCallback(
    (
      tab: AddHubTab = "accounts",
      opts?: {
        firstVisit?: boolean;
        appId?: AppId;
        intent?: ServiceEntryIntent;
      },
    ) => {
      const app =
        opts?.appId ?? (usesProviderStore(activeApp) ? activeApp : "codex");
      if (editingSessionApp.current && app !== editingSessionApp.current)
        return;
      setAddHubEntries((entries) => ({
        ...entries,
        [app]: {
          id: (entries[app]?.id ?? 0) + 1,
          tab: app === "mcode" ? "manual" : tab,
          firstVisit: opts?.firstVisit === true,
          intent: opts?.intent,
        },
      }));
      setCurrentView("addHub", app);
    },
    [activeApp, setCurrentView],
  );

  const onboarding = useServiceOnboardingStatus(credentialRuntimeAvailable);
  const onboardingOpened = useRef(false);
  useEffect(() => {
    if (
      credentialRuntimeAvailable &&
      onboarding.isSuccess &&
      !onboarding.isFetching &&
      onboarding.data?.shouldPrompt &&
      !onboardingOpened.current
    ) {
      onboardingOpened.current = true;
      handleOpenAddHub("directory", { firstVisit: true });
    }
  }, [
    credentialRuntimeAvailable,
    onboarding.isSuccess,
    onboarding.isFetching,
    onboarding.data?.shouldPrompt,
    handleOpenAddHub,
  ]);

  const navigateSection = (view: View) => {
    if (credentialRuntimeAvailable && view === "plaza") {
      void settingsApi
        .plazaSetVisible(true)
        .then(() => queryClient.invalidateQueries({ queryKey: ["settings"] }))
        .catch((error) => toast.error(String(error)));
    }
    setCurrentView(
      view,
      view === "image"
        ? "codex-image"
        : activeApp === "codex-image" ||
            (!usesStore && view !== "providers" && view !== "settings")
          ? "codex"
          : activeApp,
    );
  };

  const [visibilityBusy, setVisibilityBusy] = useState(false);
  const changeAppVisibility = async (app: AppId, visible: boolean) => {
    if (visibilityBusy) return;
    setVisibilityBusy(true);
    try {
      const nextVisibility = await settingsApi.setAppVisibility(app, visible);
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      if (visible) setActiveApp(app);
      else if (app === activeApp) {
        const next = APP_IDS.find(
          (candidate) =>
            candidate !== app &&
            candidate !== "codex-image" &&
            nextVisibility[candidate],
        );
        if (next) setActiveApp(next);
      }
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setVisibilityBusy(false);
    }
  };

  const applicationSwitcher = (
    <AppSwitcher
      activeApp={activeApp}
      variant={isC2View ? "workspace" : "default"}
      onSwitch={setActiveApp}
      visibleApps={visibleApps}
      applications={APP_IDS.filter((app) => app !== "codex-image")}
      disabled={visibilityBusy || !credentialRuntimeAvailable}
      onHideApp={(app) => void changeAppVisibility(app, false)}
      onShowApp={(app) => void changeAppVisibility(app, true)}
    />
  );

  const renderContent = () => {
    const providerList = (mutationsDisabled = false) => (
      <ProviderList
        mutationsDisabled={mutationsDisabled || providersReadFailed}
        readFailed={providersReadFailed}
        onRetryRead={() => void refetch()}
        showFailoverControls={false}
        providers={providers}
        appId={activeApp}
        isLoading={isLoading}
        isProxyRunning={currentAppUsesProxy && isProxyRunning}
        isProxyTakeover={isProxyRunning && isCurrentAppTakeoverActive}
        activeProviderId={activeProviderId}
        onSwitch={activeApp === "pi" ? handleEnablePiProvider : guardedSwitch}
        onEdit={(provider) => {
          editingSessionApp.current = activeApp;
          setEditingProvider(provider);
        }}
        onDelete={(provider) =>
          setConfirmAction({ provider, action: "delete" })
        }
        onRemoveFromConfig={
          activeApp === "opencode" ||
          activeApp === "openclaw" ||
          activeApp === "hermes" ||
          activeApp === "mcode" ||
          activeApp === "pi"
            ? (provider) =>
                setConfirmAction({
                  provider,
                  action: "remove",
                })
            : undefined
        }
        onDisableOmo={activeApp === "opencode" ? handleDisableOmo : undefined}
        onDisableOmoSlim={
          activeApp === "opencode" ? handleDisableOmoSlim : undefined
        }
        onDuplicate={handleDuplicateProvider}
        onConfigureUsage={setUsageProvider}
        onOpenWebsite={handleOpenWebsite}
        onOpenTerminal={activeApp === "claude" ? handleOpenTerminal : undefined}
        onCreate={() => handleOpenAddHub()}
        onSetAsDefault={
          activeApp === "openclaw"
            ? setAsDefaultModel
            : activeApp === "hermes"
              ? // switchProvider 的第二参是 quitChatgpt?: boolean，
                // 这里只需要 (provider) 形状，包一层避免与 modelId 签名冲突。
                (provider) => switchProvider(provider)
              : undefined
        }
      />
    );
    const content = (() => {
      switch (currentView) {
        case "sharedGroups":
          return (
            <SharedGroupsPage
              ref={workspaceRef}
              appId={isProxyAppId(activeApp) ? activeApp : "claude"}
              onOpenApplications={() => setCurrentView("providers")}
            />
          );
        case "records":
        case "resources":
          return (
            <FeatureHub
              kind={currentView}
              appId={sharedFeatureApp}
              onNavigate={setCurrentView}
              onUsage={() => setCurrentView("usage")}
              onLaunchDashboard={() => void openHermesWebUI()}
            />
          );
        case "usage":
          return <UsagePage />;
        case "settings":
          return (
            <SettingsPage
              beforeLeave={beforeNavigate}
              ref={settingsRef}
              onManageCodexMcp={() => setCurrentView("mcp", "codex")}
              onManageClaudeMcp={() => setCurrentView("mcp", "claude")}
              onManageGeminiMcp={() => setCurrentView("mcp", "gemini")}
              open={true}
              onOpenChange={goBack}
              onImportSuccess={handleImportSuccess}
              defaultTab={settingsDefaultTab}
              onManageAccounts={() => {
                setSettingsDefaultTab("general");
                setCurrentView("services");
              }}
            />
          );
        case "prompts":
          return (
            <PromptPanel
              ref={promptPanelRef}
              open={true}
              onOpenChange={goBack}
              appId={sharedFeatureApp}
              onInteractionBlockedChange={setPromptManagementBusy}
              onNavigationBlockedChange={setPromptNavigationBusy}
              onPrimaryActionChange={setPromptPrimaryAction}
            />
          );
        case "hermesMemory":
          return <HermesMemoryPanel />;
        case "skills":
          return (
            <UnifiedSkillsPanel
              ref={unifiedSkillsPanelRef}
              onOpenDiscovery={handleOpenSkillsDiscovery}
              onInteractionBlockedChange={setSkillsManagementBusy}
              onNavigationBlockedChange={setSkillsNavigationBusy}
              onCheckUpdatesStateChange={setSkillsCheckUpdatesState}
              currentApp={
                sharedFeatureApp === "openclaw" ? "claude" : sharedFeatureApp
              }
            />
          );
        case "skillsDiscovery":
          return (
            <SkillsPage
              ref={skillsPageRef}
              initialApp={
                sharedFeatureApp === "openclaw" ? "claude" : sharedFeatureApp
              }
              onSourceChange={setSkillsDiscoverySource}
            />
          );
        case "mcp":
          return (
            <UnifiedMcpPanel
              ref={mcpPanelRef}
              onOpenChange={goBack}
              onInteractionBlockedChange={setMcpManagementBusy}
            />
          );
        case "clients":
          return <ClientsPanel />;
        case "agents":
          return <AgentsPanel onOpenChange={goBack} />;
        case "universal":
          return (
            <div className="page-content">
              <UniversalProviderPanel />
            </div>
          );

        case "sessions":
          return (
            <SessionManagerPage
              key={sharedFeatureApp}
              appId={sharedFeatureApp}
            />
          );
        case "workspace":
          return <WorkspaceFilesPanel />;
        case "openclawEnv":
          return <EnvPanel />;
        case "openclawTools":
          return <ToolsPanel />;
        case "openclawAgents":
          return <AgentsDefaultsPanel />;
        default:
          return (
            <div className="page-content flex flex-col flex-1 min-h-0 gap-5 overflow-hidden">
              {activeApp !== "codex-image" && !isC2View && (
                <div className="flex min-w-0">{applicationSwitcher}</div>
              )}
              <div className="flex-1 overflow-y-auto overflow-x-hidden pb-4">
                <AnimatePresence mode="wait">
                  <motion.div
                    key={activeApp}
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    transition={{ duration: 0.15 }}
                    className="space-y-4"
                  >
                    {activeApp === "zcode" ? (
                      <ZCodeProviderPanel
                        onManageAccounts={() =>
                          setCurrentView(
                            "services",
                            activeApp,
                            undefined,
                            "zcode",
                          )
                        }
                        onNavigationBlockedChange={setZcodeNavigationBusy}
                      />
                    ) : activeApp === "mcode" ? (
                      <McodeProviderPanel onBusyChange={setMcodeNavigationBusy}>
                        {(busy) => providerList(busy)}
                      </McodeProviderPanel>
                    ) : activeApp === "codex-image" ? (
                      <>
                        <ImageTabPage onOpenAddHub={handleOpenAddHub} />
                        {providerList()}
                      </>
                    ) : isProxyAppId(activeApp) ? (
                      <ApplicationRouteWorkspace
                        ref={workspaceRef}
                        key={activeApp}
                        appId={activeApp}
                        onOpenSource={(account) => {
                          const destination = serviceSourceDestination(
                            account,
                            activeApp,
                          );
                          setCurrentView(
                            destination.view,
                            destination.app,
                            destination.account,
                            destination.platform,
                          );
                        }}
                        onManageGroups={() => setCurrentView("sharedGroups")}
                        onAdd={() => handleOpenAddHub()}
                      >
                        {(mutationsDisabled) => (
                          <>
                            {providerList(mutationsDisabled)}
                            {(activeApp === "claude" ||
                              activeApp === "codex") && (
                              <ManagedAccountApplicationPanel
                                key={activeApp}
                                appId={activeApp}
                                ref={managedAccountRef}
                              />
                            )}
                          </>
                        )}
                      </ApplicationRouteWorkspace>
                    ) : (
                      <ApplicationWorkspace
                        ref={workspaceRef}
                        key={activeApp}
                        appId={activeApp}
                        providers={providers}
                        onSwitchProvider={
                          activeApp === "pi"
                            ? handleEnablePiProvider
                            : guardedSwitch
                        }
                        onOpenAccount={(account) =>
                          setCurrentView("services", activeApp, account)
                        }
                        onAdd={() => handleOpenAddHub()}
                      >
                        {(mutationsDisabled) => providerList(mutationsDisabled)}
                      </ApplicationWorkspace>
                    )}
                  </motion.div>
                </AnimatePresence>
              </div>
            </div>
          );
      }
    })();

    return (
      <AnimatePresence mode="wait">
        <motion.div
          key={currentView}
          className="flex flex-col flex-1 min-h-0"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.2 }}
        >
          {content}
        </motion.div>
      </AnimatePresence>
    );
  };

  return (
    <div
      data-client-shell
      className={cn(
        "flex flex-col h-screen overflow-hidden selection:bg-primary/30 pl-[var(--sidebar-width)]",
        isC2View
          ? "bg-[#edf0f5] text-[#17212f] dark:bg-background dark:text-foreground"
          : "bg-background text-foreground",
      )}
      style={{
        overflowX: "hidden",
        paddingTop: currentView === "addHub" ? dragBarHeight : contentTopOffset,
      }}
    >
      {(dragBarHeight > 0 || useAppWindowControls) && (
        <div
          className="fixed top-0 left-0 right-0 z-[70] flex items-center justify-end px-2"
          data-tauri-drag-region
          style={{ WebkitAppRegion: "drag", height: dragBarHeight } as any}
        >
          {useAppWindowControls && (
            <div
              className="flex items-center gap-1"
              style={{ WebkitAppRegion: "no-drag" } as any}
            >
              <Button
                variant="ghost"
                size="icon"
                onClick={() => void handleWindowMinimize()}
                title={t("header.windowMinimize")}
                className="h-7 w-7"
              >
                <Minus className="w-4 h-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                onClick={() => void handleWindowToggleMaximize()}
                title={
                  isWindowMaximized
                    ? t("header.windowRestore")
                    : t("header.windowMaximize")
                }
                className="h-7 w-7"
              >
                {isWindowMaximized ? (
                  <Minimize2 className="w-4 h-4" />
                ) : (
                  <Maximize2 className="w-4 h-4" />
                )}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                onClick={() => void handleWindowClose()}
                title={t("header.windowClose")}
                className="h-7 w-7 hover:bg-red-500/15 hover:text-red-500"
              >
                <X className="w-4 h-4" />
              </Button>
            </div>
          )}
        </div>
      )}
      <ClientSidebar
        view={currentView}
        top={dragBarHeight}
        disabled={
          restartRequired ||
          managementBusy ||
          (editingProvider !== null && currentView === "services")
        }
        onNavigate={navigateSection}
        onHelp={() =>
          void settingsApi
            .openExternal(OFFICIAL_WEBSITE)
            .catch((e) => toast.error(extractErrorMessage(e)))
        }
        footer={
          <>
            {credentialRuntimeAvailable && <CcSwitchImportEntry />}
            <UpdateBadge
              onClick={() => {
                setSettingsDefaultTab("about");
                setCurrentView("settings");
              }}
            />
            <GitHubStarButton
              onClick={() => void settingsApi.openExternal(GITHUB_REPO)}
            />
          </>
        }
      />
      {credentialRuntimeAvailable &&
        showEnvBanner &&
        envConflicts.length > 0 && (
          <EnvWarningBanner
            conflicts={envConflicts}
            onDismiss={() => {
              setShowEnvBanner(false);
              sessionStorage.setItem("env_banner_dismissed", "true");
            }}
            onDeleted={async () => {
              try {
                const allConflicts = await checkAllEnvConflicts();
                const flatConflicts = Object.values(allConflicts).flat();
                setEnvConflicts(flatConflicts);
                if (flatConflicts.length === 0) {
                  setShowEnvBanner(false);
                }
              } catch (error) {
                console.error(
                  "[App] Failed to re-check conflicts after deletion:",
                  error,
                );
              }
            }}
          />
        )}

      <header
        hidden={currentView === "addHub"}
        className={cn(
          "fixed left-[var(--sidebar-width)] right-0 z-50 border-b border-border-default",
          isC2View ? "bg-[#f5f7fb] dark:bg-background" : "bg-background",
        )}
        {...DRAG_REGION_ATTR}
        style={
          {
            ...DRAG_REGION_STYLE,
            top: dragBarHeight,
            height: PAGE_HEADER_HEIGHT,
          } as any
        }
      >
        <div
          className={cn(
            "flex h-full items-center justify-between gap-4",
            currentView === "services"
              ? SERVICES_CONTENT_CLASS_NAME
              : "page-header",
          )}
          {...DRAG_REGION_ATTR}
          style={{ ...DRAG_REGION_STYLE } as any}
        >
          <div
            className={cn("flex items-center gap-1", isC2View && "min-w-0")}
            style={{ WebkitAppRegion: "no-drag" } as any}
          >
            {isC2View && credentialRuntimeAvailable ? (
              applicationSwitcher
            ) : !isApplicationView ? (
              <div className="flex items-center gap-2">
                {navigation.canGoBack && (
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={managementBusy}
                    onClick={goBack}
                    aria-label={t("common.back")}
                    className={cn(
                      "mr-2 rounded-lg",
                      managementBusy && "disabled:opacity-100",
                    )}
                  >
                    <ArrowLeft className="w-4 h-4" />
                    {t("common.back")}
                  </Button>
                )}
                <h1
                  className={cn(
                    "font-semibold",
                    currentView === "services" ? "text-[26px]" : "text-lg",
                  )}
                >
                  {currentView === "services" && t("client.services")}
                  {currentView === "sharedGroups" && t("client.sharedGroups")}
                  {currentView === "records" && t("client.records")}
                  {currentView === "usage" && t("usage.title")}
                  {currentView === "resources" && t("client.resources")}
                  {currentView === "plaza" && t("client.plaza")}
                  {currentView === "settings" && t("settings.title")}
                  {currentView === "prompts" &&
                    t("prompts.title", {
                      appName: t(`apps.${sharedFeatureApp}`),
                    })}
                  {currentView === "skills" && t("skills.title")}
                  {currentView === "skillsDiscovery" && t("skills.title")}
                  {currentView === "mcp" && t("mcp.unifiedPanel.title")}
                  {currentView === "clients" && t("client.features.clients")}
                  {currentView === "agents" && t("agents.title")}
                  {currentView === "universal" &&
                    t("universalProvider.title", {
                      defaultValue: "统一供应商",
                    })}
                  {currentView === "sessions" && t("sessionManager.title")}
                  {currentView === "workspace" && t("workspace.title")}
                  {currentView === "openclawEnv" && t("openclaw.env.title")}
                  {currentView === "openclawTools" && t("openclaw.tools.title")}
                  {currentView === "openclawAgents" &&
                    t("openclaw.agents.title")}
                  {currentView === "hermesMemory" && t("hermes.memory.title")}
                </h1>
              </div>
            ) : (
              <div className="flex items-center gap-3">
                <h1 className="text-lg font-semibold">
                  {t(
                    activeApp === "codex-image"
                      ? "client.image"
                      : "client.applications",
                  )}
                </h1>
              </div>
            )}
          </div>

          {credentialRuntimeAvailable && (
            <div
              className={cn(
                "flex min-w-0 items-center justify-end gap-1.5",
                isC2View ? "shrink-0" : "flex-1",
              )}
            >
              {currentView === "providers" &&
                (activeApp === "claude-desktop" || proxyAppId) && (
                  <div
                    className="flex shrink-0 items-center gap-1.5"
                    style={{ WebkitAppRegion: "no-drag" } as any}
                  >
                    {activeApp === "claude-desktop" ? (
                      <ClaudeDesktopRouteToggle />
                    ) : proxyAppId ? (
                      <>
                        {settingsData?.enableLocalProxy && (
                          <ProxyToggle activeApp={proxyAppId} />
                        )}
                      </>
                    ) : null}
                  </div>
                )}
              {currentView === "providers" &&
                usesStore &&
                (settingsData?.showProfileSwitcher ?? false) && (
                  <div
                    className="flex shrink-0 items-center"
                    style={{ WebkitAppRegion: "no-drag" } as any}
                  >
                    <ProfileSwitcher activeApp={activeApp} />
                  </div>
                )}
              {/* 固定右端：主操作（添加供应商等）shrink-0，任何配置下不被挤出 */}
              <div className="flex shrink-0 items-center py-4">
                <div
                  className="flex shrink-0 items-center gap-1.5"
                  style={{ WebkitAppRegion: "no-drag" } as any}
                >
                  {currentView === "prompts" && promptPrimaryAction && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={promptManagementBusy}
                      onClick={() => promptPanelRef.current?.openAdd()}
                      className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                    >
                      <Plus className="w-4 h-4 mr-2" />
                      {t(
                        promptPrimaryAction === "template"
                          ? "pi.prompts.newTemplate"
                          : "prompts.add",
                      )}
                    </Button>
                  )}
                  {currentView === "mcp" && (
                    <>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={mcpManagementBusy}
                        onClick={() => mcpPanelRef.current?.openImport()}
                        className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                      >
                        <Download className="w-4 h-4 mr-2" />
                        {t("mcp.importExisting")}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={mcpManagementBusy}
                        onClick={() => mcpPanelRef.current?.openAdd()}
                        className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                      >
                        <Plus className="w-4 h-4 mr-2" />
                        {t("mcp.addMcp")}
                      </Button>
                    </>
                  )}
                  {currentView === "skills" && (
                    <>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={
                          skillsManagementBusy ||
                          skillsCheckUpdatesState.isChecking ||
                          !skillsCheckUpdatesState.hasSkills
                        }
                        onClick={() =>
                          unifiedSkillsPanelRef.current?.checkUpdates()
                        }
                        className={cn(
                          "hover:bg-black/5 dark:hover:bg-white/5",
                          skillsManagementBusy && "disabled:opacity-100",
                        )}
                      >
                        {skillsCheckUpdatesState.isChecking ? (
                          <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                        ) : (
                          <RefreshCw className="w-4 h-4 mr-2" />
                        )}
                        {skillsCheckUpdatesState.isChecking
                          ? t("skills.checkingUpdates")
                          : t("skills.checkUpdates")}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={skillsManagementBusy}
                        onClick={() =>
                          unifiedSkillsPanelRef.current?.openRestoreFromBackup()
                        }
                        className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                      >
                        <History className="w-4 h-4 mr-2" />
                        {t("skills.restoreFromBackup.button")}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={skillsManagementBusy}
                        onClick={() =>
                          unifiedSkillsPanelRef.current?.openInstallFromZip()
                        }
                        className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                      >
                        <FolderArchive className="w-4 h-4 mr-2" />
                        {t("skills.installFromZip.button")}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={skillsManagementBusy}
                        onClick={() =>
                          unifiedSkillsPanelRef.current?.openImport()
                        }
                        className="relative hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                        title={
                          hasUnmanagedSkills
                            ? t("skills.unmanagedAvailable")
                            : undefined
                        }
                      >
                        <Download className="w-4 h-4 mr-2" />
                        {t("skills.import")}
                        {hasUnmanagedSkills && (
                          <span
                            className="absolute top-1 right-1 h-2 w-2 rounded-full bg-green-500"
                            aria-hidden="true"
                          />
                        )}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={skillsManagementBusy}
                        onClick={() =>
                          unifiedSkillsPanelRef.current?.openDiscovery()
                        }
                        className="hover:bg-black/5 disabled:opacity-100 dark:hover:bg-white/5"
                      >
                        <Search className="w-4 h-4 mr-2" />
                        {t("skills.discover")}
                      </Button>
                    </>
                  )}
                  {currentView === "skillsDiscovery" && (
                    <>
                      {getSkillsPageHeaderActions(skillsDiscoverySource).map(
                        ({ key, labelKey, Icon, execute }) => (
                          <Button
                            key={key}
                            variant="ghost"
                            size="sm"
                            onClick={() => execute(skillsPageRef.current)}
                            className="hover:bg-black/5 dark:hover:bg-white/5"
                          >
                            <Icon className="w-4 h-4 mr-2" />
                            {t(labelKey)}
                          </Button>
                        ),
                      )}
                    </>
                  )}
                  {currentView === "providers" && usesStore && (
                    <>
                      <Button
                        onClick={() => handleOpenAddHub()}
                        size="sm"
                        className="ml-2 gap-1.5"
                        aria-label={t("loongport.addEntry.title")}
                        title={t("loongport.addEntry.title")}
                      >
                        <Plus className="w-4 h-4" />
                        {t("client.addService")}
                      </Button>
                    </>
                  )}
                </div>
              </div>
            </div>
          )}
        </div>
      </header>

      <main
        id="main-content"
        tabIndex={-1}
        className="flex-1 min-h-0 flex flex-col overflow-y-auto animate-fade-in focus:outline-none"
      >
        {!credentialRuntimeAvailable ? (
          <div className="page-content space-y-5">
            {!restartRequired && (
              <p role="status" className="rounded-lg border p-4 text-sm">
                {t(
                  runtime.isPending || protection.isPending
                    ? "common.loading"
                    : "secrets.runtimeUnavailable",
                )}
              </p>
            )}
            {!runtime.isPending && <CredentialProtectionEntry showInline />}
            {currentView === "settings" && <ThemeSettings />}
          </div>
        ) : (
          <>
            {isOpenClawView && openclawHealthWarnings.length > 0 && (
              <OpenClawHealthBanner warnings={openclawHealthWarnings} />
            )}
            <PreservedView active={currentView === "services"}>
              <ServicesPage
                appId={activeApp}
                account={navigation.account}
                platform={navigation.platform}
                onOpenPlatform={(platform) =>
                  setCurrentView("services", activeApp, undefined, platform)
                }
                onNavigationBlockedChange={setZcodeNavigationBusy}
                onBack={navigation.back}
                onSelectAccount={(account, app) =>
                  setCurrentView("services", app, account)
                }
                onOpenAddHub={handleOpenAddHub}
                onOpenApp={(app) =>
                  setCurrentView(
                    app === "codex-image" ? "image" : "providers",
                    app,
                  )
                }
              />
            </PreservedView>
            {PROVIDER_STORE_APP_IDS.map((app) => (
              <PreservedView
                key={`add-${app}`}
                active={currentView === "addHub" && activeApp === app}
              >
                <AddHubPage
                  sourceAppId={app}
                  entryRequest={addHubEntries[app]}
                  onOpenPlatform={(platform) =>
                    setCurrentView("services", app, undefined, platform)
                  }
                  firstVisit={addHubEntries[app]?.firstVisit}
                  onBack={goBack}
                  onOpenApp={(targetApp) =>
                    setCurrentView(
                      targetApp === "codex-image" ? "image" : "providers",
                      targetApp,
                    )
                  }
                  onAddProvider={addProvider}
                />
              </PreservedView>
            ))}
            {PROVIDER_STORE_APP_IDS.map((app) => (
              <PreservedView
                key={`plaza-${app}`}
                active={currentView === "plaza" && activeApp === app}
              >
                <RelayDirectoryConnectionPage
                  sourceAppId={app}
                  onBack={goBack}
                />
              </PreservedView>
            ))}
            {!["services", "addHub", "plaza"].includes(currentView) &&
              renderContent()}
          </>
        )}
      </main>

      {credentialRuntimeAvailable && (
        <>
          <CredentialProtectionEntry />
          <ConfirmDialog
            isOpen={leaveDraftOpen}
            title={t("applications.leaveDraftTitle")}
            message={t("applications.leaveDraftDescription")}
            cancelText={t("loongport.tier.editConfirmButton")}
            confirmText={t("applications.discardAndLeave")}
            onCancel={cancelLeaveDraft}
            onConfirm={() => {
              const proceed = pendingNavigation.current;
              cancelLeaveDraft();
              if (!proceed) return;
              if (
                workspaceRef.current?.submitting ||
                settingsRef.current?.submitting ||
                managedAccountRef.current?.submitting
              ) {
                toast.info(t("common.saving"));
                return;
              }
              workspaceRef.current?.discard();
              settingsRef.current?.discard();
              managedAccountRef.current?.discard();
              proceed();
            }}
          />
          <PreservedView active={currentView !== "services"}>
            <EditProviderDialog
              mutationsDisabled={providerWritesBlocked}
              open={Boolean(editingProvider)}
              provider={effectiveEditingProvider}
              onOpenChange={(open) => {
                if (!open) {
                  editingSessionApp.current = null;
                  setEditingProvider(null);
                }
              }}
              onSubmit={handleEditProvider}
              appId={activeApp}
              isProxyTakeover={isCurrentAppTakeoverActive}
            />
          </PreservedView>

          {effectiveUsageProvider && (
            <UsageScriptModal
              mutationsDisabled={providerWritesBlocked}
              key={effectiveUsageProvider.id}
              provider={effectiveUsageProvider}
              appId={activeApp}
              isOpen={Boolean(usageProvider)}
              onClose={() => setUsageProvider(null)}
              onSave={(script) => {
                if (usageProvider && !providerWritesBlocked) {
                  void saveUsageScript(usageProvider, script);
                }
              }}
            />
          )}

          <ConfirmDialog
            confirmDisabled={providerWritesBlocked}
            isOpen={Boolean(confirmAction)}
            title={
              confirmAction?.action === "remove"
                ? t("confirm.removeProvider")
                : t("confirm.deleteProvider")
            }
            message={confirmActionMessage}
            onConfirm={() => void handleConfirmAction()}
            onCancel={() => setConfirmAction(null)}
          />

          <ConfirmDialog
            isOpen={launchDashboardOpen}
            title={t("hermes.webui.launchConfirmTitle")}
            message={t("hermes.webui.launchConfirmMessage")}
            confirmText={t("hermes.webui.launchConfirmAction")}
            variant="info"
            onConfirm={() => {
              setLaunchDashboardOpen(false);
              void (async () => {
                try {
                  await hermesApi.launchDashboard();
                  toast.success(t("hermes.webui.launching"));
                } catch (error) {
                  toast.error(t("hermes.webui.launchFailed"), {
                    description: extractErrorMessage(error) || undefined,
                  });
                }
              })();
            }}
            onCancel={() => setLaunchDashboardOpen(false)}
          />

          <DeepLinkImportDialog />

          {/* 远端公告：状态机在组件内，这里只挂一行。无公告时零存在感。 */}
          <AnnouncementDialog />

          {/* 模型对齐告警横幅：客户端模型 ≠ 档位已选模型（实际计费模型）时常驻
          告知 + 一键出口。状态机在组件内，这里只挂一行；无不符时零存在感。 */}
          <ModelMismatchBanner />

          {/* 切 codex 供应商前的「要不要退 ChatGPT」确认框。它由 `useCodexSwitchGuard`
          持有全部状态（弹不弹、切哪个），这里只挂一行 —— 与上面那个同一个模式。 */}
          {switchDialog}
        </>
      )}

      {/* **这里没有首启欢迎弹窗，是有意的。** 上游那个（`FirstRunNoticeDialog`）讲的是
          CC Switch 怎么在多个供应商之间切换，与 LoongPort 的流程无关；而且它和域名输入
          弹窗都是「flag 未确认就弹」+ zIndex=top，会叠在一起，用户得先看完一屏无关介绍
          才能摸到真正要填的东西。2026-08-04 连组件带 i18n 一起删了（一直未挂载）。
          LoongPort 的首启入口是 provider 页顶部那一区的「添加站点」。 */}
    </div>
  );
}

export default App;
