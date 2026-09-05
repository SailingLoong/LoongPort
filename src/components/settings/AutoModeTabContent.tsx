/**
 * 「省心模式」设置页：用户只管 app 和模型，系统按全局策略（价格最低/响应
 * 最快）从托管档位里自动挑最合适的。
 *
 * ## 布局语义（2026-08-17 定稿）
 *
 * - **总开关 = 省心模式本身 = 本地路由**（`useSetEasyModeMaster`）：开 = 起
 *   路由服务；关 = 全部 app 关省心 + 恢复配置停服务。只有总开关开着，各
 *   app 卡片的细节数据才可配置 —— 大开关管路由，小开关管各 app。
 * - **单卡关闭**只收回该 app 的省心模式与接管（`useDisableAutoMode`），
 *   不停路由（停不停由总开关说了算）。
 * - **自动故障转移是统一入口**（本页底部一行，作用于全部 app）：原先每个
 *   app 的队列里各一个开关，重复且易漏；队列管理本身仍按 app 收在各卡
 *   「高级」折叠里。
 */

import { useState } from "react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { Sparkles, Loader2 } from "lucide-react";

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
import { FailoverQueueManager } from "@/components/proxy/FailoverQueueManager";
import { AutoFailoverConfigPanel } from "@/components/proxy/AutoFailoverConfigPanel";
import {
  hasConfirmedAutoMode,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";
import { FailoverControls } from "@/components/settings/FailoverControls";
import {
  useAutoModeStatus,
  useSetEasyModeMaster,
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
  const masterFlow = useSetEasyModeMaster();
  const [showMasterConfirm, setShowMasterConfirm] = useState(false);

  // strategy 是全局的，任一 app 的状态里都带同一份 —— 借第一个 app 当读取锚点。
  const anchorStatus = useAutoModeStatus(PROXY_APP_IDS[0]).data;
  const strategy = anchorStatus?.strategy ?? "cheapest";

  // 总开关语义（2026-08-17 定稿）= 「是否开启省心模式」=「是否开启本地路由」：
  // 显示值就是路由服务的运行态；只有它开着，各 app 卡片才可配置（开=起路由，
  // 关=全部 app 关省心 + 恢复配置停服务，见 useSetEasyModeMaster）。
  const { isRunning } = useProxyStatus();

  const handleMasterChange = (checked: boolean) => {
    if (!checked) {
      masterFlow.mutate({ enable: false });
      return;
    }
    // 首次开启走一次性授权；已授权或服务已在跑则直接开。
    if (!isRunning && !hasConfirmedAutoMode()) {
      setShowMasterConfirm(true);
    } else {
      masterFlow.mutate({ enable: true });
    }
  };

  const handleMasterConfirm = () => {
    markAutoModeConfirmed();
    setShowMasterConfirm(false);
    masterFlow.mutate({ enable: true });
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      {/* 页头：定位说明 + 总开关 + 全局策略（策略全局一份，不按 app 重复）。 */}
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
            </div>
            <p className="text-sm text-muted-foreground">
              {t(
                "autoMode.tabDescription",
                "你只管选 app 和模型，系统按策略从托管档位里自动挑最合适的：同一会话保持当前档位（保护提示词缓存），失败自动切换下一档。档位倍率与模型单价以站点实时数据验证，所选模型均经「模型验真」确认真实可用。开启时会顺带启用本地路由并接管该 CLI 的配置，关闭时一并恢复。",
              )}
            </p>
          </div>
          {masterFlow.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Switch
              checked={isRunning}
              onCheckedChange={handleMasterChange}
              disabled={masterFlow.isPending}
              aria-label={t("autoMode.masterLabel", "省心模式总开关")}
            />
          )}
        </div>

        <p className="text-xs text-muted-foreground">
          {t(
            "autoMode.masterHint",
            "总开关就是本地路由：开启后才能为各应用打开省心模式；关闭时会关闭全部应用的省心模式、恢复配置并停止路由",
          )}
        </p>

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

      {/* 故障转移控制（统一开关 + 主页面显示开关）：与「高级」tab 的发布形态
          共用一份组件，见 FailoverControls 的文档。 */}
      <FailoverControls settings={settings} onAutoSave={onAutoSave} />

      <ConfirmDialog
        isOpen={showMasterConfirm}
        variant="info"
        title={t("autoMode.confirm.title", "开启省心模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。若本地路由未开启，将一并开启并接管该 CLI 的配置（关闭省心模式时会一并恢复）。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={handleMasterConfirm}
        onCancel={() => setShowMasterConfirm(false)}
      />
    </motion.div>
  );
}

function AutoModeAppCard({ appType }: { appType: ProxyAppId }) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(appType);
  const { isRunning, takeoverStatus } = useProxyStatus();
  const setModel = useSetAutoModeModel();

  const isEnabled = status?.enabled ?? false;
  const hasCandidates = status?.hasCandidates ?? false;
  const cliInstalled = status?.cliInstalled ?? false;
  const model = status?.model ?? null;
  const availableModels = status?.availableModels ?? [];
  // 队列/熔断只作兜底展示，仍要求接管态（与迁移前同判据）。
  const advancedDisabled = !isRunning || !(takeoverStatus?.[appType] ?? false);

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
          <span
            className={
              isEnabled
                ? "rounded-full border border-emerald-500/60 bg-emerald-500/10 px-2 py-0.5 text-xs text-emerald-600 dark:text-emerald-400"
                : "rounded-full border px-2 py-0.5 text-xs text-muted-foreground"
            }
          >
            {isEnabled
              ? t("autoMode.runMode.easy", { defaultValue: "省心" })
              : t("autoMode.runMode.self", { defaultValue: "自主" })}
          </span>
        )}
      </div>
      <p className="text-xs text-muted-foreground">
        {t("autoMode.tab.switchMovedHint", {
          defaultValue: "省心 / 自主的切换入口在主页面顶部",
        })}
      </p>

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
    </div>
  );
}
