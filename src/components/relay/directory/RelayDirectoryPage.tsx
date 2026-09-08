import { useMemo, useReducer, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  Loader2,
  Plus,
  RefreshCw,
  Search,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PLAZA_VISIBLE_DEFAULT, relayApi, settingsApi } from "@/lib/api";
import { crowdApi } from "@/lib/api/crowd";
import type { AppId } from "@/lib/api";
import type { RelayDirectoryItem, RelayImportError } from "@/lib/api/relay";
import { crowdKeys } from "@/lib/query/crowd";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";
import { useSettings } from "@/hooks/useSettings";
import { extractErrorMessage } from "@/utils/errorUtils";

import {
  DIRECTORY_PAGE_SIZE,
  filterDirectoryItems,
  pageDirectoryItems,
  reduceDirectoryView,
  visibleDirectoryRange,
} from "./directoryState";
import { RelayDirectoryRow } from "./RelayDirectoryRow";
import { CROWD_NOTICE_OPEN_EVENT } from "../CrowdNoticeDialog";
import { FirstVisitDomainDialog } from "./FirstVisitDomainDialog";
import { TransitDetailDialog } from "./TransitDetailDialog";

export interface RelayDirectoryPageProps {
  sourceAppId: AppId;
  onBack: () => void;
  onAuthenticated?: () => void;
  /**
   * 内嵌在统一添加聚合页（`AddHubPage`）里时不带自己的返回箭头与页面级容器 ——
   * 外层是聚合页的标签布局，返回由聚合页统一负责。
   */
  embedded?: boolean;
  /**
   * 新人首启落广场（`RelaySection.reloadStatus` 的新人分支）：同时弹一次
   * 「手填域名直达」弹窗（见 `FirstVisitDomainDialog`）。每进程最多一次，
   * 由调用方保证（`autoOpenedHubThisProcess`）。
   */
  firstVisit?: boolean;
}

