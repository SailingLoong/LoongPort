/**
 * 主界面顶部的自动模式快速开关。
 *
 * 只做「随时关上」：自动模式的**开启入口只在设置页**（带一次性授权），
 * 这里仅在自动模式已生效时出现，关掉即隐藏（重新开启回设置页）。
 * 与 FailoverToggle 同一形态：图标 + 开关 + hover 说明。
 */

import { Sparkles, Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { useAutoModeStatus, useSetAutoModeEnabled } from "@/lib/query/autoMode";
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

  const isEnabled = status?.enabled ?? false;

  const tooltipText = t(
    "autoMode.tooltip",
    "自动模式生效中\n系统按策略自动挑选托管档位；同一会话保持当前档位不切换\n关闭后回到手动选择，可随时在设置中重新开启",
  );

  // 未开启时整个不渲染 —— 开启入口只在设置页（主动开启，不是默认行为）
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
      {setEnabled.isPending || isLoading ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <Sparkles className="h-4 w-4 text-emerald-500 status-heartbeat" />
      )}
      <Switch
        checked={isEnabled}
        onCheckedChange={(checked) => {
          // 只响应关：开启走设置页
          if (!checked) {
            setEnabled.mutate({ appType: activeApp, enabled: false });
          }
        }}
        disabled={setEnabled.isPending}
        aria-label={t("autoMode.title", "自动模式")}
      />
    </div>
  );
}
