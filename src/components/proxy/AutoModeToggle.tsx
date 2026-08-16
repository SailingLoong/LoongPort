/**
 * 主界面顶部的省心模式快速开关（当前 app 维度）。
 *
 * 常驻双态开关：开启与设置页同一编排（首次开启弹同款一次性授权），关闭走
 * [`useDisableAutoMode`] 连路由接管一并收回。此前「生效时才出现、关掉即消失」
 * 的显隐会让顶栏布局跳变，用户实测后否决（#168 决策修订）。无托管档位时
 * 开不动（灰化）但随时可关。与 FailoverToggle 同一形态：图标 + 开关 + hover 说明。
 */

import { useState } from "react";
import { Sparkles, Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  hasConfirmedAutoMode,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";
import {
  useAutoModeStatus,
  useDisableAutoMode,
  useEnableAutoMode,
} from "@/lib/query/autoMode";
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
  const enableFlow = useEnableAutoMode();
  const disableFlow = useDisableAutoMode();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEnabled = status?.enabled ?? false;
  const hasCandidates = status?.hasCandidates ?? false;
  const isPending = enableFlow.isPending || disableFlow.isPending;

  const tooltipText = t(
    "autoMode.tooltip",
    "省心模式（Beta）：开启后系统按策略自动挑选托管档位，同一会话保持当前档位不切换；关闭时一并恢复该 CLI 的路由接管",
  );

  const handleToggle = (checked: boolean) => {
    if (!checked) {
      disableFlow.mutate({ appType: activeApp });
      return;
    }
    if (hasConfirmedAutoMode()) {
      enableFlow.mutate({ appType: activeApp });
    } else {
      setShowConfirm(true);
    }
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
        onCheckedChange={handleToggle}
        // 开启需要托管档位（与设置页同判据）；关闭随时可用
        disabled={isPending || (!isEnabled && !hasCandidates)}
        aria-label={t("autoMode.title", "省心模式")}
      />
      <ConfirmDialog
        isOpen={showConfirm}
        variant="info"
        title={t("autoMode.confirm.title", "开启省心模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。若本地路由未开启，将一并开启并接管该 CLI 的配置（关闭省心模式时会一并恢复）。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={() => {
          markAutoModeConfirmed();
          setShowConfirm(false);
          enableFlow.mutate({ appType: activeApp });
        }}
        onCancel={() => setShowConfirm(false)}
      />
    </div>
  );
}
