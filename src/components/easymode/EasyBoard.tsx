/**
 * 首页省心视图：省心模式生效时替换该 app 的 provider 页。
 *
 * 应用内选择模型与自动/手动模式；全局策略只读展示并链接设置，
 * 手动模式可拖动档位排序。全部档位事实来自后端看板。
 */
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { ModelPicker } from "./ModelPicker";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import {
  useAutoModeStatus,
  useSetAutoModeModel,
  useSetEasyModeManualOrder,
  useSetEasyModeMode,
  useTierBoard,
} from "@/lib/query/autoMode";
import { useResetCircuitBreaker } from "@/lib/query/failover";
import { cn } from "@/lib/utils";
import { SelfManagedBar } from "./SelfManagedBar";
import { TierList } from "./TierList";

export function EasyBoard({
  appId,
  onOpenSettings,
}: {
  appId: string;
  onOpenSettings: () => void;
}) {
  const { t } = useTranslation();
  const { data: board, isLoading } = useTierBoard(appId);
  const { status, isRunning, startProxyServer } = useProxyStatus();
  const { data: autoStatus } = useAutoModeStatus(appId);
  const setModel = useSetAutoModeModel();
  const setMode = useSetEasyModeMode();
  const setOrder = useSetEasyModeManualOrder();
  const resetBreaker = useResetCircuitBreaker();

  // The backend target can change on a hot switch before any request succeeds.
  // It is separate from the persisted configuration and is not request history.
  const activeProviderId = status?.active_targets?.find(
    (target) => target.app_type === appId,
  )?.provider_id;

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
      <SelfManagedBar appId={appId} />
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
        {board.modelOptions.length > 0 ? (
          <ModelPicker
            model={board.model}
            modelOptions={board.modelOptions}
            disabled={setModel.isPending}
            onSelect={(model) => setModel.mutate({ appType: appId, model })}
          />
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
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="text-muted-foreground">
              {t("autoMode.board.globalStrategy", { defaultValue: "全局策略" })}
            </span>
            <span>{t(`autoMode.strategy.${board.strategy}`)}</span>
            <button
              type="button"
              onClick={onOpenSettings}
              className="rounded-md border px-2.5 py-1 text-xs transition-colors hover:bg-accent"
            >
              {t("autoMode.board.strategySettings", {
                defaultValue: "全局选路设置",
              })}
            </button>
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
          activeProviderId={activeProviderId}
          onReorder={(orderedIds) =>
            setOrder.mutate({ appType: appId, orderedIds })
          }
        />
      )}

      {autoStatus?.cliInstalled === false ? (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          {t("client.configMissing")}
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
