/**
 * 故障转移控制块：统一开关（全部 app 一次生效）+ 主页面显示开关 + 首开确认。
 *
 * 两个宿主共用一份：
 * - 「省心模式」tab（Beta 开关开着时）—— 省心模式下队列是兜底，控制放一起；
 * - 「高级」tab 的「自动故障转移」折叠项（省心模式 Beta 关闭的发布形态）——
 *   故障转移是早已发布的既有功能，省心 tab 藏起来时它必须有家，不能跟着消失。
 * 队列管理本身：省心形态在各卡「高级」折叠里；发布形态在 FailoverAccordionItem
 * 的 app 子标签里（见下）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Activity, Zap, ShieldAlert } from "lucide-react";

import { Switch } from "@/components/ui/switch";
import { ToggleRow } from "@/components/ui/toggle-row";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { FailoverQueueManager } from "@/components/proxy/FailoverQueueManager";
import { AutoFailoverConfigPanel } from "@/components/proxy/AutoFailoverConfigPanel";
import { useSetFailoverAll } from "@/lib/query/autoMode";
import { useAutoFailoverEnabled } from "@/lib/query/failover";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { SettingsFormState } from "@/hooks/useSettings";
import { getAppLabel, PROXY_APP_IDS } from "@/config/appConfig";

interface FailoverControlsProps {
  settings: SettingsFormState;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>;
}

export function FailoverControls({
  settings,
  onAutoSave,
}: FailoverControlsProps) {
  const { t } = useTranslation();
  const setFailoverAll = useSetFailoverAll();
  const [showFailoverConfirm, setShowFailoverConfirm] = useState(false);

  // 统一故障转移开关的状态：全部 app 都开 = 开。
  const failoverStates = PROXY_APP_IDS.map(
    (appType) => useAutoFailoverEnabled(appType).data ?? false,
  );
  const failoverChecked = failoverStates.every(Boolean);

  const handleDisplayToggleChange = (checked: boolean) => {
    if (checked && !settings?.failoverConfirmed) {
      setShowFailoverConfirm(true);
    } else {
      void onAutoSave({ enableFailoverToggle: checked });
    }
  };

  const handleFailoverConfirm = async () => {
    setShowFailoverConfirm(false);
    await onAutoSave({ failoverConfirmed: true, enableFailoverToggle: true });
  };

  return (
    <>
      {/* 统一的自动故障转移开关：作用于全部 app。 */}
      <div className="rounded-xl glass-card p-6">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <div className="flex items-center gap-2">
              <Zap className="h-4 w-4 text-orange-500" />
              <span className="text-sm font-medium">
                {t("proxy.failover.autoSwitch", "自动故障转移")}
              </span>
            </div>
            <p className="text-xs text-muted-foreground">
              {t(
                "proxy.failover.autoSwitchDescription",
                "开启后各应用立即切换到各自队列的 P1，请求失败时自动切换队列中的下一个供应商；对所有应用统一生效",
              )}
            </p>
          </div>
          <Switch
            checked={failoverChecked}
            onCheckedChange={(checked) =>
              setFailoverAll.mutate({
                apps: [...PROXY_APP_IDS],
                enabled: checked,
              })
            }
            disabled={setFailoverAll.isPending}
            aria-label={t("proxy.failover.autoSwitch", "自动故障转移")}
          />
        </div>
      </div>

      {/* 主页面故障转移开关：控制顶栏 FailoverToggle 是否显示。 */}
      <div className="rounded-xl glass-card p-6">
        <ToggleRow
          icon={<ShieldAlert className="h-4 w-4 text-orange-500" />}
          title={t("settings.advanced.proxy.enableFailoverToggle")}
          description={t(
            "settings.advanced.proxy.enableFailoverToggleDescription",
          )}
          checked={settings?.enableFailoverToggle ?? false}
          onCheckedChange={handleDisplayToggleChange}
        />
      </div>

      <ConfirmDialog
        isOpen={showFailoverConfirm}
        variant="info"
        title={t("confirm.failover.title")}
        message={t("confirm.failover.message")}
        confirmText={t("confirm.failover.confirm")}
        onConfirm={() => void handleFailoverConfirm()}
        onCancel={() => setShowFailoverConfirm(false)}
      />
    </>
  );
}

/**
 * 「高级」tab 的故障转移折叠项（发布形态）：统一开关 + 各 app 的队列/熔断管理。
 * 省心模式 Beta 开着时不渲染 —— 那些管理收在省心模式页的卡片「高级」折叠里，
 * 两处同时出现只会让人困惑。
 */
export function FailoverAccordionItem({
  settings,
  onAutoSave,
}: FailoverControlsProps) {
  const { t } = useTranslation();
  const { isRunning, takeoverStatus } = useProxyStatus();

  return (
    <AccordionItem
      value="failover"
      className="rounded-xl glass-card overflow-hidden"
    >
      <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
        <div className="flex items-center gap-3">
          <Activity className="h-5 w-5 text-orange-500" />
          <div className="text-left">
            <h3 className="text-base font-semibold">
              {t("settings.advanced.failover.title")}
            </h3>
            <p className="text-sm text-muted-foreground font-normal">
              {t("settings.advanced.failover.description")}
            </p>
          </div>
        </div>
      </AccordionTrigger>
      <AccordionContent className="space-y-4 px-6 pb-6 pt-4 border-t border-border/50">
        <FailoverControls settings={settings} onAutoSave={onAutoSave} />
        <Tabs defaultValue="claude" className="w-full">
          <TabsList className="grid w-full grid-cols-4">
            {PROXY_APP_IDS.map((id) => (
              <TabsTrigger key={id} value={id}>
                {getAppLabel(id)}
              </TabsTrigger>
            ))}
          </TabsList>
          {PROXY_APP_IDS.map((appType) => {
            const disabled =
              !isRunning || !(takeoverStatus?.[appType] ?? false);
            return (
              <TabsContent
                key={appType}
                value={appType}
                className="mt-4 space-y-6"
              >
                <FailoverQueueManager appType={appType} disabled={disabled} />
                <div className="border-t border-border/50 pt-6">
                  <AutoFailoverConfigPanel
                    appType={appType}
                    disabled={disabled}
                  />
                </div>
              </TabsContent>
            );
          })}
        </Tabs>
      </AccordionContent>
    </AccordionItem>
  );
}
