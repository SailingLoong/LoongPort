import React from "react";
import { RefreshCw, AlertCircle, Clock } from "lucide-react";
import { useTranslation } from "react-i18next";
import { type AppId } from "@/lib/api";
import { useUsageQuery } from "@/lib/query/queries";
import { UsageData, UsageResult, Provider } from "@/types";
import { TierBadge } from "@/components/SubscriptionQuotaFooter";
import type { QuotaTier } from "@/types/subscription";

interface UsageFooterProps {
  provider: Provider;
  providerId: string;
  appId: AppId;
  usageEnabled: boolean; // 是否启用了用量查询
  isCurrent: boolean; // 是否为当前激活的供应商
  isInConfig?: boolean; // OpenCode: 是否已添加到配置
  inline?: boolean; // 是否内联显示（在按钮左侧）
}

/** UsageData → QuotaTier 转换（Token Plan 使用） */
function toQuotaTier(data: UsageData): QuotaTier {
  const extra = data.extra;
  if (extra && extra.startsWith("{")) {
    try {
      const parsed = JSON.parse(extra);
      return {
        name: data.planName || "",
        utilization: data.used || 0,
        resetsAt: parsed.resetsAt || null,
        usedValueUsd: parsed.usedValueUsd ?? null,
        maxValueUsd: parsed.maxValueUsd ?? null,
        planLabel: parsed.planLabel ?? null,
      };
    } catch {
      // fall through to plain string
    }
  }
  return {
    name: data.planName || "",
    utilization: data.used || 0,
    resetsAt: extra || null,
  };
}

/** [`InlineUsage`] 的入参。刻意**不含**「怎么拿到这些值」—— 那是调用方的事。 */
export interface InlineUsageProps {
  /** 用量结果。`undefined` = 还没有任何可显示的值（首查失败且无缓存）。 */
  usage?: UsageResult;
  /** 正在查（刷新按钮转圈 + 禁用）。 */
  loading: boolean;
  /** 上次**成功**查询的时间戳，`null` = 从未。 */
  lastQueriedAt: number | null;
  onRefresh: () => unknown | Promise<unknown>;
  /** 刷新按钮的完整语义。省略时沿用 provider 页的「刷新用量」。 */
  refreshLabel?: string;
  /** 把更新时间、刷新与用量压在同一行。LoongPort 行使用，provider 页保持原布局。 */
  singleLine?: boolean;
  /** 是否显示服务端返回的已用值。 */
  showUsed?: boolean;
}

/**
 * provider 卡片右侧那条内联用量条：上次查询时间 + 手动刷新按钮 / 已用·剩余·单位。
 *
 * ## 为什么抽出来 export
 *
 * LoongPort 的「中转站 × 分组」页与「官网 API」块要显示同一样东西（维护者的要求是
 * 「复用 provider 页的样子」）。抽成子组件而不是在那边照抄一份 JSX —— 照抄的那份
 * 会在上游改这里时静默分叉。仓内已有同形先例：`SubscriptionQuotaFooter` export 了
 * `TierBadge` 给本文件用。
 *
 * ## 失败态也在这里，不在调用方
 *
 * 「查失败」必须仍然渲染出**带刷新按钮的一行**，否则整块消失、用户无从重查
 * （见下方 `usageEnabled` 那处注释里的同一条推理）。把它留在组件内，调用方就没有
 * 「忘了处理失败态」这个选项。
 */
