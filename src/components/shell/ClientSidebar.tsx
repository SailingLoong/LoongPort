import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  AppWindow,
  CircleHelp,
  Compass,
  Image,
  Layers3,
  Plug,
  Settings,
  History,
  Waypoints,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ClientView } from "./navigation";

const items = [
  {
    view: "providers",
    icon: AppWindow,
    key: "applications",
    children: ["providers"],
  },
  {
    view: "services",
    icon: Layers3,
    key: "services",
    children: ["services", "addHub"],
  },
  { view: "image", icon: Image, key: "image", children: ["image"] },
  {
    view: "records",
    icon: History,
    key: "records",
    children: ["records", "sessions"],
  },
  {
    view: "resources",
    icon: Plug,
    key: "resources",
    children: [
      "resources",
      "skills",
      "skillsDiscovery",
      "mcp",
      "prompts",
      "agents",
      "universal",
      "workspace",
      "openclawEnv",
      "openclawTools",
      "openclawAgents",
      "hermesMemory",
    ],
  },
  { view: "plaza", icon: Compass, key: "plaza", children: ["plaza"] },
] satisfies {
  view: ClientView;
  icon: typeof AppWindow;
  key: string;
  children: string[];
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
      className="fixed bottom-0 left-0 z-40 flex w-[176px] flex-col border-r border-border-default bg-muted/25 px-3 py-5"
      style={{ top }}
    >
      <div className="mb-7 flex items-center gap-2 px-2 text-lg font-semibold tracking-tight">
        <Waypoints className="h-5 w-5 text-primary" />
        LoongPort
      </div>
      <nav aria-label={t("client.navigation")} className="space-y-1">
        {items.map((item) => (
          <Button
            key={item.view}
            variant="ghost"
            disabled={disabled}
            aria-current={item.children.includes(view) ? "page" : undefined}
            onClick={() => onNavigate(item.view)}
            className={cn(
              "h-10 w-full justify-start gap-2.5 px-2.5 font-normal",
              item.children.includes(view) &&
                "bg-background text-primary shadow-sm font-medium",
            )}
          >
            <item.icon className="h-4 w-4" />
            {t(`client.${item.key}`)}
          </Button>
        ))}
      </nav>
      <div className="mt-auto space-y-1 pt-6">
        <Button
          variant="ghost"
          className="w-full justify-start gap-2.5 px-2.5"
          disabled={disabled}
          aria-current={view === "settings" ? "page" : undefined}
          onClick={() => onNavigate("settings")}
        >
          <Settings className="h-4 w-4" />
          {t("common.settings")}
        </Button>
        <Button
          variant="ghost"
          className="w-full justify-start gap-2.5 px-2.5"
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