export function RelayDirectoryPage({
  sourceAppId,
  onBack,
  onAuthenticated,
  embedded = false,
  firstVisit = false,
}: RelayDirectoryPageProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [view, dispatch] = useReducer(reduceDirectoryView, {
    search: "",
    page: 1,
  });
  const [authenticatingHost, setAuthenticatingHost] = useState<string | null>(
    null,
  );
  const authenticationInProgress = useRef(false);
  // 新人首启的域名直达弹窗：只跟首挂载那一次走（firstVisit 由调用方保证
  // 每进程至多一次），用户关掉就纯逛广场。
  const [firstVisitOpen, setFirstVisitOpen] = useState(firstVisit);
  // 站方公开数据详情：持有整个 item（弹窗要站名 + transit 投影），非空即开。
  const [transitDetail, setTransitDetail] = useState<RelayDirectoryItem | null>(
    null,
  );
  const { settings: appSettings } = useSettings();
  // 兜底与后端默认值同向（2026-09-07 起默认开）：字段实际总在，只有极端的
  // 缺键情形才会落到兜底。
  const crowdEnabled = appSettings?.crowdMetricsEnabled ?? true;
  // 广场开关只藏推荐列表本身：关掉时列表为空（不拉清单）、页头同步时间与
  // 刷新按钮退场，但搜索框与「搜不到就地直连」保留 —— 关列表不等于关广场。
  const plazaVisible = appSettings?.plazaVisible ?? PLAZA_VISIBLE_DEFAULT;
  // 详情弹窗的实测深数据（w7/时段/分布）：共建门禁内 —— 行级观测徽章公开，
  // 由列表 DTO 的 item.crowd 承载，不走这份门禁内快照。
  const crowdSnapshotQuery = useQuery({
    queryKey: crowdKeys.snapshot,
    queryFn: () => crowdApi.getSnapshot(),
    // 门禁第一道：不参与就不发查询（后端命令是第二道，null 也是合法返回）。
    enabled: crowdEnabled,
    staleTime: 5 * 60 * 1000,
    gcTime: 10 * 60 * 1000,
    retry: 1,
  });
  const crowdSnapshot = crowdSnapshotQuery.data ?? null;

  const directoryQuery = useQuery({
    queryKey: relayDirectoryKeys.listing(),
    queryFn: () => relayApi.listDirectory(),
    // 列表被广场开关藏起来时连清单都不发请求；即便命中旧缓存，下方的
    // filtered 守卫也保证一行都不渲染。
    enabled: plazaVisible,
    staleTime: Infinity,
    gcTime: Infinity,
  });

  const refreshMutation = useMutation({
    mutationFn: () => relayApi.refreshDirectory(),
    onSuccess: (result) => {
      queryClient.setQueryData(relayDirectoryKeys.listing(), result);
    },
    onError: (reason) => {
      toast.error(
        t("loongport.directory.refreshFailed", {
          reason: extractErrorMessage(reason),
        }),
      );
    },
  });

  const listing = directoryQuery.data ?? null;
  const filtered = useMemo(
    () =>
      filterDirectoryItems(
        plazaVisible ? (listing?.items ?? []) : [],
        view.search,
      ),
    [plazaVisible, listing?.items, view.search],
  );
  const paged = pageDirectoryItems(filtered, view.page);
  const range = visibleDirectoryRange(
    paged.page,
    DIRECTORY_PAGE_SIZE,
    filtered.length,
  );

  const importErrorMessage = (reason: unknown): string | null => {
    const typed =
      typeof reason === "object" && reason !== null
        ? (reason as Partial<RelayImportError>)
        : undefined;
    if (typed?.kind === "cancelled") return null;
    // 两种 kind 的指引不能共用：unsupported_site 是「协议没识别出来，网页验证
    // 可能有用」；not_in_directory 是「目录里没有这个站」，正确出路是本页底部
    // 的手动输入框（Manual 来源不走签名目录校验）。
    if (typed?.kind === "unsupported_site") {
      return t("loongport.addSite.unsupportedSite");
    }
    if (typed?.kind === "not_in_directory") {
      return t("loongport.addSite.notInDirectory");
    }
    if (typed?.kind === "protocol_conflict") {
      return t("loongport.addSite.protocolConflict");
    }
    if (typed?.kind === "transport") {
      return t("loongport.addSite.transportError");
    }
    const message = extractErrorMessage(reason).replace(
      /^(?:配置错误:\s*)+/,
      "",
    );
    return message || t("loongport.addSite.importFailed");
  };

  const authenticate = async (
    entryUrl: string,
    host: string,
    source: "directory" | "manual" = "directory",
  ) => {
    if (authenticationInProgress.current) return;
    authenticationInProgress.current = true;
    setAuthenticatingHost(host);
    try {
      // manual = 用户手填域名（不在白名单里）：走保守的 Manual 打开规则。
      const result = await (source === "directory"
        ? relayApi.importDirectorySite(entryUrl)
        : relayApi.importSite(entryUrl));
      toast.success(
        t("loongport.addSite.connected", { name: result.siteName }),
      );
      try {
        await relayApi.refresh(result.relayId, sourceAppId);
      } catch (reason) {
        toast.error(
          t("loongport.directory.provisionFailed", {
            reason: extractErrorMessage(reason),
          }),
        );
      }
      onAuthenticated?.();
      onBack();
    } catch (reason) {
      const message = importErrorMessage(reason);
      if (message) toast.error(message);
    } finally {
      authenticationInProgress.current = false;
      setAuthenticatingHost(null);
    }
  };

  const syncedAt =
    listing && listing.syncedAt > 0
      ? new Intl.DateTimeFormat(i18n.resolvedLanguage || undefined, {
          dateStyle: "medium",
          timeStyle: "short",
        }).format(new Date(listing.syncedAt * 1000))
      : "";

  return (
    <div
      className={
        embedded
          ? "flex w-full flex-col"
          : "mx-auto flex h-full w-full max-w-[1180px] flex-col px-6 pb-6"
      }
    >
      <FirstVisitDomainDialog
        open={firstVisitOpen}
        onDismiss={() => setFirstVisitOpen(false)}
        onSubmit={(domain) => {
          setFirstVisitOpen(false);
          // 归因播种广场开关默认值：与导入成败无关（导入失败也来自那家站），
          // fire-and-forget —— 后端写-if-None，失败只保持默认展示。
          void settingsApi
            .plazaSeedFromFirstSite(domain)
            .catch(() => undefined);
          // 与广场搜索框手填完全同一条 manual 导入链（保守打开规则）。
          void authenticate(domain, domain, "manual");
        }}
      />
      {transitDetail && (
        <TransitDetailDialog
          item={transitDetail}
          open
          onDismiss={() => setTransitDetail(null)}
          crowdStats={
            transitDetail
              ? (crowdSnapshot?.sites[transitDetail.siteDomain] ?? null)
              : null
          }
          crowdEnabled={crowdEnabled}
          onOpenCrowdNotice={() =>
            // 弹窗单实例挂在 App 层（主动告知 + 广场再入口共用），这里只广播。
            window.dispatchEvent(new Event(CROWD_NOTICE_OPEN_EVENT))
          }
        />
      )}
      <div className="flex items-start justify-between gap-4 border-b border-border-default py-4">
        <div className="flex min-w-0 items-start gap-3">
          {!embedded && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="mt-0.5 h-8 w-8 shrink-0"
              onClick={onBack}
              aria-label={t("common.back")}
            >
              <ArrowLeft className="h-4 w-4" />
            </Button>
          )}
          <div className="min-w-0">
            <h1 className="text-lg font-semibold tracking-tight">
              {t("loongport.directory.title")}
            </h1>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {t("loongport.directory.description")}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
          {plazaVisible && (
            <>
              {syncedAt && (
                <span>
                  {t("loongport.directory.source.syncedAt", { time: syncedAt })}
                </span>
              )}
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                disabled={directoryQuery.isPending || refreshMutation.isPending}
                onClick={() => refreshMutation.mutate()}
                aria-label={t("loongport.directory.actions.refresh")}
              >
                <RefreshCw
                  className={
                    directoryQuery.isFetching || refreshMutation.isPending
                      ? "h-4 w-4 animate-spin"
                      : "h-4 w-4"
                  }
                />
              </Button>
            </>
          )}
        </div>
      </div>

      <div className="flex items-center justify-end gap-4 py-4">
        <div className="relative w-full max-w-xs">
          <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
          <Input
            value={view.search}
            onChange={(event) =>
              dispatch({ type: "search", search: event.target.value })
            }
            onKeyDown={(event) => {
              // 搜不到时回车 = 把搜索词作为站点直连导入（与虚框按钮同一动作）。
              if (
                event.key === "Enter" &&
                paged.items.length === 0 &&
                view.search.trim() &&
                !authenticationInProgress.current
              ) {
                const site = view.search.trim();
                void authenticate(site, site, "manual");
              }
            }}
            placeholder={t("loongport.directory.searchPlaceholder")}
            className="pl-9"
          />
        </div>
      </div>

      <section className="min-h-0 flex-1 overflow-hidden rounded-lg border border-border-default bg-background shadow-sm">
        {plazaVisible && directoryQuery.isPending && !listing ? (
          <div className="flex h-48 items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("loongport.directory.loading")}
          </div>
        ) : directoryQuery.isError && !listing ? (
          <div className="p-4">
            <Alert variant="destructive">
              <AlertTitle>{t("loongport.directory.errorTitle")}</AlertTitle>
              <AlertDescription className="mt-2 flex items-center justify-between gap-4">
                <span>
                  {extractErrorMessage(directoryQuery.error) ||
                    t("loongport.directory.errorBody")}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={directoryQuery.isFetching}
                  onClick={() => void directoryQuery.refetch()}
                >
                  {t("loongport.directory.actions.retry")}
                </Button>
              </AlertDescription>
            </Alert>
          </div>
        ) : paged.items.length === 0 && view.search.trim() ? (
          // 搜不到就地转添加：白名单没有的站，搜索词就是要连的地址 —— 整块
          // 虚框是碰撞框（与区块空态同一个交互语言），回车同效。
          <button
            type="button"
            disabled={authenticatingHost !== null}
            onClick={() => {
              const site = view.search.trim();
              void authenticate(site, site, "manual");
            }}
            className="m-4 flex w-[calc(100%-2rem)] flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border p-6 text-sm text-muted-foreground transition-colors hover:border-blue-400/60 hover:bg-muted/40 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-60"
          >
            <span>
              {t("loongport.directory.noMatch", {
                query: view.search.trim(),
              })}
            </span>
            <span className="inline-flex items-center gap-1.5 font-medium text-blue-600 dark:text-blue-400">
              {authenticatingHost === view.search.trim() ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Plus className="h-3.5 w-3.5" />
              )}
              {t("loongport.directory.addAsSite", {
                query: view.search.trim(),
              })}
            </span>
          </button>
        ) : paged.items.length === 0 ? (
          <div className="flex h-48 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
            <span>{t("loongport.directory.empty")}</span>
          </div>
        ) : (
          <div className="h-full overflow-auto">
            {paged.items.map((item: RelayDirectoryItem) => (
              <RelayDirectoryRow
                key={item.siteHost}
                item={item}
                busy={authenticatingHost === item.siteHost}
                disabled={authenticatingHost !== null}
                onAuthenticate={(selected) => {
                  void authenticate(selected.entryUrl, selected.siteHost);
                }}
                onOpenTransit={setTransitDetail}
              />
            ))}
          </div>
        )}
      </section>

      <div className="flex items-center justify-between gap-4 py-3 text-xs text-muted-foreground">
        <span>{t("loongport.directory.compatibilityNote")}</span>
        <div className="flex items-center gap-2">
          <span>
            {t("loongport.directory.pagination.range", {
              from: range.from,
              to: range.to,
              total: filtered.length,
            })}
          </span>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="h-7 w-7"
            disabled={paged.page <= 1}
            onClick={() => dispatch({ type: "page", page: paged.page - 1 })}
            aria-label={t("loongport.directory.pagination.previous")}
          >
            <ChevronLeft className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="h-7 w-7"
            disabled={paged.page >= paged.totalPages}
            onClick={() => dispatch({ type: "page", page: paged.page + 1 })}
            aria-label={t("loongport.directory.pagination.next")}
          >
            <ChevronRight className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
    </div>
  );
}