export const InlineUsage: React.FC<InlineUsageProps> = ({
  usage,
  loading,
  lastQueriedAt,
  onRefresh,
  refreshLabel,
  singleLine = false,
  showUsed = true,
}) => {
  const { t } = useTranslation();

  // 相对时间要自己走动，否则「1 分钟前」会一直停在那儿直到别的 state 变化。
  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    if (!lastQueriedAt) return;
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [lastQueriedAt]);

  const refresh = (e: React.MouseEvent) => {
    // 行本身可点（切档位 / 展开），刷新不该顺带触发它。
    e.stopPropagation();
    onRefresh();
  };

  const refreshButton = (
    <button
      onClick={refresh}
      disabled={loading}
      className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0 text-muted-foreground"
      title={refreshLabel ?? t("usage.refreshUsage")}
      aria-label={refreshLabel ?? t("usage.refreshUsage")}
    >
      <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
    </button>
  );

  const firstUsage = usage?.data?.[0];
  if (!usage || !usage.success || !firstUsage) {
    return (
      <div className="inline-flex items-center gap-2 text-xs rounded-lg border border-border-default bg-card px-3 py-2 shadow-sm">
        <div
          className="flex items-center gap-1.5 text-red-500 dark:text-red-400"
          title={usage?.error || undefined}
        >
          <AlertCircle size={12} />
          <span>{t("usage.queryFailed")}</span>
        </div>
        {refreshButton}
      </div>
    );
  }

  const isExpired = firstUsage.isValid === false;

  const usageValues = (
    <>
      {showUsed && firstUsage.used !== undefined && (
        <div className="flex items-center gap-0.5">
          <span className="text-gray-500 dark:text-gray-400">
            {t("usage.used")}
          </span>
          <span className="tabular-nums text-gray-600 dark:text-gray-400 font-medium">
            {firstUsage.used.toFixed(2)}
          </span>
        </div>
      )}

      {firstUsage.remaining !== undefined && (
        <div className="flex items-center gap-0.5">
          <span className="text-gray-500 dark:text-gray-400">
            {t("usage.remaining")}
          </span>
          <span
            className={`font-semibold tabular-nums ${
              isExpired
                ? "text-red-500 dark:text-red-400"
                : firstUsage.remaining <
                    (firstUsage.total || firstUsage.remaining) * 0.1
                  ? "text-orange-500 dark:text-orange-400"
                  : "text-green-600 dark:text-green-400"
            }`}
          >
            {firstUsage.remaining.toFixed(2)}
          </span>
        </div>
      )}

      {firstUsage.unit && (
        <span className="text-gray-500 dark:text-gray-400">
          {firstUsage.unit}
        </span>
      )}

      {firstUsage.extra && (
        <span
          className="text-gray-500 dark:text-gray-400 truncate max-w-[150px]"
          title={firstUsage.extra}
        >
          {firstUsage.extra}
        </span>
      )}
    </>
  );

  const queriedAt = (
    <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
      <Clock size={10} />
      {lastQueriedAt
        ? formatRelativeTime(lastQueriedAt, now, t)
        : t("usage.never", { defaultValue: "从未更新" })}
    </span>
  );

  if (singleLine) {
    return (
      <div className="flex flex-row items-center gap-2 text-xs whitespace-nowrap flex-shrink-0">
        {usageValues}
        {queriedAt}
        {refreshButton}
      </div>
    );
  }

  return (
    <div className="flex flex-col items-end gap-1 text-xs whitespace-nowrap flex-shrink-0">
      {/* 第一行：更新时间和刷新按钮 */}
      <div className="flex items-center gap-2 justify-end">
        {queriedAt}
        {refreshButton}
      </div>

      {/* 第二行：用量和剩余 */}
      <div className="flex items-center gap-2">{usageValues}</div>
    </div>
  );
};

