/**
 * 模型验真的行级宿主：summaries 拉取、验证弹窗状态与结果变化事件订阅全部
 * 收在这里。中转站区块（`RelaySection`）只挂这一个组件并传入档位 provider 集；
 * 行内的入口/徽章组件通过 context 消费 —— 验真状态不再穿过
 * `RelayTierList` → `RelayRow` 三层 prop 钻透。
 *
 * 功能下线时（`MODEL_VERIFICATION_ENABLED = false`）：不拉取、不订阅，
 * context 恒返回「无结论、无任务」，所有验真 UI 默认不渲染。
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  modelVerificationApi,
  type VerificationReport,
  type VerificationScope,
  type VerificationScopeSummary,
  type VerificationVerdict,
} from "@/lib/api/modelVerification";
import { MODEL_VERIFICATION_CHANGED } from "@/lib/api/events";
import { useTauriEvent } from "@/hooks/useTauriEvent";

import { MODEL_VERIFICATION_ENABLED } from "./availability";
import { ModelVerificationDialog } from "./ModelVerificationDialog";

/** 打开验证弹窗所需的最小档位事实；`TierInfo` 结构性满足它。 */
export interface VerificationTierRef {
  providerId: string;
  displayName: string;
}

interface TierVerificationContextValue {
  /** 档位当前的有限验真结论；无结论/功能下线时为 undefined。 */
  verdictFor: (providerId: string) => VerificationVerdict | undefined;
  /** 打开该档位的模型验证弹窗。 */
  openVerification: (tier: VerificationTierRef) => void;
  /** 该档位是否正有验证任务在跑（用于行内 spinner 与操作组钉住）。 */
  isVerifying: (providerId: string) => boolean;
}

const TierVerificationContext =
  createContext<TierVerificationContextValue | null>(null);

export function useTierVerification(): TierVerificationContextValue {
  const value = useContext(TierVerificationContext);
  // 没挂 Provider 却用入口/徽章 = 接线错误，直接抛错而不是静默不渲染。
  if (!value) {
    throw new Error(
      "useTierVerification 必须在 TierVerificationProvider 内使用",
    );
  }
  return value;
}

export function TierVerificationProvider({
  appId,
  providerIds,
  children,
}: {
  appId: string;
  providerIds: string[];
  children: ReactNode;
}) {
  const [summaries, setSummaries] = useState<
    Record<string, VerificationScopeSummary>
  >({});
  const [selectedTier, setSelectedTier] = useState<VerificationTierRef | null>(
    null,
  );
  const [dialogOpen, setDialogOpen] = useState(false);
  const [verifyingProviderId, setVerifyingProviderId] = useState<string | null>(
    null,
  );
  const requestRef = useRef(0);
  const providerIdsRef = useRef(providerIds);
  providerIdsRef.current = providerIds;

  const load = useCallback(async () => {
    if (!MODEL_VERIFICATION_ENABLED) return;
    const request = ++requestRef.current;
    const current = providerIdsRef.current;
    if (current.length === 0) {
      setSummaries({});
      return;
    }
    try {
      const result = await modelVerificationApi.listSummaries(current, appId);
      if (request !== requestRef.current) return;
      setSummaries(
        Object.fromEntries(
          result.map((summary) => [summary.providerId, summary]),
        ),
      );
    } catch {
      // summaries 只是次要状态：失败时保留最近一次完整的后端视图。
    }
  }, [appId]);

  useEffect(() => {
    void load();
  }, [load, providerIds]);

  // 只在事件属于本 tab 且目标档位确实在本区时才重拉，避免切别的 app 的
  // 供应商也触发这里多跑一轮请求。
  useTauriEvent<VerificationScope>(MODEL_VERIFICATION_CHANGED, (scope) => {
    if (
      scope?.appType !== appId ||
      !providerIdsRef.current.includes(scope.providerId)
    ) {
      return;
    }
    void load();
  });

  const openVerification = useCallback(
    (tier: VerificationTierRef) => {
      if (
        verifyingProviderId !== null &&
        selectedTier?.providerId !== tier.providerId
      ) {
        return;
      }
      setSelectedTier(tier);
      setDialogOpen(true);
    },
    [selectedTier, verifyingProviderId],
  );

  const handleRunningChange = useCallback(
    (running: boolean) => {
      setVerifyingProviderId(
        running ? (selectedTier?.providerId ?? null) : null,
      );
    },
    [selectedTier],
  );

  const value = useMemo<TierVerificationContextValue>(
    () => ({
      verdictFor: (providerId) =>
        summaries[providerId]?.badgeVerdict ?? undefined,
      openVerification,
      isVerifying: (providerId) => providerId === verifyingProviderId,
    }),
    [summaries, openVerification, verifyingProviderId],
  );

  const selectedReport: VerificationReport | null = selectedTier
    ? (summaries[selectedTier.providerId]?.representativeReport ?? null)
    : null;

  return (
    <TierVerificationContext.Provider value={value}>
      {children}
      {selectedTier && (
        <ModelVerificationDialog
          key={`${selectedTier.providerId}:${appId}`}
          providerId={selectedTier.providerId}
          appType={appId}
          tierDisplayName={selectedTier.displayName}
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          onRunningChange={handleRunningChange}
          report={selectedReport}
        />
      )}
    </TierVerificationContext.Provider>
  );
}
