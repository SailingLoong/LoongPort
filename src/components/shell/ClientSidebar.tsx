import { useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  AppWindow,
  BarChart3,
  CircleHelp,
  Compass,
  Image,
  Layers3,
  Plug,
  Settings,
  History,
  ChevronsLeft,
  ChevronsRight,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipPortal,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { sidebarWidth, useSidebarCollapsed } from "@/hooks/useSidebarCollapsed";
import { cn } from "@/lib/utils";
import appIcon from "@/assets/icons/app-icon.png";
import { getNavigationSection, type ClientView } from "./navigation";
import { SHELL_OVERLAY_Z_INDEX } from "./layout";

const sidebarItemClassName =
  "h-9 w-full justify-start gap-3 px-3 font-normal aria-[current=page]:bg-primary/10 aria-[current=page]:text-primary aria-[current=page]:font-medium";

const items = [
  {
    view: "providers",
    icon: AppWindow,
    key: "applications",
  },
  {
    view: "services",
    icon: Layers3,
    key: "services",
  },
  { view: "image", icon: Image, key: "image" },
  {
    view: "records",
    icon: History,
    key: "records",
  },
  { view: "usage", icon: BarChart3, key: "usage" },
  {
    view: "resources",
    icon: Plug,
    key: "resources",
  },
  { view: "plaza", icon: Compass, key: "plaza" },
] satisfies {
  view: ClientView;
  icon: typeof AppWindow;
  key: string;
}[];

export function ClientSidebar({
  view,
  top,
  disabled,
  onNavigate,
  onHelp,
  footer,
}: {
  view: ClientView;
  top: number;
  disabled: boolean;
  onNavigate: (view: ClientView) => void;
  onHelp: () => void;
  footer?: ReactNode;
}) {
  const { t } = useTranslation();
  const navRef = useRef<HTMLElement>(null);
  const { collapsed, toggle } = useSidebarCollapsed(navRef);
  const itemClassName = cn(
    sidebarItemClassName,
    collapsed && "justify-center px-0",
  );
  const toggleLabel = t(
    collapsed ? "client.expandSidebar" : "client.collapseSidebar",
  );
  return (
    <TooltipProvider delayDuration={0}>
      <a
        href="#main-content"
        onClick={() => document.getElementById("main-content")?.focus()}
        className="fixed -left-[9999px] rounded-md bg-background px-3 py-2 text-sm text-foreground shadow-md focus:left-4 focus:outline-none focus:ring-2 focus:ring-ring"
        style={{ top: top + 8, zIndex: SHELL_OVERLAY_Z_INDEX }}
      >
        {t("client.skipToContent")}
      </a>
      <aside
        ref={navRef}
        className="fixed bottom-0 left-0 z-40 flex flex-col overflow-hidden border-r border-border-default bg-muted/35 px-3 py-3"
        style={{ top, width: sidebarWidth(collapsed) }}
      >
        <div
          className={cn(
            "mb-4 flex h-10 shrink-0 items-center gap-1.5 text-base font-semibold tracking-tight",
            collapsed ? "justify-center" : "justify-between",
          )}
        >
          <span
            className={cn(
              "flex min-w-0 items-center gap-2",
              collapsed && "sr-only",
            )}
          >
            <img
              src={appIcon}
              alt=""
              aria-hidden
              className="h-5 w-5 shrink-0 rounded-sm"
            />
            <span>LoongPort</span>
          </span>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-8 w-8 shrink-0"
                aria-label={toggleLabel}
                aria-expanded={!collapsed}
                aria-controls="client-navigation"
                onClick={toggle}
              >
                {collapsed ? (
                  <ChevronsRight className="h-4 w-4" />
                ) : (
                  <ChevronsLeft className="h-4 w-4" />
                )}
              </Button>
            </TooltipTrigger>
            <TooltipPortal>
              <TooltipContent
                side="right"
                style={{ zIndex: SHELL_OVERLAY_Z_INDEX }}
              >
                {toggleLabel}
              </TooltipContent>
            </TooltipPortal>
          </Tooltip>
        </div>
        <nav
          id="client-navigation"
          aria-label={t("client.navigation")}
          className="min-h-0 flex-1 space-y-1 overflow-y-auto"
        >
          {items.map((item) => (
            <Tooltip key={item.view}>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  disabled={disabled}
                  aria-label={t(`client.${item.key}`)}
                  aria-current={
                    getNavigationSection(view) === item.view
                      ? "page"
                      : undefined
                  }
                  onClick={() => onNavigate(item.view)}
                  className={itemClassName}
                >
                  <item.icon aria-hidden className="h-4 w-4 shrink-0" />
                  <span className={collapsed ? "sr-only" : "truncate"}>
                    {t(`client.${item.key}`)}
                  </span>
                </Button>
              </TooltipTrigger>
              {collapsed && (
                <TooltipPortal>
                  <TooltipContent
                    side="right"
                    style={{ zIndex: SHELL_OVERLAY_Z_INDEX }}
                  >
                    {t(`client.${item.key}`)}
                  </TooltipContent>
                </TooltipPortal>
              )}
            </Tooltip>
          ))}
        </nav>
        <div className="mt-auto shrink-0 space-y-1 pt-3">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                className={itemClassName}
                disabled={disabled}
                aria-label={t("common.settings")}
                aria-current={view === "settings" ? "page" : undefined}
                onClick={() => onNavigate("settings")}
              >
                <Settings aria-hidden className="h-4 w-4 shrink-0" />
                <span className={collapsed ? "sr-only" : "truncate"}>
                  {t("common.settings")}
                </span>
              </Button>
            </TooltipTrigger>
            {collapsed && (
              <TooltipPortal>
                <TooltipContent
                  side="right"
                  style={{ zIndex: SHELL_OVERLAY_Z_INDEX }}
                >
                  {t("common.settings")}
                </TooltipContent>
              </TooltipPortal>
            )}
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                className={itemClassName}
                aria-label={t("client.help")}
                onClick={onHelp}
              >
                <CircleHelp aria-hidden className="h-4 w-4 shrink-0" />
                <span className={collapsed ? "sr-only" : "truncate"}>
                  {t("client.help")}
                </span>
              </Button>
            </TooltipTrigger>
            {collapsed && (
              <TooltipPortal>
                <TooltipContent
                  side="right"
                  style={{ zIndex: SHELL_OVERLAY_Z_INDEX }}
                >
                  {t("client.help")}
                </TooltipContent>
              </TooltipPortal>
            )}
          </Tooltip>
          <div
            className={cn(
              "flex flex-wrap items-center gap-1 px-1 pt-3",
              collapsed && "justify-center",
            )}
          >
            {footer}
          </div>
        </div>
      </aside>
    </TooltipProvider>
  );
}
