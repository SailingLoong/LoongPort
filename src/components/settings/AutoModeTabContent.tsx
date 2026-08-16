/**
 * 「自动模式」设置页：用户只管 app 和模型，系统按全局策略（价格最低/响应
 * 最快）从托管档位里自动挑最合适的（Beta）。
 *
 * ## 为什么从路由 tab 迁出来
 *
 * 原先它藏在 设置→路由→「自动故障转移」折叠项→app 子标签 三层之下，且开关
 * 被「路由运行 + 接管」前置禁用成灰态 —— 用户找不到（2026-08-16 实反馈）。
 * 自动模式是**产品**，路由/接管/熔断是它的**基础设施**，层级应该反过来：
 * 这里是主入口，底层设置收在「高级路由」tab（过渡形态，最终并入高级 tab）。
 *
 * ## 一键开启
 *
 * 开关不再因前置未满足而变灰：点击时若路由未运行/未接管，弹一次性授权后
 * 由 [`useEnableAutoMode`] 顺带开启（复用既有命令，后端零新增编排）。
 *
 * ## 组件去向说明
 *
 * 故障转移队列（FailoverQueueManager）与熔断/超时（AutoFailoverConfigPanel）
 * 从原路由 tab 随迁为每张卡片的「高级」折叠 —— 自动模式优先于队列，
 * 两者是同一 app 的两档自动化，放一起才说得清。
 */

import { useState } from "react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { Sparkles, Loader2, ShieldAlert } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ToggleRow } from "@/components/ui/toggle-row";
import { FailoverQueueManager } from "@/components/proxy/FailoverQueueManager";
import { AutoFailoverConfigPanel } from "@/components/proxy/AutoFailoverConfigPanel";
import {
  hasConfirmedAutoMode,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";
import {
  useAutoModeStatus,
  useEnableAutoMode,
  useSetAutoModeEnabled,
  useSetAutoModeModel,
  useSetAutoModeStrategy,
} from "@/lib/query/autoMode";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { SettingsFormState } from "@/hooks/useSettings";
import {
  getAppLabel,
  PROXY_APP_IDS,
  type ProxyAppId,
} from "@/config/appConfig";
import { cn } from "@/lib/utils";

interface AutoModeTabContentProps {
  settings: SettingsFormState;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>;
}

/** Select 的「不限模型」哨兵值（Select 不能用空串当 value）。 */
const MODEL_ANY = "__any__";

export function AutoModeTabContent({
  settings,
  onAutoSave,
}: AutoModeTabContentProps) {
  const { t } = useTranslation();
  const setStrategy = useSetAutoModeStrategy();
  const [showFailoverConfirm, setShowFailoverConfirm] = useState(false);

  // strategy 是全局的，任一 app 的状态里都带同一份 —— 借第一个 app 当读取锚点。
  const { data: anchorStatus } = useAutoModeStatus(PROXY_APP_IDS[0]);
  const strategy = anchorStatus?.strategy ?? "cheapest";

  const handleFailoverToggleChange = (checked: boolean) => {
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
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      {/* 页头：定位说明 + Beta + 全局策略。策略全局一份，不按 app 重复。 */}
      <div className="rounded-xl glass-card p-6 space-y-4">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Sparkles className="h-5 w-5 text-emerald-500" />
          </div>
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <h3 className="text-base font-semibold">
                {t("autoMode.title", "自动模式")}
              </h3>
              <Badge variant="secondary">{t("autoMode.beta", "Beta")}</Badge>
            </div>
            <p className="text-sm text-muted-foreground">
              {t(
                "autoMode.tabDescription",
                "你只管选 app 和模型，系统按策略从托管档位里自动挑最合适的：同一会话保持当前档位（保护提示词缓存），失败自动切换下一档。开启时会顺带启用本地路由并接管该 CLI 的配置。",
              )}
            </p>
          </div>
        </div>

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
                disabled={setStrategy.isPending}
                onClick={() => setStrategy.mutate({ strategy: option.value })}
                className={cn(
                  "rounded-lg border p-2.5 text-left transition-colors disabled:opacity-50",
                  strategy === option.value
                    ? "border-emerald-500/60 bg-emerald-500/10"
                    : "border-border hover:bg-muted/50",
                )}
              >
                <span className="block text-sm font-medium">
                  {option.label}
                </span>
                <span className="block text-xs text-muted-foreground">
                  {option.hint}
                </span>
              </button>
            ))}
          </div>
        </div>
      </div>

      {PROXY_APP_IDS.map((appType) => (
        <AutoModeAppCard key={appType} appType={appType} />
      ))}

      {/* 主页面故障转移开关：随故障转移设置一起从原路由 tab 迁来。 */}
      <div className="rounded-xl glass-card p-6">
        <ToggleRow
          icon={<ShieldAlert className="h-4 w-4 text-orange-500" />}
          title={t("settings.advanced.proxy.enableFailoverToggle")}
          description={t(
            "settings.advanced.proxy.enableFailoverToggleDescription",
          )}
          checked={settings?.enableFailoverToggle ?? false}
          onCheckedChange={handleFailoverToggleChange}
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
    </motion.div>
  );
}

