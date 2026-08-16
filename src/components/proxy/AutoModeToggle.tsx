/**
 * 主界面顶部的自动模式快速开关（当前 app 维度）。
 *
 * 双向都能操作：开启走与设置页同一套「一次性授权 + 一键编排」（未开路由/
 * 接管时顺带开启，见 [`useEnableAutoMode`]），关闭维持原路径。此前这里只在
 * 自动模式已生效时出现且只能关 —— 用户得先去设置页三层深的位置找开启入口
 * （2026-08-16 实反馈「找不到开关」），现在顶栏就是完整入口。
 * 与 FailoverToggle 同一形态：图标 + 开关 + hover 说明。
 */

import { useState } from "react";
import { Sparkles, Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  useAutoModeStatus,
  useEnableAutoMode,
  useSetAutoModeEnabled,
} from "@/lib/query/autoMode";
import {
  hasConfirmedAutoMode,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import { cn } from "@/lib/utils";
import { useTranslation } from "react-i18next";
import type { ProxyAppId } from "@/config/appConfig";

interface AutoModeToggleProps {
  className?: string;
  activeApp: ProxyAppId;
}

export function AutoModeToggle({ className, activeApp }: AutoModeToggleProps) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(activeApp);
  const setEnabled = useSetAutoModeEnabled();
  const enableFlow = useEnableAutoMode();
  const { isRunning, takeoverStatus } = useProxyStatus();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEnabled = status?.enabled ?? false;
  const isPending = setEnabled.isPending || enableFlow.isPending;
  const prerequisitesMet = isRunning && (takeoverStatus?.[activeApp] ?? false);

  const tooltipText = isEnabled
    ? t(
        "autoMode.tooltip",
        "自动模式生效中（Beta）\n系统按策略自动挑选托管档位；同一会话保持当前档位不切换\n关闭后回到手动选择，可随时重新开启",
      )
    : t(
        "autoMode.tooltipOff",
        "自动模式（Beta）\n系统按策略自动挑选托管档位，同一会话保持不变\n开启时若本地路由未启用会一并开启",
      );

  const doEnable = () => {
    if (prerequisitesMet) {
      setEnabled.mutate({ appType: activeApp, enabled: true });
    } else {
      enableFlow.mutate({ appType: activeApp });
    }
  };

  const handleCheckedChange = (checked: boolean) => {
    if (!checked) {
      setEnabled.mutate({ appType: activeApp, enabled: false });
      return;
    }
    if (hasConfirmedAutoMode()) {
      doEnable();
    } else {
      setShowConfirm(true);
    }
  };

  const handleConfirm = () => {
    markAutoModeConfirmed();
    setShowConfirm(false);
    doEnable();
  };

  return (
    <div
      className={cn(
        "flex items-center gap-1 px-1.5 h-8 rounded-lg bg-muted/50 transition-all",
        className,
      )}
      title={tooltipText}
    >
      {isPending || isLoading ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <Sparkles
          className={cn(
            "h-4 w-4",
            isEnabled
              ? "text-emerald-500 status-heartbeat"
              : "text-muted-foreground",
          )}
        />
      )}
      <Switch
        checked={isEnabled}
        onCheckedChange={handleCheckedChange}
        disabled={isPending}
        aria-label={t("autoMode.title", "自动模式")}
      />

      <ConfirmDialog
        isOpen={showConfirm}
        variant="info"
        title={t("autoMode.confirm.title", "开启自动模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。若本地路由未开启，将一并开启并接管该 CLI 的配置。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={handleConfirm}
        onCancel={() => setShowConfirm(false)}
      />
    </div>
  );
}
