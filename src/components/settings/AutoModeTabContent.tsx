/**
 * 「省心模式」设置页：用户只管 app 和模型，系统按全局策略（价格最低/响应
 * 最快）从托管档位里自动挑最合适的（Beta）。
 *
 * ## 布局语义
 *
 * - **总开关**在页头：一次开/关全部有托管档位的 app；逐 app 的卡片开关仍是
 *   细粒度入口。主入口只在设置页 —— 顶栏开关默认不展示（生效时才出现，
 *   只做随时关上）。
 * - **关闭即收回路由**：关省心模式（无论总开关还是单卡）默认把该 app 的
 *   路由接管一并关掉（`useDisableAutoMode`，与开启编排对称）。
 * - **自动故障转移是统一入口**（本页底部一行，作用于全部 app）：原先每个
 *   app 的队列里各一个开关，重复且易漏；队列管理本身仍按 app 收在各卡
 *   「高级」折叠里。
 */

import { useState } from "react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { Sparkles, Loader2, ShieldAlert, Zap } from "lucide-react";

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
  useDisableAutoMode,
  useEnableAutoMode,
  useSetAutoModeEnabled,
  useSetAutoModeAll,
  useSetAutoModeModel,
  useSetAutoModeStrategy,
  useSetFailoverAll,
} from "@/lib/query/autoMode";
import { useAutoFailoverEnabled } from "@/lib/query/failover";
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
  const setAll = useSetAutoModeAll();
  const setFailoverAll = useSetFailoverAll();
  const [showFailoverConfirm, setShowFailoverConfirm] = useState(false);

  // strategy 是全局的，任一 app 的状态里都带同一份 —— 借第一个 app 当读取锚点。
  const anchorStatus = useAutoModeStatus(PROXY_APP_IDS[0]).data;
  const strategy = anchorStatus?.strategy ?? "cheapest";

  // 总开关的状态语义：全部「真正能开」的 app（有托管档位 **且** CLI 装过）
  // 都开着 = 开。只看档位会把 CLI 未装的 app 也算进来 —— 那个永远开不了，
  // 总开关就永远「开了又弹回」（2026-08-17 实测症状）。各 app 状态由卡片
  // 各自订阅（同 query key 共享缓存），这里拿全集算总开关的显示值。
  const statuses = PROXY_APP_IDS.map((appType) => ({
    appType,
    status: useAutoModeStatus(appType).data,
  }));
  const eligible = statuses.filter(
    ({ status }) => status?.hasCandidates && (status?.cliInstalled ?? false),
  );
  const masterChecked =
    eligible.length > 0 &&
    eligible.every(({ status }) => status?.enabled ?? false);

  // 统一故障转移开关的状态：全部 app 都开 = 开。
  const failoverStates = PROXY_APP_IDS.map(
    (appType) => useAutoFailoverEnabled(appType).data ?? false,
  );
  const failoverChecked = failoverStates.every(Boolean);

  const handleMasterChange = (checked: boolean) => {
    if (checked) {
      setAll.mutate({
        apps: eligible.map(({ appType }) => appType),
        enable: true,
      });
    } else {
      setAll.mutate({
        apps: statuses
          .filter(({ status }) => status?.enabled)
          .map(({ appType }) => appType),
        enable: false,
      });
    }
  };

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
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      {/* 页头：定位说明 + Beta + 总开关 + 全局策略（策略全局一份，不按 app 重复）。 */}
      <div className="rounded-xl glass-card p-6 space-y-4">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Sparkles className="h-5 w-5 text-emerald-500" />
          </div>
          <div className="space-y-1 flex-1">
            <div className="flex items-center gap-2">
              <h3 className="text-base font-semibold">
                {t("autoMode.title", "省心模式")}
              </h3>
              <Badge variant="secondary">{t("autoMode.beta", "Beta")}</Badge>
            </div>
            <p className="text-sm text-muted-foreground">
              {t(
                "autoMode.tabDescription",
                "你只管选 app 和模型，系统按策略从托管档位里自动挑最合适的：同一会话保持当前档位（保护提示词缓存），失败自动切换下一档。档位倍率与模型单价以站点实时数据验证，所选模型均经「模型验真」确认真实可用。开启时会顺带启用本地路由并接管该 CLI 的配置，关闭时一并恢复。",
              )}
            </p>
          </div>
          {setAll.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Switch
              checked={masterChecked}
              onCheckedChange={handleMasterChange}
              disabled={setAll.isPending || eligible.length === 0}
              aria-label={t("autoMode.masterLabel", "省心模式总开关")}
            />
          )}
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

      {/* 统一的自动故障转移开关：作用于全部 app（队列管理仍在各卡片「高级」里）。 */}
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
    </motion.div>
  );
}

function AutoModeAppCard({ appType }: { appType: ProxyAppId }) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(appType);
  const { isRunning, takeoverStatus } = useProxyStatus();
  const setEnabled = useSetAutoModeEnabled();
  const enableFlow = useEnableAutoMode();
  const disableFlow = useDisableAutoMode();
  const setModel = useSetAutoModeModel();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEnabled = status?.enabled ?? false;
  const hasCandidates = status?.hasCandidates ?? false;
  const cliInstalled = status?.cliInstalled ?? false;
  const model = status?.model ?? null;
  const availableModels = status?.availableModels ?? [];
  // 队列/熔断只作兜底展示，仍要求接管态（与迁移前同判据）。
  const advancedDisabled = !isRunning || !(takeoverStatus?.[appType] ?? false);
  const isPending =
    setEnabled.isPending || enableFlow.isPending || disableFlow.isPending;

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
      // 关闭默认连路由接管一起收回（开启编排的对称面）。
      disableFlow.mutate({ appType });
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
              {!hasCandidates
                ? t(
                    "autoMode.noCandidatesHint",
                    "没有可用的托管档位 —— 先在中转站区登录并获取档位",
                  )
                : !cliInstalled
                  ? t(
                      "autoMode.cliMissingHint",
                      "未检测到该 CLI 的配置文件（未安装或未初始化），无法接管；安装并运行一次该 CLI 后再来开启",
                    )
                  : t(
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
            disabled={isPending || !hasCandidates || !cliInstalled}
            aria-label={t("autoMode.title", "省心模式")}
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
              "省心模式生效中，优先于故障转移队列。同一会话内保持当前档位不切换；当前档位持续失败时按策略顺序切换（切换会丢失提示词缓存）。档位价格经站点倍率验证，模型经「模型验真」确认。",
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
        title={t("autoMode.confirm.title", "开启省心模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。若本地路由未开启，将一并开启并接管该 CLI 的配置（关闭省心模式时会一并恢复）。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={handleConfirm}
        onCancel={() => setShowConfirm(false)}
      />
    </div>
  );
}