const UsageFooter: React.FC<UsageFooterProps> = ({
  provider,
  providerId,
  appId,
  usageEnabled,
  isCurrent,
  isInConfig = false,
  inline = false,
}) => {
  const { t } = useTranslation();
  const isTokenPlan =
    provider.meta?.usage_script?.templateType === "token_plan";

  // 统一的用量查询（自动查询仅对当前激活的供应商启用）
  // OpenCode（累加模式）：使用 isInConfig 代替 isCurrent
  const shouldAutoQuery = appId === "opencode" ? isInConfig : isCurrent;
  const autoQueryInterval = shouldAutoQuery
    ? provider.meta?.usage_script?.autoQueryInterval || 0
    : 0;

  const {
    data: usage,
    isFetching: loading,
    isError,
    lastQueriedAt,
    refetch,
  } = useUsageQuery(providerId, appId, {
    enabled: usageEnabled,
    autoQueryInterval,
  });

  // 🆕 定期更新当前时间，用于刷新相对时间显示
  const [now, setNow] = React.useState(Date.now());

  React.useEffect(() => {
    if (!lastQueriedAt) return;

    // 每30秒更新一次当前时间，触发相对时间显示的刷新
    const interval = setInterval(() => {
      setNow(Date.now());
    }, 30000); // 30秒

    return () => clearInterval(interval);
  }, [lastQueriedAt]);

  // 只在启用用量查询且有数据时显示。后端把瞬时传输失败转成了 reject：有缓存
  // 成功值时 react-query 保留 data 照常展示；首次查询就失败则 data 为空——
  // 此时（isError）仍要渲染失败态给出重试入口，否则 footer 整体消失、无从重查。
  if (!usageEnabled || (!usage && !isError)) return null;

  // 错误状态（业务失败，或无缓存成功值的 reject）
  if (!usage || !usage.success) {
    if (inline) {
      return (
        <InlineUsage
          usage={usage}
          loading={loading}
          lastQueriedAt={lastQueriedAt}
          onRefresh={refetch}
        />
      );
    }

    return (
      <div className="mt-3 rounded-xl border border-border-default bg-card px-4 py-3 shadow-sm">
        <div className="flex items-center justify-between gap-2 text-xs">
          <div className="flex items-center gap-2 text-red-500 dark:text-red-400">
            <AlertCircle size={14} />
            <span>{usage?.error || t("usage.queryFailed")}</span>
          </div>

          {/* 刷新按钮 */}
          <button
            onClick={() => refetch()}
            disabled={loading}
            className="p-1 rounded hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors disabled:opacity-50 flex-shrink-0"
            title={t("usage.refreshUsage")}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
      </div>
    );
  }

  const usageDataList = usage.data || [];

  // 无数据时不显示
  if (usageDataList.length === 0) return null;

  // ── Token Plan：订阅风格内联渲染（百分比徽章 + 倒计时） ──
  if (isTokenPlan && inline) {
    return (
      <div className="flex flex-col items-end gap-1 text-xs whitespace-nowrap flex-shrink-0">
        {/* 第一行：查询时间 + 刷新 */}
        <div className="flex items-center gap-2 justify-end">
          <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
            <Clock size={10} />
            {lastQueriedAt
              ? formatRelativeTime(lastQueriedAt, now, t)
              : t("usage.never", { defaultValue: "从未更新" })}
          </span>
          <button
            onClick={(e) => {
              e.stopPropagation();
              refetch();
            }}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0 text-muted-foreground"
            title={t("usage.refreshUsage")}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
        {/* 第二行：tier 徽章（复用官方订阅的 TierBadge） */}
        <div className="flex items-center gap-2">
          {(() => {
            const tiers = usageDataList.map((d) => toQuotaTier(d));
            const planLabel = tiers[0]?.planLabel;
            return (
              <>
                {planLabel && (
                  <span className="font-semibold text-muted-foreground">
                    💰 {planLabel}
                  </span>
                )}
                {tiers.map((tier, index) => (
                  <TierBadge key={index} tier={tier} t={t} />
                ))}
              </>
            );
          })()}
        </div>
      </div>
    );
  }

  // ── 通用用量：内联模式 ──
  if (inline) {
    return (
      <InlineUsage
        usage={usage}
        loading={loading}
        lastQueriedAt={lastQueriedAt}
        onRefresh={refetch}
      />
    );
  }

  return (
    <div className="mt-3 rounded-xl border border-border-default bg-card px-4 py-3 shadow-sm">
      {/* 标题行：包含刷新按钮和自动查询时间 */}
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs text-gray-500 dark:text-gray-400 font-medium">
          {t("usage.planUsage")}
        </span>
        <div className="flex items-center gap-2">
          {/* 自动查询时间提示 */}
          {lastQueriedAt && (
            <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
              <Clock size={10} />
              {formatRelativeTime(lastQueriedAt, now, t)}
            </span>
          )}
          <button
            onClick={() => refetch()}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50"
            title={t("usage.refreshUsage")}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
      </div>

      {/* 套餐列表 */}
      <div className="flex flex-col gap-3">
        {usageDataList.map((usageData, index) => (
          <UsagePlanItem key={index} data={usageData} />
        ))}
      </div>
    </div>
  );
};

