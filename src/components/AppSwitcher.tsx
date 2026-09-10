import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import type { VisibleApps } from "@/types";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Portal } from "@radix-ui/react-tooltip";
import { ProviderIcon } from "@/components/ProviderIcon";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { Image as ImageIcon, Monitor, Plus, Terminal, X } from "lucide-react";
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
  // （PR #116 上游合并时随上游版 AppSwitcher 丢过一次，别再丢。）
  // 生图页原也在此列（OpenAI 标 + 图片角标）—— tab 改名「生图」并支持多家生图
  // 模型后已换成独立图标（见 AppGlyph 的生图分支），不再挂 codex 的品牌。
};

interface AppSwitcherProps {
  activeApp: AppId;
  onSwitch: (app: AppId) => void;
  visibleApps?: VisibleApps;
  applications?: AppId[];
  disabled?: boolean;
  /** tab 上的 ×：就地隐藏一个应用，与设置页「主页面显示」是同一开关 */
  onHideApp?: (app: AppId) => void;
  /** 末尾「+」：把隐藏的应用加回主页面 */
  onShowApp?: (app: AppId) => void;
}

const APP_ICON_NAME: Record<Exclude<AppId, "codex-image">, string> = {
  claude: "claude",
  "claude-desktop": "claude",
  codex: "openai",
  gemini: "gemini",
  grokbuild: "grok",
  opencode: "opencode",
  openclaw: "openclaw",
  hermes: "hermes",
  pi: "pi",
};

