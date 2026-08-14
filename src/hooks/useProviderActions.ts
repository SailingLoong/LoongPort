import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { piApi, providersApi, openclawApi, type AppId } from "@/lib/api";
import type {
  Provider,
  UsageScript,
  OpenClawProviderConfig,
  OpenClawDefaultModel,
} from "@/types";
import type { OpenClawSuggestedDefaults } from "@/config/openclawProviderPresets";
import { injectCodingPlanUsageScript } from "@/config/codingPlanProviders";
import {
  useAddProviderMutation,
  useUpdateProviderMutation,
  useDeleteProviderMutation,
  useSwitchProviderMutation,
} from "@/lib/query";
import { usageKeys } from "@/lib/query/usage";
import { extractErrorMessage } from "@/utils/errorUtils";
import { openclawKeys } from "@/hooks/useOpenClaw";
import type { ProviderRoutingReason } from "@/types";

/**
 * Hook for managing provider actions (add, update, delete, switch)
 * Extracts business logic from App.tsx
 */
export function useProviderActions(activeApp: AppId) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const addProviderMutation = useAddProviderMutation(activeApp);
  const updateProviderMutation = useUpdateProviderMutation(activeApp);
  const deleteProviderMutation = useDeleteProviderMutation(activeApp);
  const switchProviderMutation = useSwitchProviderMutation(activeApp);

  // 添加供应商
  const addProvider = useCallback(
    async (
      provider: Omit<Provider, "id"> & {
        providerKey?: string;
        suggestedDefaults?: OpenClawSuggestedDefaults;
        addToLive?: boolean;
        ensureClaudeDesktopOfficialSeed?: boolean;
        ensureCodexOfficialSeed?: boolean;
        ensureGrokBuildOfficialSeed?: boolean;
      },
    ) => {
      const enhanced = injectCodingPlanUsageScript(activeApp, provider);
      await addProviderMutation.mutateAsync(enhanced);

      // OpenClaw: register models to allowlist after adding provider
      if (activeApp === "openclaw" && provider.suggestedDefaults) {
        const { model, modelCatalog } = provider.suggestedDefaults;
        let modelsRegistered = false;

        try {
          // 1. Merge model catalog (allowlist)
          if (modelCatalog && Object.keys(modelCatalog).length > 0) {
            const existingCatalog = (await openclawApi.getModelCatalog()) || {};
            const mergedCatalog = { ...existingCatalog, ...modelCatalog };
            await openclawApi.setModelCatalog(mergedCatalog);
            await queryClient.invalidateQueries({
              queryKey: openclawKeys.health,
            });
            modelsRegistered = true;
          }

          // 2. Set default model (only if not already set)
          if (model) {
            const existingDefault = await openclawApi.getDefaultModel();
            if (!existingDefault?.primary) {
              await openclawApi.setDefaultModel(model);
              await queryClient.invalidateQueries({
                queryKey: openclawKeys.health,
              });
            }
          }

          // Show success toast if models were registered
          if (modelsRegistered) {
            toast.success(
              t("notifications.openclawModelsRegistered", {
                defaultValue: "模型已注册到 /model 列表",
              }),
              { closeButton: true },
            );
          }
        } catch (error) {
          // Log warning but don't block main flow - provider config is already saved
          console.warn(
            "[OpenClaw] Failed to register models to allowlist:",
            error,
          );
        }
      }
    },
    [addProviderMutation, activeApp, queryClient, t],
  );

  // 更新供应商
  const updateProvider = useCallback(
    async (provider: Provider, originalId?: string) => {
      await updateProviderMutation.mutateAsync({
        provider,
        originalId,
      });

      // 更新托盘菜单（失败不影响主操作）
      try {
        await providersApi.updateTrayMenu();
      } catch (trayError) {
        console.error(
          "Failed to update tray menu after updating provider",
          trayError,
        );
      }
    },
    [updateProviderMutation],
  );

  // 切换供应商
  /**
   * 切换供应商。
   *
   * `quitChatgpt` 由调用方在弹过确认框、用户同意之后传 true —— 它让后端走
   * 「退 ChatGPT → 切 → 重开」那套编排（与 LoongPort 档位切换共用同一份实现）。
   *
   * **为什么 codex 也需要这个**：ChatGPT 桌面版自带一份 codex 核心、与命令行 codex
   * 共用同一个 `~/.codex`。它在跑的时候切任何 codex 供应商（不只是 LoongPort 的托管
   * 档位）都有同一个问题：它启动时读了旧 `config.toml` 不重启就仍连旧的，而且**它退出
   * 时会回写那个文件**、可能把刚写的覆盖掉。
   *
   * 不传 = 不碰 ChatGPT。托盘快切 / deeplink 导入没有弹确认框的机会，而未经用户同意
   * 就关掉他正开着的 app 是不能接受的。
   */
  const switchProvider = useCallback(
    async (provider: Provider, quitChatgpt?: boolean) => {
      try {
        const result = await switchProviderMutation.mutateAsync({
          providerId: provider.id,
          quitChatgpt,
        });
        if (result.status === "confirmationRequired") {
          return result;
        }
        const routingReason = result.routingNotice
          ? routingReasonText(result.routingNotice, t)
          : null;
        if (routingReason) {
          toast.warning(
            t("notifications.proxyRequiredForSwitch", {
              reason: routingReason,
              defaultValue:
                "此供应商{{reason}}，需要代理服务才能正常使用，请先启动代理",
            }),
          );
        }

        if (result?.warnings) {
          for (const warning of result.warnings) {
            toast.warning(warning);
          }
        }

        // 若已弹过 proxyRequired 警告则不再弹 success
        if (!routingReason) {
          let messageKey = "notifications.switchSuccess";
          let defaultMessage = "切换成功！";
          if (activeApp === "codex") {
            messageKey = "notifications.codexRestartRequired";
            defaultMessage = "切换成功，请重启客户端以生效";
          } else if (activeApp === "grokbuild") {
            messageKey = "notifications.grokBuildRestartRequired";
            defaultMessage = "切换成功，请重启 Grok Build 以生效";
          } else if (activeApp === "claude-desktop") {
            if (provider.meta?.claudeDesktopMode === "proxy") {
              messageKey = "notifications.claudeDesktopProxyRestartRequired";
              defaultMessage =
                "切换成功，请保持 LoongPort 运行，并重启 Claude Desktop 后生效";
            } else {
              messageKey = "notifications.claudeDesktopRestartRequired";
              defaultMessage = "切换成功，重启 Claude Desktop 后生效";
            }
          } else if (activeApp === "opencode" || activeApp === "openclaw") {
            messageKey = "notifications.addToConfigSuccess";
            defaultMessage = "已添加到配置";
          }
          toast.success(t(messageKey, { defaultValue: defaultMessage }), {
            closeButton: true,
          });
        }
        return result;
      } catch {
        // 错误提示由 mutation 处理
        return undefined;
      }
    },
    [switchProviderMutation, activeApp, t],
  );

  // 删除供应商
  const deleteProvider = useCallback(
    async (id: string) => {
      await deleteProviderMutation.mutateAsync(id);
    },
    [deleteProviderMutation],
  );

  // 保存用量脚本
  const saveUsageScript = useCallback(
    async (provider: Provider, script: UsageScript) => {
      try {
        const updatedProvider: Provider = {
          ...provider,
          meta: {
            ...provider.meta,
            usage_script: script,
          },
        };

        if (activeApp === "pi") {
          await piApi.updateProviderUsageScript(provider.id, script);
        } else {
          await providersApi.update(updatedProvider, activeApp);
        }
        await queryClient.invalidateQueries({
          queryKey: ["providers", activeApp],
        });
        // 🔧 保存用量脚本后，也应该失效该 provider 的用量查询缓存
        // 这样主页列表会使用新配置重新查询，而不是使用测试时的缓存
        await queryClient.invalidateQueries({
          queryKey: usageKeys.script(provider.id, activeApp),
        });
        await queryClient.invalidateQueries({
          queryKey: ["subscription", "quota", activeApp],
        });
        toast.success(
          t("provider.usageSaved", {
            defaultValue: "用量查询配置已保存",
          }),
          { closeButton: true },
        );
      } catch (error) {
        const detail =
          extractErrorMessage(error) ||
          t("provider.usageSaveFailed", {
            defaultValue: "用量查询配置保存失败",
          });
        toast.error(detail);
      }
    },
    [activeApp, queryClient, t],
  );

  // Set provider as default model (OpenClaw only)
  const setAsDefaultModel = useCallback(
    async (provider: Provider, modelId?: string) => {
      const config = provider.settingsConfig as OpenClawProviderConfig;
      if (!config.models || config.models.length === 0) {
        toast.error(
          t("notifications.openclawNoModels", {
            defaultValue: "该供应商没有配置模型",
          }),
        );
        return;
      }

      const selectedModel = modelId
        ? config.models.find((model) => model.id === modelId)
        : config.models[0];
      if (!selectedModel) {
        toast.error(
          t("notifications.openclawModelNotFound", {
            defaultValue: "所选模型已不存在，请刷新后重试",
          }),
        );
        return;
      }

      try {
        const primary = `${provider.id}/${selectedModel.id}`;
        const existingDefault = await openclawApi.getDefaultModel();
        const model: OpenClawDefaultModel = {
          ...(existingDefault ?? {}),
          primary,
        };
        if (existingDefault?.fallbacks) {
          model.fallbacks = existingDefault.fallbacks.filter(
            (fallback) => fallback !== primary,
          );
        }

        await openclawApi.setDefaultModel(model);
        await queryClient.invalidateQueries({
          queryKey: openclawKeys.defaultModel,
        });
        await queryClient.invalidateQueries({
          queryKey: openclawKeys.health,
        });
        toast.success(
          t("notifications.openclawDefaultModelSet", {
            defaultValue: "已设为默认模型",
          }),
          { closeButton: true },
        );
      } catch (error) {
        const detail =
          extractErrorMessage(error) ||
          t("notifications.openclawDefaultModelSetFailed", {
            defaultValue: "设置默认模型失败",
          });
        toast.error(detail);
      }
    },
    [queryClient, t],
  );

  return {
    addProvider,
    updateProvider,
    switchProvider,
    deleteProvider,
    saveUsageScript,
    setAsDefaultModel,
    isLoading:
      addProviderMutation.isPending ||
      updateProviderMutation.isPending ||
      deleteProviderMutation.isPending ||
      switchProviderMutation.isPending,
  };
}

