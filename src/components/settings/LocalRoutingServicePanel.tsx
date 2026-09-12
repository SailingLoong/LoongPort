/** Local proxy runtime controls shared by connection settings. */

import { useState } from "react";
import { Server, Activity } from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { ProxyPanel } from "@/components/proxy";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { SettingsFormState } from "@/hooks/useSettings";

interface LocalRoutingServicePanelProps {
  settings: SettingsFormState;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>;
}

export function LocalRoutingServicePanel({
  settings,
  onAutoSave,
}: LocalRoutingServicePanelProps) {
  const { t } = useTranslation();
  const [showProxyConfirm, setShowProxyConfirm] = useState(false);

  const {
    isRunning,
    startProxyServer,
    stopWithRestore,
    isPending: isProxyPending,
  } = useProxyStatus();

  const handleToggleProxy = async (checked: boolean) => {
    try {
      if (!checked) {
        await stopWithRestore();
      } else if (!settings?.proxyConfirmed) {
        setShowProxyConfirm(true);
      } else {
        await startProxyServer();
      }
    } catch (error) {
      console.error("Toggle proxy failed:", error);
    }
  };

  const handleProxyConfirm = async () => {
    setShowProxyConfirm(false);
    try {
      await onAutoSave({ proxyConfirmed: true });
      await startProxyServer();
    } catch (error) {
      console.error("Proxy confirm failed:", error);
    }
  };

  return (
    <AccordionItem
      value="localRouting"
      className="rounded-xl glass-card overflow-hidden"
    >
      <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
        <div className="flex items-center gap-3">
          <Server className="h-5 w-5 text-green-500" />
          <div className="text-left">
            <h3 className="text-base font-semibold">
              {t("settings.advanced.proxy.title")}
            </h3>
            <p className="text-sm text-muted-foreground font-normal">
              {t("settings.advanced.proxy.description")}
            </p>
          </div>
          <Badge
            variant={isRunning ? "default" : "secondary"}
            className="gap-1.5 h-6 ml-auto mr-2"
          >
            <Activity
              className={`h-3 w-3 ${isRunning ? "status-heartbeat" : ""}`}
            />
            {isRunning
              ? t("settings.advanced.proxy.running")
              : t("settings.advanced.proxy.stopped")}
          </Badge>
        </div>
      </AccordionTrigger>
      <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
        <ProxyPanel
          showProviderQueues={false}
          enableLocalProxy={settings?.enableLocalProxy ?? false}
          onEnableLocalProxyChange={(checked) =>
            onAutoSave({ enableLocalProxy: checked })
          }
          onToggleProxy={handleToggleProxy}
          isProxyPending={isProxyPending}
        />
      </AccordionContent>

      <ConfirmDialog
        isOpen={showProxyConfirm}
        variant="info"
        title={t("confirm.proxy.title")}
        message={t("confirm.proxy.message")}
        confirmText={t("confirm.proxy.confirm")}
        onConfirm={() => void handleProxyConfirm()}
        onCancel={() => setShowProxyConfirm(false)}
      />
    </AccordionItem>
  );
}
