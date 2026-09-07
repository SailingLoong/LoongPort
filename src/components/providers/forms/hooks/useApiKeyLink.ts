import { useMemo } from "react";
import type { AppId } from "@/lib/api";
import { usePresetReferralUrls } from "@/hooks/usePresetReferralUrls";
import { resolvePresetReferralUrl } from "@/lib/presetReferrals";
import type { ProviderCategory } from "@/types";
import type { ProviderPreset } from "@/config/claudeProviderPresets";
import type { CodexProviderPreset } from "@/config/codexProviderPresets";
import type { GeminiProviderPreset } from "@/config/geminiProviderPresets";
import type { OpenCodeProviderPreset } from "@/config/opencodeProviderPresets";
import type { ClaudeDesktopProviderPreset } from "@/config/claudeDesktopProviderPresets";

type PresetEntry = {
  id: string;
  preset:
    | ProviderPreset
    | CodexProviderPreset
    | GeminiProviderPreset
    | OpenCodeProviderPreset
    | ClaudeDesktopProviderPreset;
};

interface UseApiKeyLinkProps {
  appId: AppId;
  category?: ProviderCategory;
  selectedPresetId: string | null;
  presetEntries: PresetEntry[];
  formWebsiteUrl: string;
}

/**
 * 管理 API Key 获取链接的显示和 URL
 */
export function useApiKeyLink({
  appId,
  category,
  selectedPresetId,
  presetEntries,
  formWebsiteUrl,
}: UseApiKeyLinkProps) {
  // 返佣覆盖（远端配置）：命中时「获取 API Key」打开维护者的返佣链接，
  // 未命中回落预设自带的中性链接。
  const presetReferrals = usePresetReferralUrls();
  // 判断是否显示 API Key 获取链接
  const shouldShowApiKeyLink = useMemo(() => {
    return (
      category !== "official" &&
      (category === "cn_official" ||
        category === "aggregator" ||
        category === "third_party")
    );
  }, [category]);

  // 获取当前预设条目
  const currentPresetEntry = useMemo(() => {
    if (selectedPresetId && selectedPresetId !== "custom") {
      return presetEntries.find((item) => item.id === selectedPresetId);
    }
    return undefined;
  }, [selectedPresetId, presetEntries]);

  // 获取当前供应商的网址（用于 API Key 链接）
  const getWebsiteUrl = useMemo(() => {
    if (currentPresetEntry) {
      const preset = currentPresetEntry.preset;
      // 对于 cn_official、aggregator、third_party，优先使用 apiKeyUrl（可能包含推广参数）
      const neutralUrl =
        preset.category === "cn_official" ||
        preset.category === "aggregator" ||
        preset.category === "third_party"
          ? preset.apiKeyUrl || preset.websiteUrl || ""
          : preset.websiteUrl || "";
      return (
        resolvePresetReferralUrl(neutralUrl, presetReferrals) ?? neutralUrl
      );
    }
    return formWebsiteUrl || "";
  }, [currentPresetEntry, formWebsiteUrl, presetReferrals]);

  return {
    shouldShowApiKeyLink:
      appId === "claude" ||
      appId === "claude-desktop" ||
      appId === "codex" ||
      appId === "gemini" ||
      appId === "opencode" ||
      appId === "openclaw" ||
      appId === "hermes"
        ? shouldShowApiKeyLink
        : false,
    websiteUrl: getWebsiteUrl,
  };
}
