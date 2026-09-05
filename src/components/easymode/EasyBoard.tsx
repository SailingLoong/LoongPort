/**
 * 首页省心视图：省心模式生效时替换该 app 的 provider 页。
 *
 * 用户只做三件事：选模型、选模式（自动/手动）；自动下选策略（省钱/省时），
 * 手动下拖动档位卡排序。全部档位事实来自后端看板（唯源），这里只渲染。
 */
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import {
  useAutoModeStatus,
  useSetAutoModeModel,
  useSetAutoModeStrategy,
  useSetEasyModeManualOrder,
  useSetEasyModeMode,
  useTierBoard,
} from "@/lib/query/autoMode";
import { useResetCircuitBreaker } from "@/lib/query/failover";
import { cn } from "@/lib/utils";
import { TierList } from "./TierList";

/** Select 不能用空串当 value 的哨兵（与 AutoModeTabContent 同一个约定）。 */
const MODEL_ANY = "__any__";

export function EasyBoard({ appId }: { appId: string }) {
  const { t } = useTranslation();
  const { data: board, isLoading } = useTierBoard(appId);
  const { isRunning, startProxyServer } = useProxyStatus();
  const { data: status } = useAutoModeStatus(appId);
  const setModel = useSetAutoModeModel();
  const setStrategy = useSetAutoModeStrategy();
  const setMode = useSetEasyModeMode();
  const setOrder = useSetEasyModeManualOrder();
  const resetBreaker = useResetCircuitBreaker();

  // 熔断/降级档位：右上角「重试全部」逐个清健康+熔断（单卡上另有单独按钮）
  const failedTiers = (board?.tiers ?? []).filter(
    (tier) =>
      tier.isHealthy === false ||
      tier.breakerState != null ||
      (tier.consecutiveFailures ?? 0) > 0,
  );
  const handleRetryAll = async () => {
    for (const tier of failedTiers) {
      await resetBreaker
        .mutateAsync({
          providerId: tier.providerId,
          appType: appId,
        })
        .catch(() => undefined);
    }
  };

  if (isLoading) {
    return (
      <div className="text-sm text-muted-foreground">
        {t("autoMode.board.loading", { defaultValue: "载入档位…" })}
      </div>
    );
  }
  if (!board) return null;

  const manual = board.mode === "manual";

  return (
    <div className="space-y-4">
      {!isRunning ? (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2">
          <p className="text-xs text-amber-600 dark:text-amber-400">
            {t("autoMode.runMode.routingStopped", {
              defaultValue: "本地路由未运行，流量不会经省心选路",
            })}
          </p>
          <button
            type="button"
            onClick={() => void startProxyServer()}
            className="rounded-md border px-2.5 py-1 text-xs transition-colors hover:bg-accent"
          >
            {t("autoMode.runMode.startRouting", {
              defaultValue: "启动本地路由",
            })}
          </button>
        </div>
      ) : null}
      <div className="flex flex-wrap items-center gap-3">
        {board.availableModels.length > 0 ? (
          <Select
            value={board.model ?? MODEL_ANY}
            disabled={setModel.isPending}
            onValueChange={(value) =>
              setModel.mutate({
                appType: appId,
                model: value === MODEL_ANY ? null : value,
              })
            }
          >
            <SelectTrigger className="w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={MODEL_ANY}>
                {t("autoMode.modelAny", { defaultValue: "不限模型" })}
              </SelectItem>
              {board.availableModels.map((model) => (
                <SelectItem key={model} value={model}>
                  {model}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}

        <div className="grid grid-cols-2 gap-2">
          <ChoiceButton
            active={!manual}
            disabled={setMode.isPending}
            onClick={() => setMode.mutate({ appType: appId, mode: "auto" })}
          >
            {t("autoMode.board.modeAuto", { defaultValue: "自动" })}
          </ChoiceButton>
          <ChoiceButton
            active={manual}
            disabled={setMode.isPending || !board.tiers.length}
            onClick={() => setMode.mutate({ appType: appId, mode: "manual" })}
          >
            {t("autoMode.board.modeManual", { defaultValue: "手动排序" })}
          </ChoiceButton>
        </div>

        {!manual ? (
          <div className="grid grid-cols-2 gap-2">
            <ChoiceButton
              active={board.strategy === "cheapest"}
              disabled={setStrategy.isPending}
              onClick={() => setStrategy.mutate({ strategy: "cheapest" })}
            >
              {t("autoMode.strategy.cheapest", { defaultValue: "省钱" })}
            </ChoiceButton>
            <ChoiceButton
              active={board.strategy === "fastest"}
              disabled={setStrategy.isPending}
              onClick={() => setStrategy.mutate({ strategy: "fastest" })}
            >
              {t("autoMode.strategy.fastest", { defaultValue: "省时" })}
            </ChoiceButton>
          </div>
        ) : null}

        {failedTiers.length > 0 ? (
          <button
            type="button"
            onClick={() => void handleRetryAll()}
            disabled={resetBreaker.isPending}
            className="ml-auto inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent disabled:opacity-50"
          >
            <RefreshCw
              className={cn(
                "h-3.5 w-3.5",
                resetBreaker.isPending && "animate-spin",
              )}
            />
            {t("autoMode.board.retryAll", {
              defaultValue: "重试全部熔断档位",
            })}
          </button>
        ) : null}
      </div>

      {manual ? (
        <p className="text-xs text-muted-foreground">
          {t("autoMode.board.dragHint", {
            defaultValue: "拖动卡片调整优先级，故障自动落下一家并回切",
          })}
        </p>
      ) : null}

      {board.tiers.length === 0 ? (
        <div className="rounded-lg border border-dashed p-8 text-center text-sm text-muted-foreground">
          {t("autoMode.board.empty", { defaultValue: "还没有可用档位" })}
        </div>
      ) : (
        <TierList
          tiers={board.tiers}
          manual={manual}
          appType={appId}
          onReorder={(orderedIds) =>
            setOrder.mutate({ appType: appId, orderedIds })
          }
        />
      )}

      {!status?.cliInstalled ? (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          {t("autoMode.cliMissingHint", {
            defaultValue: "该 CLI 未安装，接管不会生效",
          })}
        </p>
      ) : null}
    </div>
  );
}

/** 二选一按钮（形状与 AutoModeTabContent 的策略选择器一致）。 */
function ChoiceButton({
  active,
  disabled,
  onClick,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={
        active
          ? "rounded-md border border-emerald-500/60 bg-emerald-500/10 px-3 py-1.5 text-sm text-emerald-600 transition-colors dark:text-emerald-400"
          : "rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent"
      }
    >
      {children}
    </button>
  );
}
