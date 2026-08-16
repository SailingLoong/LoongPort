/**
 * 主界面顶部的省心模式快速开关（当前 app 维度）。
 *
 * 主入口在设置页（总开关 + 一次性授权）；这里**默认不展示** —— 仅在当前
 * app 的省心模式已生效时出现，做「随时关上」。关闭走 [`useDisableAutoMode`]：
 * 连该 app 的路由接管一并收回（开启编排的对称面）。重新开启回设置页。
 * 与 FailoverToggle 同一形态：图标 + 开关 + hover 说明。
 */

import { Sparkles, Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { useAutoModeStatus, useDisableAutoMode } from "@/lib/query/autoMode";
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
  const disableFlow = useDisableAutoMode();

  const isEnabled = status?.enabled ?? false;

  const tooltipText = t(
    "autoMode.tooltip",
    "省心模式生效中（Beta）\n系统按策略自动挑选托管档位；同一会话保持当前档位不切换\n关闭时将一并恢复该 CLI 的路由接管；重新开启请到设置页",
  );

  // 默认不展示：主入口在设置页；这里只在生效中出现，随时关上。
  if (!isEnabled) {
    return null;
  }

  return (
    <div
      className={cn(
        "flex items-center gap-1 px-1.5 h-8 rounded-lg bg-muted/50 transition-all",
        className,
      )}
      title={tooltipText}
    >
      {disableFlow.isPending || isLoading ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <Sparkles className="h-4 w-4 text-emerald-500 status-heartbeat" />
      )}
      <Switch
        checked={isEnabled}
        onCheckedChange={(checked) => {
          if (!checked) {
            disableFlow.mutate({ appType: activeApp });
          }
        }}
        disabled={disableFlow.isPending}
        aria-label={t("autoMode.title", "省心模式")}
      />
    </div>
  );
}
