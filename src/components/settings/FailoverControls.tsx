/**
 * 故障转移控制块：统一开关（全部 app 一次生效）+ 主页面显示开关 + 首开确认。
 *
 * 宿主是「省心模式」tab —— 省心模式下队列是兜底，控制放一起；队列管理本身
 * 收在各 app 卡片的「高级」折叠里。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Zap, ShieldAlert } from "lucide-react";

import { Switch } from "@/components/ui/switch";
import { ToggleRow } from "@/components/ui/toggle-row";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useSetFailoverAll } from "@/lib/query/autoMode";
import { useAutoFailoverEnabled } from "@/lib/query/failover";
import type { SettingsFormState } from "@/hooks/useSettings";
import { PROXY_APP_IDS } from "@/config/appConfig";

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
