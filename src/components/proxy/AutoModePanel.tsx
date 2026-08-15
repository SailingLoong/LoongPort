/**
 * 自动模式面板（按 app）：系统按全局策略从托管档位里自动挑最合适的。
 *
 * 开启是一次性授权 —— 弹确认对话框说明「系统会自动切换档位（会话亲和保护
 * 进行中的会话）」，确认过一次就不再问（localStorage 记住，与故障转移的
 * failoverConfirmed 同一做法，只是这个状态纯属 UX，不值得进后端设置）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Sparkles, Loader2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  useAutoModeStatus,
  useSetAutoModeEnabled,
  useSetAutoModeStrategy,
} from "@/lib/query/autoMode";
import type { ProxyAppId } from "@/config/appConfig";
import { cn } from "@/lib/utils";

const CONFIRMED_STORAGE_KEY = "loongport.autoModeConfirmed";

interface AutoModePanelProps {
  appType: ProxyAppId;
  /** 接管未开时禁用（自动切换只发生在接管态，与故障转移同一条前置） */
  disabled?: boolean;
}

export function AutoModePanel({
  appType,
  disabled = false,
}: AutoModePanelProps) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(appType);
  const setEnabled = useSetAutoModeEnabled();
  const setStrategy = useSetAutoModeStrategy();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEnabled = status?.enabled ?? false;
  const strategy = status?.strategy ?? "cheapest";
  const isDisabled = disabled || setEnabled.isPending;

  const handleToggle = (checked: boolean) => {
    if (!checked) {
      setEnabled.mutate({ appType, enabled: false });
      return;
    }
    const confirmed = localStorage.getItem(CONFIRMED_STORAGE_KEY) === "true";
    if (confirmed) {
      setEnabled.mutate({ appType, enabled: true });
    } else {
      setShowConfirm(true);
    }
  };

  const handleConfirm = () => {
    localStorage.setItem(CONFIRMED_STORAGE_KEY, "true");
    setShowConfirm(false);
    setEnabled.mutate({ appType, enabled: true });
  };

  return (
    <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Sparkles className="h-4 w-4 text-emerald-500" />
          </div>
          <div className="space-y-1">
            <p className="text-sm font-medium leading-none">
              {t("autoMode.title", "自动模式")}
            </p>
            <p className="text-xs text-muted-foreground">
              {t(
                "autoMode.description",
                "系统从托管档位里自动挑最合适的，当前档位会话中保持不变",
              )}
            </p>
          </div>
        </div>
        {isLoading ? (
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        ) : (
          <Switch
            checked={isEnabled}
            onCheckedChange={handleToggle}
            disabled={isDisabled}
            aria-label={t("autoMode.title", "自动模式")}
          />
        )}
      </div>

      {/* 策略选择：全局共享，改一次对所有 app 生效 */}
      <div className="space-y-2">
        <Label className="text-xs text-muted-foreground">
          {t("autoMode.strategyLabel", "挑选策略（全局）")}
        </Label>
        <div className="grid grid-cols-2 gap-2">
          {(
            [
              {
                value: "cheapest",
                label: t("autoMode.strategy.cheapest", "价格最低"),
                hint: t("autoMode.strategy.cheapestHint", "按档位倍率升序"),
              },
              {
                value: "fastest",
                label: t("autoMode.strategy.fastest", "响应最快"),
                hint: t(
                  "autoMode.strategy.fastestHint",
                  "按近 7 天首字耗时升序",
                ),
              },
            ] as const
          ).map((option) => (
            <button
              key={option.value}
              type="button"
              disabled={!isEnabled || setStrategy.isPending}
              onClick={() => setStrategy.mutate({ strategy: option.value })}
              className={cn(
                "rounded-lg border p-2.5 text-left transition-colors disabled:opacity-50",
                strategy === option.value
                  ? "border-emerald-500/60 bg-emerald-500/10"
                  : "border-border hover:bg-muted/50",
              )}
            >
              <span className="block text-sm font-medium">{option.label}</span>
              <span className="block text-xs text-muted-foreground">
                {option.hint}
              </span>
            </button>
          ))}
        </div>
      </div>

      {isEnabled && (
        <Alert className="border-blue-500/40 bg-blue-500/10">
          <AlertDescription className="text-xs">
            {t(
              "autoMode.activeHint",
              "自动模式生效中，优先于故障转移队列。同一会话内保持当前档位不切换；当前档位持续失败时按策略顺序切换（切换会丢失提示词缓存）。",
            )}
          </AlertDescription>
        </Alert>
      )}

      <ConfirmDialog
        isOpen={showConfirm}
        variant="info"
        title={t("autoMode.confirm.title", "开启自动模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={handleConfirm}
        onCancel={() => setShowConfirm(false)}
      />
    </div>
  );
}