function AutoModeAppCard({ appType }: { appType: ProxyAppId }) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(appType);
  const { isRunning, takeoverStatus } = useProxyStatus();
  const setEnabled = useSetAutoModeEnabled();
  const enableFlow = useEnableAutoMode();
  const setModel = useSetAutoModeModel();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEnabled = status?.enabled ?? false;
  const model = status?.model ?? null;
  const availableModels = status?.availableModels ?? [];
  // 自动模式下队列/熔断只作兜底展示，仍要求接管态（与迁移前同判据）。
  const advancedDisabled = !isRunning || !(takeoverStatus?.[appType] ?? false);
  const isPending = setEnabled.isPending || enableFlow.isPending;

  const prerequisitesMet = !advancedDisabled;

  const doEnable = () => {
    if (prerequisitesMet) {
      setEnabled.mutate({ appType, enabled: true });
    } else {
      enableFlow.mutate({ appType });
    }
  };

  const handleToggle = (checked: boolean) => {
    if (!checked) {
      setEnabled.mutate({ appType, enabled: false });
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
    <div className="rounded-xl glass-card p-6 space-y-4">
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Sparkles
              className={cn(
                "h-4 w-4",
                isEnabled
                  ? "text-emerald-500 status-heartbeat"
                  : "text-muted-foreground",
              )}
            />
          </div>
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <p className="text-sm font-medium leading-none">
                {getAppLabel(appType)}
              </p>
              {isEnabled && (
                <Badge
                  variant="secondary"
                  className="bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
                >
                  {t("autoMode.statusActive", "生效中")}
                </Badge>
              )}
            </div>
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
            disabled={isPending}
            aria-label={t("autoMode.title", "自动模式")}
          />
        )}
      </div>

      {/* 模型偏好：只有存在模型目录的 app（Codex 系）才有这一栏。 */}
      {availableModels.length > 0 && (
        <div className="space-y-2">
          <Label className="text-xs text-muted-foreground">
            {t("autoMode.modelLabel", "模型偏好")}
          </Label>
          <Select
            value={model ?? MODEL_ANY}
            disabled={!isEnabled || setModel.isPending}
            onValueChange={(value) =>
              setModel.mutate({
                appType,
                model: value === MODEL_ANY ? null : value,
              })
            }
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={MODEL_ANY}>
                {t("autoMode.modelAny", "不限模型")}
              </SelectItem>
              {availableModels.map((name) => (
                <SelectItem key={name} value={name}>
                  {name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-xs text-muted-foreground">
            {t(
              "autoMode.modelHint",
              "选了模型后只在目录含该模型的档位里挑；显式点选会立即切到最优档位",
            )}
          </p>
        </div>
      )}

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

      {/* 高级：故障转移队列与熔断/超时（从原路由 tab 随迁）。 */}
      <Accordion type="single" collapsible>
        <AccordionItem value="advanced" className="border-none">
          <AccordionTrigger className="py-2 text-sm text-muted-foreground hover:no-underline">
            {t("autoMode.advancedSection", "高级（故障转移队列与熔断）")}
          </AccordionTrigger>
          <AccordionContent className="space-y-6 pt-4">
            <div className="space-y-4">
              <div>
                <h4 className="text-sm font-semibold">
                  {t("proxy.failoverQueue.title")}
                </h4>
                <p className="text-xs text-muted-foreground">
                  {t("proxy.failoverQueue.description")}
                </p>
              </div>
              <FailoverQueueManager
                appType={appType}
                disabled={advancedDisabled}
              />
            </div>
            <div className="border-t border-border/50 pt-6">
              <AutoFailoverConfigPanel
                appType={appType}
                disabled={advancedDisabled}
              />
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>

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
