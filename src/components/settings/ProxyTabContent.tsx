import { useState } from "react";
import { Server, Activity, Zap, Globe } from "lucide-react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { ProxyPanel } from "@/components/proxy";
import { RectifierConfigPanel } from "@/components/settings/RectifierConfigPanel";
import { GlobalProxySettings } from "@/components/settings/GlobalProxySettings";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { SettingsFormState } from "@/hooks/useSettings";

interface ProxyTabContentProps {
  settings: SettingsFormState;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>;
}

/**
 * 「高级路由」设置页（过渡形态）。
 *
 * 自动模式、故障转移队列与熔断/超时已迁往「自动模式」tab（主入口），
 * 这里只剩路由基础设施：本地路由服务面板、整流器、全局出站代理。
 * 下一步计划：这三块并入设置「高级」tab 后本 tab 移除。
 */
export function ProxyTabContent({
  settings,
  onAutoSave,
}: ProxyTabContentProps) {
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
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      <Accordion type="multiple" defaultValue={[]} className="w-full space-y-4">
        {/* Local Proxy */}
        <AccordionItem
          value="proxy"
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
              enableLocalProxy={settings?.enableLocalProxy ?? false}
              onEnableLocalProxyChange={(checked) =>
                onAutoSave({ enableLocalProxy: checked })
              }
              onToggleProxy={handleToggleProxy}
              isProxyPending={isProxyPending}
            />
          </AccordionContent>
        </AccordionItem>

        {/* Rectifier */}
        <AccordionItem
          value="rectifier"
          className="rounded-xl glass-card overflow-hidden"
        >
          <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
            <div className="flex items-center gap-3">
              <Zap className="h-5 w-5 text-purple-500" />
              <div className="text-left">
                <h3 className="text-base font-semibold">
                  {t("settings.advanced.rectifier.title")}
                </h3>
                <p className="text-sm text-muted-foreground font-normal">
                  {t("settings.advanced.rectifier.description")}
                </p>
              </div>
            </div>
          </AccordionTrigger>
          <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
            <RectifierConfigPanel />
          </AccordionContent>
        </AccordionItem>

        {/* Global Outbound Proxy */}
        <AccordionItem
          value="globalProxy"
          className="rounded-xl glass-card overflow-hidden"
        >
          <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
            <div className="flex items-center gap-3">
              <Globe className="h-5 w-5 text-cyan-500" />
              <div className="text-left">
                <h3 className="text-base font-semibold">
                  {t("settings.advanced.globalProxy.title")}
                </h3>
                <p className="text-sm text-muted-foreground font-normal">
                  {t("settings.advanced.globalProxy.description")}
                </p>
              </div>
            </div>
          </AccordionTrigger>
          <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
            <GlobalProxySettings />
          </AccordionContent>
        </AccordionItem>
      </Accordion>

      <ConfirmDialog
        isOpen={showProxyConfirm}
        variant="info"
        title={t("confirm.proxy.title")}
        message={t("confirm.proxy.message")}
        confirmText={t("confirm.proxy.confirm")}
        onConfirm={() => void handleProxyConfirm()}
        onCancel={() => setShowProxyConfirm(false)}
      />
    </motion.div>
  );
}
