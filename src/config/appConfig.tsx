import React from "react";
import type { AppId } from "@/lib/api/types";
import type { VisibleApps } from "@/types";
import {
  ClaudeIcon,
  CodexIcon,
  GeminiIcon,
  OpenClawIcon,
} from "@/components/BrandIcons";
import { ProviderIcon } from "@/components/ProviderIcon";

export interface AppConfig {
  label: string;
  icon: React.ReactNode;
  activeClass: string;
  badgeClass: string;
}

export const APP_IDS: AppId[] = [
  "claude",
  "claude-desktop",
  "codex",
  "codex-image",
  "gemini",
  "grokbuild",
  "opencode",
  "openclaw",
  "hermes",
];

/** App IDs shown in Skills panels (excludes OpenClaw — it doesn't support Skills) */
export const SKILLS_APP_IDS: AppId[] = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "hermes",
];

/** App IDs shown in MCP panels (excludes OpenClaw) */
export const MCP_APP_IDS: AppId[] = [...SKILLS_APP_IDS];

/**
 * 全部标签默认可见时的 `VisibleApps`。
 *
 * ⚠️ **唯一定义** —— 原来 `App.tsx` 与 `AppVisibilitySettings.tsx` 各写一份字面量，
 * 加一个 app 时改一处漏一处（这次是编译器抓到的，但只因为 `VisibleApps` 的新字段
 * 是必填；若新字段可选，分叉就会静默 —— 一处认为默认显示、另一处认为默认隐藏）。
 * CLAUDE.md §三点六。
 *
 * 注意 `hermes: true`：这里是**前端读不到设置时的兜底**，与后端
 * `VisibleApps::default()`（那边 hermes 是 false）有意不同 —— 那边管「新装机的初始值」，
 * 这里管「设置还没加载完的那一帧」，让标签先都显示比闪一下少一个更平稳。
 */
export const ALL_APPS_VISIBLE: VisibleApps = {
  claude: true,
  "claude-desktop": true,
  codex: true,
  "codex-image": true,
  gemini: true,
  grokbuild: true,
  opencode: true,
  openclaw: true,
  hermes: true,
};

export const APP_ICON_MAP: Record<AppId, AppConfig> = {
  claude: {
    label: "Claude",
    icon: <ClaudeIcon size={14} />,
    activeClass:
      "bg-orange-500/10 ring-1 ring-orange-500/20 hover:bg-orange-500/20 text-orange-600 dark:text-orange-400",
    badgeClass:
      "bg-orange-500/10 text-orange-700 dark:text-orange-300 hover:bg-orange-500/20 border-0 gap-1.5",
  },
  "claude-desktop": {
    label: "Claude Desktop",
    icon: <ClaudeIcon size={14} />,
    activeClass:
      "bg-amber-500/10 ring-1 ring-amber-500/20 hover:bg-amber-500/20 text-amber-700 dark:text-amber-300",
    badgeClass:
      "bg-amber-500/10 text-amber-700 dark:text-amber-300 hover:bg-amber-500/20 border-0 gap-1.5",
  },
  codex: {
    label: "Codex",
    icon: <CodexIcon size={14} />,
    activeClass:
      "bg-green-500/10 ring-1 ring-green-500/20 hover:bg-green-500/20 text-green-600 dark:text-green-400",
    badgeClass:
      "bg-green-500/10 text-green-700 dark:text-green-300 hover:bg-green-500/20 border-0 gap-1.5",
  },
  // 生图：与 codex 同一个品牌图标（它就是 codex 的一部分），配色用 violet ——
  // 蓝（当前在用）/ 绿（codex）/ amber（Claude Desktop）都已有主，violet 没被占。
  "codex-image": {
    // `label` 只被 MCP / Skills 面板的 app 选择器消费（`AppToggleGroup` /
    // `AppCountBar`），而生图栏不在 `MCP_APP_IDS` / `SKILLS_APP_IDS` 里 ⇒ 走不到这里。
    // 留英文而不是中文：万一将来被别处消费，混语比英文更糟。标签栏那个名字走
    // i18n（`apps.codex-image`，见 `AppSwitcher`）。
    label: "Codex Images",
    icon: <CodexIcon size={14} />,
    activeClass:
      "bg-violet-500/10 ring-1 ring-violet-500/20 hover:bg-violet-500/20 text-violet-600 dark:text-violet-400",
    badgeClass:
      "bg-violet-500/10 text-violet-700 dark:text-violet-300 hover:bg-violet-500/20 border-0 gap-1.5",
  },
  gemini: {
    label: "Gemini",
    icon: <GeminiIcon size={14} />,
    activeClass:
      "bg-blue-500/10 ring-1 ring-blue-500/20 hover:bg-blue-500/20 text-blue-600 dark:text-blue-400",
    badgeClass:
      "bg-blue-500/10 text-blue-700 dark:text-blue-300 hover:bg-blue-500/20 border-0 gap-1.5",
  },
  grokbuild: {
    label: "Grok Build",
    icon: (
      <ProviderIcon
        icon="grok"
        name="Grok Build"
        size={14}
        showFallback={false}
      />
    ),
    activeClass:
      "bg-cyan-500/10 ring-1 ring-cyan-500/20 hover:bg-cyan-500/20 text-cyan-700 dark:text-cyan-300",
    badgeClass:
      "bg-cyan-500/10 text-cyan-700 dark:text-cyan-300 hover:bg-cyan-500/20 border-0 gap-1.5",
  },
  opencode: {
    label: "OpenCode",
    icon: (
      <ProviderIcon
        icon="opencode"
        name="OpenCode"
        size={14}
        showFallback={false}
      />
    ),
    activeClass:
      "bg-indigo-500/10 ring-1 ring-indigo-500/20 hover:bg-indigo-500/20 text-indigo-600 dark:text-indigo-400",
    badgeClass:
      "bg-indigo-500/10 text-indigo-700 dark:text-indigo-300 hover:bg-indigo-500/20 border-0 gap-1.5",
  },
  openclaw: {
    label: "OpenClaw",
    icon: <OpenClawIcon size={14} />,
    activeClass:
      "bg-rose-500/10 ring-1 ring-rose-500/20 hover:bg-rose-500/20 text-rose-600 dark:text-rose-400",
    badgeClass:
      "bg-rose-500/10 text-rose-700 dark:text-rose-300 hover:bg-rose-500/20 border-0 gap-1.5",
  },
  hermes: {
    label: "Hermes",
    icon: (
      <ProviderIcon
        icon="hermes"
        name="Hermes"
        size={14}
        showFallback={false}
      />
    ),
    activeClass:
      "bg-violet-500/10 ring-1 ring-violet-500/20 hover:bg-violet-500/20 text-violet-600 dark:text-violet-400",
    badgeClass:
      "bg-violet-500/10 text-violet-700 dark:text-violet-300 hover:bg-violet-500/20 border-0 gap-1.5",
  },
};
