import type { ReactNode } from "react";
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
} from "lucide-react";
import { Button } from "@/components/ui/button";
import appIcon from "@/assets/icons/app-icon.png";
import { getNavigationSection, type ClientView } from "./navigation";

const sidebarItemClassName =
  "h-10 w-full justify-start gap-3 px-3 font-normal aria-[current=page]:bg-primary/10 aria-[current=page]:text-primary aria-[current=page]:font-medium";

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
  return (
    <aside
      className="fixed bottom-0 left-0 z-40 flex w-[var(--sidebar-width)] flex-col border-r border-border-default bg-muted/35 px-3 py-5"
      style={{ top }}
    >
      <div className="mb-7 flex items-center gap-2.5 px-3 text-lg font-semibold tracking-tight">
        <img src={appIcon} alt="" aria-hidden className="h-5 w-5 rounded-sm" />
        LoongPort
      </div>
      <nav aria-label={t("client.navigation")} className="space-y-1">
        {items.map((item) => (
          <Button
            key={item.view}
            variant="ghost"
            disabled={disabled}
            aria-current={
              getNavigationSection(view) === item.view ? "page" : undefined
            }
            onClick={() => onNavigate(item.view)}
            className={sidebarItemClassName}
          >
            <item.icon className="h-4 w-4" />
            {t(`client.${item.key}`)}
          </Button>
        ))}
      </nav>
      <div className="mt-auto space-y-1 pt-6">
        <Button
          variant="ghost"
          className={sidebarItemClassName}
          disabled={disabled}
          aria-current={view === "settings" ? "page" : undefined}
          onClick={() => onNavigate("settings")}
        >
          <Settings className="h-4 w-4" />
          {t("common.settings")}
        </Button>
        <Button
          variant="ghost"
          className={sidebarItemClassName}
          onClick={onHelp}
        >
          <CircleHelp className="h-4 w-4" />
          {t("client.help")}
        </Button>
        <div className="flex flex-wrap items-center gap-1 px-1 pt-3">
          {footer}
        </div>
      </div>
    </aside>
  );
}
