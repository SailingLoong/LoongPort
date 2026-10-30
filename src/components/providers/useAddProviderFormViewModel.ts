import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { Provider, CustomEndpoint, UniversalProvider } from "@/types";
import { universalProvidersApi } from "@/lib/api";
import type { ProviderFormValues } from "@/components/providers/forms/ProviderForm";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { providerPresets } from "@/config/claudeProviderPresets";
import { codexProviderPresets } from "@/config/codexProviderPresets";
import { geminiProviderPresets } from "@/config/geminiProviderPresets";
import { claudeDesktopProviderPresets } from "@/config/claudeDesktopProviderPresets";
import { extractCodexBaseUrl } from "@/utils/providerConfigUtils";
import { extractGrokBuildBaseUrl } from "@/utils/grokBuildConfig";
import { GROKBUILD_OFFICIAL_PROVIDER_ID } from "@/utils/providerCapabilities";
import type { OpenClawSuggestedDefaults } from "@/config/openclawProviderPresets";
import type { UniversalProviderPreset } from "@/config/universalProviderPresets";
import type { ManagedAuthProvider } from "@/lib/api";
import type { AddProviderFormProps } from "./AddProviderForm";

export function useAddProviderFormViewModel({
  appId,
  onSubmit,
  onDone,
  onSaveStart,
}: AddProviderFormProps) {
  const { t } = useTranslation();
  const viewActive = usePreservedViewActive();
  const ownershipToken = useMemo(
    () => Symbol("add-provider-owner"),
    [appId, viewActive],
  );
  const owner = useRef({
    token: ownershipToken,
    active: viewActive,
    live: false,
  });
  owner.current.token = ownershipToken;
  owner.current.active = viewActive;
  useEffect(() => {
    owner.current.live = true;
    return () => {
      owner.current.live = false;
    };
  }, [ownershipToken]);
  const admitted = () =>
    owner.current.live &&
    owner.current.active &&
    owner.current.token === ownershipToken;
  // OpenCode and OpenClaw don't support universal providers
  const showUniversalTab =
    appId !== "opencode" &&
    appId !== "openclaw" &&
    appId !== "hermes" &&
    appId !== "pi" &&
    appId !== "mcode" &&
    appId !== "grokbuild" &&
    appId !== "claude-desktop";
  const [activeTab, setActiveTab] = useState<"app-specific" | "universal">(
    "app-specific",
  );
  const activeTabRef = useRef(activeTab);
  const [universalDraft, setUniversalDraft] = useState<{
    token: symbol;
    generation: number;
  } | null>(null);
  const universalDraftRef = useRef(universalDraft);
  const universalFormOpen = universalDraft !== null;
  const appDraft = useMemo(() => Symbol("app-provider-draft"), [appId]);
  const [savedAppDraft, setSavedAppDraft] = useState<symbol | null>(null);
  const savedAppDraftRef = useRef<symbol | null>(null);
  const appSaveCompleted = savedAppDraft === appDraft;
  const [appSaveInFlight, setAppSaveInFlight] = useState(false);
  const draftGeneration = useRef(0);
  const commandGeneration = draftGeneration.current;
  const sameDraft = () =>
    owner.current.active &&
    owner.current.token === ownershipToken &&
    commandGeneration === draftGeneration.current;
  const admittedDraft = () => admitted() && sameDraft();
  const acceptedUniversal = useRef<{
    token: symbol;
    generation: number;
  } | null>(null);
  const pendingAppSave = useRef<{ token: symbol; generation: number } | null>(
    null,
  );
  const [selectedUniversalPreset, setSelectedUniversalPreset] =
    useState<UniversalProviderPreset | null>(null);
  const [formSubmitting, setIsFormSubmittingState] = useState(false);
  const isFormSubmitting = formSubmitting || appSaveInFlight;
  const setIsFormSubmitting = useCallback(
    (value: boolean) => {
      if (sameDraft()) setIsFormSubmittingState(value);
    },
    [ownershipToken, commandGeneration],
  );
  // 托管 OAuth 账号的面板目标（上游 #3879 移植）：聚合页没有弹窗 open 翻转，
  // 只在切换 app 标签时收起；页面卸载自然重置。
  const [authSettingsTarget, setAuthSettingsTargetState] =
    useState<ManagedAuthProvider | null>(null);
  const setAuthSettingsTarget = useCallback(
    (value: ManagedAuthProvider | null) => {
      if (sameDraft()) setAuthSettingsTargetState(value);
    },
    [ownershipToken, commandGeneration],
  );
  useEffect(() => {
    setAuthSettingsTargetState(null);
  }, [appId]);
  // 表单随聚合页标签的挂/卸载自然重置，没有弹窗 open 翻转那回事 —— token 只跟 appId。
  const formReadyToken = useMemo(() => Symbol("provider-form-ready"), [appId]);
  const currentFormReadyToken = useRef(formReadyToken);
  currentFormReadyToken.current = formReadyToken;
  const [formReadyState, setFormReadyState] = useState({
    token: formReadyToken,
    ready: appId !== "pi",
  });
  const isFormReady =
    formReadyState.token === formReadyToken
      ? formReadyState.ready
      : appId !== "pi";
  const handleSubmitReadyChange = useCallback(
    (ready: boolean) => {
      if (sameDraft() && currentFormReadyToken.current === formReadyToken) {
        setFormReadyState({ token: formReadyToken, ready });
      }
    },
    [formReadyToken, ownershipToken, commandGeneration],
  );

  const handleUniversalProviderSave = useCallback(
    async (provider: UniversalProvider) => {
      if (
        !admittedDraft() ||
        activeTabRef.current !== "universal" ||
        universalDraftRef.current?.generation !== commandGeneration
      )
        return;
      if (acceptedUniversal.current?.generation === commandGeneration) return;
      acceptedUniversal.current = {
        token: ownershipToken,
        generation: commandGeneration,
      };
      const saveEpoch = onSaveStart?.();
      const savedDraftGeneration = draftGeneration.current;
      try {
        await universalProvidersApi.upsert(provider);
      } catch (error) {
        console.error(
          "[AddProviderForm] Failed to save universal provider",
          error,
        );
        if (admittedDraft())
          toast.error(
            t("universalProvider.addFailed", {
              defaultValue: "统一供应商添加失败",
            }),
          );
        return;
      }

      try {
        await universalProvidersApi.sync(provider.id);
        if (admittedDraft())
          toast.success(
            t("universalProvider.addedAndSynced", {
              defaultValue: "统一供应商已添加并同步",
            }),
          );
      } catch (error) {
        console.error(
          "[AddProviderForm] Provider saved but sync failed",
          error,
        );
        if (admittedDraft())
          toast.warning(
            t("universalProvider.addedButSyncFailed", {
              defaultValue: "统一供应商已添加，但同步失败",
            }),
          );
      }

      // The modal closes synchronously on submit. A late save must not
      // close a subsequently opened draft or navigate away from it.
      if (admittedDraft() && savedDraftGeneration === draftGeneration.current) {
        onDone("saved", saveEpoch);
      }
    },
    [t, onDone, onSaveStart, ownershipToken, commandGeneration],
  );

  const handleUniversalFormClose = useCallback(() => {
    if (!admittedDraft()) return;
    // A save hands off before the modal closes. A plain close retires its draft.
    if (acceptedUniversal.current?.generation !== commandGeneration) {
      ++draftGeneration.current;
    }
    universalDraftRef.current = null;
    setUniversalDraft(null);
    setSelectedUniversalPreset(null);
  }, [ownershipToken, commandGeneration]);

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      if (
        !admittedDraft() ||
        (showUniversalTab && activeTabRef.current !== "app-specific") ||
        pendingAppSave.current ||
        savedAppDraftRef.current === appDraft
      )
        return;
      const parsedConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;

      // 构造基础提交数据
      const providerData: Omit<Provider, "id"> & {
        providerKey?: string;
        suggestedDefaults?: OpenClawSuggestedDefaults;
        ensureClaudeDesktopOfficialSeed?: boolean;
        ensureGrokBuildOfficialSeed?: boolean;
      } = {
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        ...(values.meta ? { meta: values.meta } : {}),
      };
      if (appId === "claude-desktop" && values.presetId) {
        const presetIndex = parseInt(
          values.presetId.replace("claude-desktop-", ""),
        );
        const preset = claudeDesktopProviderPresets[presetIndex];
        providerData.ensureClaudeDesktopOfficialSeed =
          values.presetCategory === "official" &&
          preset?.category === "official";
      }

      if (appId === "grokbuild" && values.presetId) {
        providerData.ensureGrokBuildOfficialSeed =
          values.presetCategory === "official" &&
          values.presetId === GROKBUILD_OFFICIAL_PROVIDER_ID;
      }

      // Apps whose native catalog has a stable provider key use it as the
      // managed provider identity.
      if (
        (appId === "opencode" ||
          appId === "openclaw" ||
          appId === "hermes" ||
          appId === "pi" ||
          appId === "mcode") &&
        values.providerKey
      ) {
        providerData.providerKey = values.providerKey;
      }
      const hasCustomEndpoints =
        providerData.meta?.custom_endpoints &&
        Object.keys(providerData.meta.custom_endpoints).length > 0;

      if (!hasCustomEndpoints && values.presetCategory !== "omo") {
        const urlSet = new Set<string>();

        const addUrl = (rawUrl?: string) => {
          const url = (rawUrl || "").trim().replace(/\/+$/, "");
          if (url && url.startsWith("http")) {
            urlSet.add(url);
          }
        };

        if (values.presetId) {
          if (appId === "claude") {
            const presets = providerPresets;
            const presetIndex = parseInt(
              values.presetId.replace("claude-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (preset?.endpointCandidates) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "codex") {
            const presets = codexProviderPresets;
            const presetIndex = parseInt(values.presetId.replace("codex-", ""));
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "gemini") {
            const presets = geminiProviderPresets;
            const presetIndex = parseInt(
              values.presetId.replace("gemini-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "claude-desktop") {
            const presets = claudeDesktopProviderPresets;
            const presetIndex = parseInt(
              values.presetId.replace("claude-desktop-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
              addUrl(preset.baseUrl);
            }
          }
        }

        if (appId === "claude") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.ANTHROPIC_BASE_URL) {
            addUrl(env.ANTHROPIC_BASE_URL);
          }
        } else if (appId === "claude-desktop") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.ANTHROPIC_BASE_URL) {
            addUrl(env.ANTHROPIC_BASE_URL);
          }
        } else if (appId === "codex") {
          const config = parsedConfig.config as string | undefined;
          if (config) {
            const extractedBaseUrl = extractCodexBaseUrl(config);
            if (extractedBaseUrl) {
              addUrl(extractedBaseUrl);
            }
          }
        } else if (appId === "gemini") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.GOOGLE_GEMINI_BASE_URL) {
            addUrl(env.GOOGLE_GEMINI_BASE_URL);
          }
        } else if (appId === "grokbuild") {
          const config = parsedConfig.config as string | undefined;
          if (config) {
            addUrl(extractGrokBuildBaseUrl(config));
          }
        } else if (appId === "opencode") {
          const options = parsedConfig.options as
            Record<string, any> | undefined;
          if (options?.baseURL) {
            addUrl(options.baseURL);
          }
        } else if (appId === "openclaw") {
          // OpenClaw uses baseUrl directly
          if (parsedConfig.baseUrl) {
            addUrl(parsedConfig.baseUrl as string);
          }
        } else if (appId === "hermes") {
          if (parsedConfig.base_url) {
            addUrl(parsedConfig.base_url as string);
          }
        }

        const urls = Array.from(urlSet);
        if (urls.length > 0) {
          const now = Date.now();
          const customEndpoints: Record<string, CustomEndpoint> = {};
          urls.forEach((url) => {
            customEndpoints[url] = {
              url,
              addedAt: now,
              lastUsed: undefined,
            };
          });

          providerData.meta = {
            ...(providerData.meta ?? {}),
            custom_endpoints: customEndpoints,
          };
        }
      }

      // OpenClaw: pass suggestedDefaults for model registration
      if (appId === "openclaw" && values.suggestedDefaults) {
        providerData.suggestedDefaults = values.suggestedDefaults;
      }

      const submission = {
        token: ownershipToken,
        generation: commandGeneration,
      };
      pendingAppSave.current = submission;
      setAppSaveInFlight(true);
      try {
        const saveEpoch = onSaveStart?.();
        await onSubmit(providerData);
        savedAppDraftRef.current = appDraft;
        setSavedAppDraft(appDraft);
        if (admittedDraft()) {
          ++draftGeneration.current;
          onDone("saved", saveEpoch);
        }
      } finally {
        if (pendingAppSave.current === submission) {
          pendingAppSave.current = null;
          setAppSaveInFlight(false);
        }
      }
    },
    [
      appId,
      onSubmit,
      onDone,
      onSaveStart,
      ownershipToken,
      commandGeneration,
      showUniversalTab,
      appDraft,
      appSaveCompleted,
    ],
  );

  const cancel = () => {
    if (!admittedDraft()) return;
    ++draftGeneration.current;
    onDone("cancel");
  };
  const openUniversalForm = () => {
    if (
      !admittedDraft() ||
      !showUniversalTab ||
      activeTabRef.current !== "universal"
    )
      return;
    const draft = {
      token: ownershipToken,
      generation: ++draftGeneration.current,
    };
    universalDraftRef.current = draft;
    setUniversalDraft(draft);
  };
  const changeTab = (tab: "app-specific" | "universal") => {
    if (!admittedDraft()) return;
    ++draftGeneration.current;
    activeTabRef.current = tab;
    setActiveTab(tab);
  };
  return {
    showUniversalTab,
    activeTab,
    universalFormOpen,
    appSaveCompleted,
    selectedUniversalPreset,
    isFormSubmitting,
    setIsFormSubmitting,
    authSettingsTarget,
    setAuthSettingsTarget,
    isFormReady,
    handleSubmitReadyChange,
    handleUniversalProviderSave,
    handleUniversalFormClose,
    handleSubmit,
    cancel,
    openUniversalForm,
    changeTab,
  };
}