function routingReasonText(
  reason: ProviderRoutingReason,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  const keys: Record<ProviderRoutingReason, [string, string]> = {
    githubCopilot: [
      "notifications.proxyReasonCopilot",
      "使用 GitHub Copilot 作为 Claude 供应商",
    ],
    managedOAuth: [
      "notifications.proxyReasonManagedOAuth",
      "使用托管 OAuth 登录（令牌由本地路由注入）",
    ],
    openAiChat: [
      "notifications.proxyReasonOpenAIChat",
      "使用 OpenAI Chat 接口格式",
    ],
    openAiResponses: [
      "notifications.proxyReasonOpenAIResponses",
      "使用 OpenAI Responses 接口格式",
    ],
    anthropicMessages: [
      "notifications.proxyReasonAnthropicMessages",
      "使用 Anthropic Messages 接口格式",
    ],
    claudeDesktop: [
      "notifications.proxyReasonClaudeDesktop",
      "使用 Claude Desktop 本地路由模式",
    ],
    fullUrl: ["notifications.proxyReasonFullUrl", "开启了完整 URL 连接模式"],
    routingRequired: [
      "notifications.proxyReasonRoutingRequired",
      "需要本地路由处理请求",
    ],
  };
  const [key, defaultValue] = keys[reason];
  return t(key, { defaultValue });
}
