import { useTranslation } from "react-i18next";
import {
  ArrowRight,
  Activity,
  MessageSquare,
  Plug,
  Sparkles,
  FileText,
  Bot,
  Layers3,
  Folder,
  Settings2,
  Wrench,
  Brain,
  LayoutDashboard,
  type LucideIcon,
} from "lucide-react";
import type { AppId } from "@/lib/api";
import type { ClientView } from "./navigation";
import { Button } from "@/components/ui/button";

export function FeatureHub({
  kind,
  appId,
  onNavigate,
  onUsage,
  onLaunchDashboard,
}: {
  kind: "records" | "resources";
  appId: AppId;
  onNavigate: (view: ClientView) => void;
  onUsage: () => void;
  onLaunchDashboard: () => void;
}) {
  const { t } = useTranslation();
  const entries: { key: string; icon: LucideIcon; action: () => void }[] =
    kind === "records"
      ? [
          { key: "usage", icon: Activity, action: onUsage },
          ...(appId !== "codex-image"
            ? [
                {
                  key: "sessions",
                  icon: MessageSquare,
                  action: () => onNavigate("sessions"),
                },
              ]
            : []),
        ]
      : [
          ...(appId !== "pi"
            ? [{ key: "mcp", icon: Plug, action: () => onNavigate("mcp") }]
            : []),
          { key: "skills", icon: Sparkles, action: () => onNavigate("skills") },
          {
            key: "prompts",
            icon: FileText,
            action: () => onNavigate("prompts"),
          },
          { key: "agents", icon: Bot, action: () => onNavigate("agents") },
          {
            key: "universal",
            icon: Layers3,
            action: () => onNavigate("universal"),
          },
          {
            key: "workspace",
            icon: Folder,
            action: () => onNavigate("workspace"),
          },
          ...(appId === "openclaw"
            ? [
                {
                  key: "openclawEnv",
                  icon: Settings2,
                  action: () => onNavigate("openclawEnv"),
                },
                {
                  key: "openclawTools",
                  icon: Wrench,
                  action: () => onNavigate("openclawTools"),
                },
                {
                  key: "openclawAgents",
                  icon: Bot,
                  action: () => onNavigate("openclawAgents"),
                },
              ]
            : []),
          ...(appId === "hermes"
            ? [
                {
                  key: "hermesMemory",
                  icon: Brain,
                  action: () => onNavigate("hermesMemory"),
                },
                {
                  key: "hermesDashboard",
                  icon: LayoutDashboard,
                  action: onLaunchDashboard,
                },
              ]
            : []),
        ];
  return (
    <div className="page-content">
      <p className="mb-6 text-sm leading-6 text-muted-foreground">
        {t(`client.${kind}Description`)}
      </p>
      <div className="grid gap-4 md:grid-cols-2">
        {entries.map((entry) => {
          const Icon = entry.icon;
          return (
            <Button
              key={entry.key}
              variant="ghost"
              onClick={entry.action}
              className="group h-auto min-h-28 justify-start gap-4 whitespace-normal rounded-xl border border-border-default bg-card p-5 text-left hover:border-primary/30 hover:bg-accent/50"
            >
              <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground group-hover:text-primary">
                <Icon className="h-5 w-5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-semibold text-foreground">
                  {t(`client.features.${entry.key}`)}
                </span>
                <span className="mt-1.5 block text-sm font-normal leading-5 text-muted-foreground">
                  {t(`client.featureDescriptions.${entry.key}`)}
                </span>
              </span>
              <ArrowRight className="ml-3 h-4 w-4 shrink-0 text-muted-foreground" />
            </Button>
          );
        })}
      </div>
    </div>
  );
}
