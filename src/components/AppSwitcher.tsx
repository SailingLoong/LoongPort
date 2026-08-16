import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import type { VisibleApps } from "@/types";
import { ProviderIcon } from "@/components/ProviderIcon";
import { cn } from "@/lib/utils";
import { Image as ImageIcon, Monitor, Terminal, X } from "lucide-react";
import {
  APP_DISPLAY_NAME,
  APP_IDS,
  getAppDisplayName,
} from "@/config/appConfig";
import { LAST_APP_STORAGE_KEY } from "@/config/constants";

const APP_BADGE_ICON: Partial<
  Record<AppId, { icon: typeof Terminal; offsetY?: number }>
> = {
  claude: { icon: Terminal },
  "claude-desktop": { icon: Monitor, offsetY: 0.5 },
  // 与 claude-desktop 同一个手法：同品牌图标 + 一个角标说明「这是那个 CLI 的另一面」。
  // 角标是图片而不是文字，因为它要在 20px 的图标上认得出来。
  // （PR #116 上游合并时随上游版 AppSwitcher 丢过一次，别再丢。）
  "codex-image": { icon: ImageIcon, offsetY: 0.5 },
};

interface AppSwitcherProps {
  activeApp: AppId;
  onSwitch: (app: AppId) => void;
  visibleApps?: VisibleApps;
  /** tab 上的 ×：就地隐藏一个应用，与设置页「主页面显示」是同一开关 */
  onHideApp?: (app: AppId) => void;
}

const APP_ICON_NAME: Record<AppId, string> = {
  claude: "claude",
  "claude-desktop": "claude",
  codex: "openai",
  "codex-image": "openai",
  gemini: "gemini",
  grokbuild: "grok",
  opencode: "opencode",
  openclaw: "openclaw",
  hermes: "hermes",
  pi: "pi",
};

/** 应用图标 + 角标（Claude Code / Desktop 用角标区分终端与桌面） */
function AppGlyph({ app, isActive }: { app: AppId; isActive: boolean }) {
  const badgeConfig = APP_BADGE_ICON[app];
  const BadgeIcon = badgeConfig?.icon;
  return (
    <span className="relative inline-flex shrink-0">
      <ProviderIcon
        icon={APP_ICON_NAME[app]}
        name={APP_DISPLAY_NAME[app]}
        size={20}
      />
      {BadgeIcon && (
        <span
          className={cn(
            "absolute -bottom-0.5 -right-0.5 flex items-center justify-center rounded-[3px] border h-[11px] w-[11px]",
            isActive
              ? "bg-background border-border text-foreground"
              : "bg-muted border-background text-muted-foreground group-hover:bg-background group-hover:text-foreground",
          )}
          aria-hidden="true"
        >
          <BadgeIcon
            className="h-[8px] w-[8px]"
            strokeWidth={2.5}
            style={
              badgeConfig?.offsetY
                ? { transform: `translateY(${badgeConfig.offsetY}px)` }
                : undefined
            }
          />
        </span>
      )}
    </span>
  );
}

export function AppSwitcher({
  activeApp,
  onSwitch,
  visibleApps,
  onHideApp,
}: AppSwitcherProps) {
  const { t } = useTranslation();

  const handleSwitch = (app: AppId) => {
    if (app === activeApp) return;
    localStorage.setItem(LAST_APP_STORAGE_KEY, app);
    onSwitch(app);
  };

  // Filter apps based on visibility settings (default all visible)
  const appsToShow = APP_IDS.filter((app) => {
    if (!visibleApps) return true;
    return visibleApps[app];
  });
  // 与设置页同一护栏：只剩一个可见应用时不可再隐藏，否则没有任何 tab 可点
  const canHide = appsToShow.length > 1 && onHideApp !== undefined;

  return (
    <div
      className="inline-flex bg-muted rounded-xl p-1 gap-1"
      style={{ WebkitAppRegion: "no-drag" } as any}
    >
      {appsToShow.map((app) => {
        const isActive = activeApp === app;
        const name = getAppDisplayName(app, t);
        return (
          <div key={app} className="group relative">
            <button
              type="button"
              onClick={() => handleSwitch(app)}
              title={name}
              aria-label={name}
              className={cn(
                "inline-flex items-center px-3 h-8 rounded-md text-sm font-medium transition-all duration-200",
                isActive
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground hover:bg-background/50",
              )}
            >
              <AppGlyph app={app} isActive={isActive} />
            </button>
            {canHide && (
              <button
                type="button"
                title={t("appSwitcher.hide")}
                aria-label={`${t("appSwitcher.hide")}: ${name}`}
                onClick={(event) => {
                  event.stopPropagation();
                  onHideApp?.(app);
                }}
                className={cn(
                  "absolute -top-1.5 -right-1 z-10 flex h-3.5 w-3.5 items-center justify-center",
                  "rounded-full border border-border bg-background text-muted-foreground shadow-sm",
                  "opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 hover:text-foreground",
                )}
              >
                <X
                  aria-hidden="true"
                  className="h-[9px] w-[9px]"
                  strokeWidth={2.5}
                />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