// ── 通用用量组件 ────────────────────────────────────────────

// 单个套餐数据展示组件
const UsagePlanItem: React.FC<{ data: UsageData }> = ({ data }) => {
  const { t } = useTranslation();
  const {
    planName,
    extra,
    isValid,
    invalidMessage,
    total,
    used,
    remaining,
    unit,
  } = data;

  // 判断套餐是否失效（isValid 为 false 或未定义时视为有效）
  const isExpired = isValid === false;

  return (
    <div className="flex items-center gap-3">
      {/* 标题部分：25% */}
      <div
        className="text-xs text-gray-500 dark:text-gray-400 min-w-0"
        style={{ width: "25%" }}
      >
        {planName ? (
          <span
            className={`font-medium truncate block ${isExpired ? "text-red-500 dark:text-red-400" : ""}`}
            title={planName}
          >
            💰 {planName}
          </span>
        ) : (
          <span className="opacity-50">—</span>
        )}
      </div>

      {/* 扩展字段：30% */}
      <div
        className="text-xs text-gray-500 dark:text-gray-400 min-w-0 flex items-center gap-2"
        style={{ width: "30%" }}
      >
        {extra && (
          <span
            className={`truncate ${isExpired ? "text-red-500 dark:text-red-400" : ""}`}
            title={extra}
          >
            {extra}
          </span>
        )}
        {isExpired && (
          <span className="text-red-500 dark:text-red-400 font-medium text-[10px] px-1.5 py-0.5 bg-red-50 dark:bg-red-900/20 rounded flex-shrink-0">
            {invalidMessage || t("usage.invalid")}
          </span>
        )}
      </div>

      {/* 用量信息：45% */}
      <div
        className="flex items-center justify-end gap-2 text-xs flex-shrink-0"
        style={{ width: "45%" }}
      >
        {/* 总额度 */}
        {total !== undefined && (
          <>
            <span className="text-gray-500 dark:text-gray-400">
              {t("usage.total")}
            </span>
            <span className="tabular-nums text-gray-600 dark:text-gray-400">
              {total === -1 ? "∞" : total.toFixed(2)}
            </span>
            <span className="text-gray-400 dark:text-gray-600">|</span>
          </>
        )}

        {/* 已用额度 */}
        {used !== undefined && (
          <>
            <span className="text-gray-500 dark:text-gray-400">
              {t("usage.used")}
            </span>
            <span className="tabular-nums text-gray-600 dark:text-gray-400">
              {used.toFixed(2)}
            </span>
            <span className="text-gray-400 dark:text-gray-600">|</span>
          </>
        )}

        {/* 剩余额度 - 突出显示 */}
        {remaining !== undefined && (
          <>
            <span className="text-gray-500 dark:text-gray-400">
              {t("usage.remaining")}
            </span>
            <span
              className={`font-semibold tabular-nums ${
                isExpired
                  ? "text-red-500 dark:text-red-400"
                  : remaining < (total || remaining) * 0.1
                    ? "text-orange-500 dark:text-orange-400"
                    : "text-green-600 dark:text-green-400"
              }`}
            >
              {remaining.toFixed(2)}
            </span>
          </>
        )}

        {unit && (
          <span className="text-gray-500 dark:text-gray-400">{unit}</span>
        )}
      </div>
    </div>
  );
};

// 格式化相对时间
function formatRelativeTime(
  timestamp: number,
  now: number,
  t: (key: string, options?: { count?: number }) => string,
): string {
  const diff = Math.floor((now - timestamp) / 1000); // 秒

  if (diff < 60) {
    return t("usage.justNow");
  } else if (diff < 3600) {
    const minutes = Math.floor(diff / 60);
    return t("usage.minutesAgo", { count: minutes });
  } else if (diff < 86400) {
    const hours = Math.floor(diff / 3600);
    return t("usage.hoursAgo", { count: hours });
  } else {
    const days = Math.floor(diff / 86400);
    return t("usage.daysAgo", { count: days });
  }
}

export default UsageFooter;