/** 应用图标 + 角标（Claude Code / Desktop 用角标区分终端与桌面） */
function AppGlyph({ app, isActive }: { app: AppId; isActive: boolean }) {
  // 生图页用独立的图片图标（跟页面的 violet 主题同色）：它不再从属于某一家 CLI
  // （生图模型 gpt-image / nano-banana / grok-imagine 多家族），挂着 OpenAI 品牌
  // 反而误导。其余 app 走品牌图标（+可选角标）。
  if (app === "codex-image") {
    return (
      <ImageIcon
        className={cn(
          "h-5 w-5 shrink-0",
          isActive
            ? "text-violet-600 dark:text-violet-400"
            : "text-muted-foreground",
        )}
        aria-hidden="true"
      />
    );
  }
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

/** 位移超过该值才算拖拽（在此之前松手照常触发 click 切换 tab）。 */
const DRAG_START_THRESHOLD_PX = 4;

export function AppSwitcher({
  activeApp,
  onSwitch,
  visibleApps,
  onHideApp,
  onShowApp,
  applications = APP_IDS,
  disabled = false,
}: AppSwitcherProps) {
  const { t } = useTranslation();
  const [addOpen, setAddOpen] = useState(false);
  const [dragging, setDragging] = useState(false);

  const stripRef = useRef<HTMLDivElement>(null);
  const activeTabRef = useRef<HTMLButtonElement>(null);
  const dragStateRef = useRef<{
    pointerId: number;
    startX: number;
    startScrollLeft: number;
    dragging: boolean;
  } | null>(null);
  // 拖拽结束的那次 pointerup 会紧接着派发一次 click；不拦下它就会在松手瞬间
  // 切到指针底下恰好停着的那个 tab。pointerdown 时清零、消费时清零。
  const suppressClickRef = useRef(false);

  // 纵向滚轮在条带上转成横向滚动（App 全局隐藏滚动条，滚轮/拖拽是仅有的两个
  // 平移手段）。React 的 onWheel 是 passive 监听，preventDefault 不生效，须挂原生。
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0 || event.deltaX !== 0) return;
      if (el.scrollWidth <= el.clientWidth) return;
      const before = el.scrollLeft;
      el.scrollLeft = before + event.deltaY;
      if (el.scrollLeft !== before) event.preventDefault();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // 拖拽平移：pointerdown 不立即 capture（会把子按钮的 click 吞掉），超过阈值
  // 才对条带 setPointerCapture —— 出窗后 move/up 仍能送达，非拖拽点击不受影响。
  useEffect(() => {
    const onPointerMove = (event: PointerEvent) => {
      const state = dragStateRef.current;
      const el = stripRef.current;
      if (!state || !el || state.pointerId !== event.pointerId) return;
      const dx = event.clientX - state.startX;
      if (!state.dragging) {
        if (Math.abs(dx) < DRAG_START_THRESHOLD_PX) return;
        state.dragging = true;
        setDragging(true);
        el.setPointerCapture(state.pointerId);
      }
      el.scrollLeft = state.startScrollLeft - dx;
    };
    const endDrag = (event: PointerEvent) => {
      const state = dragStateRef.current;
      if (!state || state.pointerId !== event.pointerId) return;
      dragStateRef.current = null;
      if (state.dragging) {
        setDragging(false);
        suppressClickRef.current = true;
      }
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
    };
  }, []);

  const handlePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    suppressClickRef.current = false;
    dragStateRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startScrollLeft: stripRef.current?.scrollLeft ?? 0,
      dragging: false,
    };
  };

  const handleClickCapture = (event: React.MouseEvent<HTMLDivElement>) => {
    if (!suppressClickRef.current) return;
    suppressClickRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  };

  // 切到条带视野外的 tab（或激活 app 被隐藏后的自动回退）时把它滚进来。
  useEffect(() => {
    activeTabRef.current?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, [activeApp]);

  const handleSwitch = (app: AppId) => {
    if (app === activeApp) return;
    localStorage.setItem(LAST_APP_STORAGE_KEY, app);
    onSwitch(app);
  };

  // Filter apps based on visibility settings (default all visible)
  const appsToShow = applications.filter((app) => {
    if (!visibleApps) return true;
    return visibleApps[app] || app === activeApp;
  });
  // 隐藏的应用（「+」的候选）；visibleApps 未加载时视为全部可见
  const hiddenApps = visibleApps
    ? applications.filter((app) => !visibleApps[app] && app !== activeApp)
    : [];
  // 与设置页同一护栏：只剩一个可见应用时不可再隐藏，否则没有任何 tab 可点
  const canHide = appsToShow.length > 1 && onHideApp !== undefined;

  return (
    <TooltipProvider delayDuration={350}>
      <div
        className="inline-flex max-w-full min-w-0 items-center gap-2 rounded-2xl border border-border/60 bg-card p-2 shadow-sm"
        style={{ WebkitAppRegion: "no-drag" } as any}
      >
        {/* 可滚动 tab 条带：tab 多到放不下时不再被静默裁剪，滚轮 / 拖拽平移
          （App 全局隐藏滚动条）；负 margin 抵掉为 × 角标留的溢出空间。 */}
        <div
          ref={stripRef}
          onPointerDown={handlePointerDown}
          onClickCapture={handleClickCapture}
          className={cn(
            "-mx-2 -my-2 flex min-w-0 touch-pan-x gap-1 overflow-x-auto px-2 py-2",
            dragging && "cursor-grabbing",
          )}
        >
          {appsToShow.map((app) => {
            const isActive = activeApp === app;
            const name = getAppDisplayName(app, t);
            return (
              <div key={app} className="group relative shrink-0">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      ref={isActive ? activeTabRef : undefined}
                      type="button"
                      onClick={() => handleSwitch(app)}
                      disabled={disabled}
                      aria-pressed={isActive}
                      aria-label={name}
                      className={cn(
                        "inline-flex items-center justify-center w-12 h-11 rounded-xl text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
                        isActive
                          ? "bg-blue-500/10 text-blue-600 ring-1 ring-inset ring-blue-500/30 dark:text-blue-400"
                          : "text-muted-foreground hover:text-foreground hover:bg-muted",
                      )}
                    >
                      <AppGlyph app={app} isActive={isActive} />
                    </button>
                  </TooltipTrigger>
                  <Portal>
                    <TooltipContent side="bottom">{name}</TooltipContent>
                  </Portal>
                </Tooltip>
                {canHide && (
                  <button
                    type="button"
                    disabled={disabled}
                    title={t("appSwitcher.hide")}
                    aria-label={`${t("appSwitcher.hide")}: ${name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onHideApp?.(app);
                    }}
                    className={cn(
                      "absolute -top-1 -right-1 z-10 flex h-5 w-5 items-center justify-center",
                      "rounded-full border border-border bg-background text-muted-foreground shadow-sm",
                      "opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:text-foreground",
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
        {onShowApp && (
          <Popover open={addOpen} onOpenChange={setAddOpen}>
            <PopoverTrigger asChild>
              <button
                type="button"
                disabled={disabled}
                title={t("appSwitcher.add")}
                aria-label={t("appSwitcher.add")}
                className={cn(
                  "inline-flex shrink-0 items-center justify-center w-10 h-11 rounded-xl border border-dashed border-border transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
                  addOpen
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground hover:bg-background/50",
                )}
              >
                <Plus size={20} className="shrink-0" />
              </button>
            </PopoverTrigger>
            <PopoverContent
              side="bottom"
              align="end"
              sideOffset={6}
              className="z-[100] w-56 p-1"
            >
              {hiddenApps.length === 0 ? (
                <p className="px-2.5 py-2 text-sm text-muted-foreground">
                  {t("appSwitcher.allShown")}
                </p>
              ) : (
                hiddenApps.map((app) => (
                  <button
                    key={app}
                    type="button"
                    disabled={disabled}
                    onClick={() => {
                      onShowApp(app);
                      setAddOpen(false);
                    }}
                    className="group flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                  >
                    <AppGlyph app={app} isActive={false} />
                    <span className="truncate">
                      {getAppDisplayName(app, t)}
                    </span>
                  </button>
                ))
              )}
            </PopoverContent>
          </Popover>
        )}
      </div>
    </TooltipProvider>
  );
}
