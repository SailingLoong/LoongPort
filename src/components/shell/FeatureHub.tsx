import { useTranslation } from "react-i18next";
import { ArrowRight } from "lucide-react";
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
  const entries: { key: string; action: () => void }[] =
    kind === "records"
      ? [
          { key: "usage", action: onUsage },
          ...(appId !== "codex-image"
            ? [{ key: "sessions", action: () => onNavigate("sessions") }]
            : []),
        ]
      : [
          ...(appId !== "pi"
            ? [{ key: "mcp", action: () => onNavigate("mcp") }]
            : []),
          { key: "skills", action: () => onNavigate("skills") },
          { key: "prompts", action: () => onNavigate("prompts") },
          { key: "agents", action: () => onNavigate("agents") },
          { key: "universal", action: () => onNavigate("universal") },
          { key: "workspace", action: () => onNavigate("workspace") },
          ...(appId === "openclaw"
            ? [
                { key: "openclawEnv", action: () => onNavigate("openclawEnv") },
                {
                  key: "openclawTools",
                  action: () => onNavigate("openclawTools"),
                },
                {
                  key: "openclawAgents",
                  action: () => onNavigate("openclawAgents"),
                },
              ]
            : []),
          ...(appId === "hermes"
            ? [
                {
                  key: "hermesMemory",
                  action: () => onNavigate("hermesMemory"),
                },
                { key: "hermesDashboard", action: onLaunchDashboard },
              ]
            : []),
        ];
  return (
    <div className="mx-auto w-full max-w-[1100px] px-7 py-5">
      <p className="mb-5 text-sm text-muted-foreground">
        {t(`client.${kind}Description`)}
      </p>
      <div className="grid gap-x-7 md:grid-cols-2">
        {entries.map((entry) => (
          <Button
            key={entry.key}
            variant="ghost"
            onClick={entry.action}
            className="h-auto justify-between rounded-none border-b border-border-default px-1 py-5 text-left"
          >
            <span>
              <span className="block text-sm font-medium">
                {t(`client.features.${entry.key}`)}
              </span>
              <span className="mt-1 block text-xs font-normal text-muted-foreground">
                {t(`client.featureDescriptions.${entry.key}`)}
              </span>
            </span>
            <ArrowRight className="ml-3 h-4 w-4 shrink-0 text-muted-foreground" />
          </Button>
        ))}
      </div>
    </div>
  );
}
