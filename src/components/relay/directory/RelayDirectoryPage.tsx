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
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { relayApi } from "@/lib/api";
import { crowdApi } from "@/lib/api/crowd";
import type { AppId } from "@/lib/api";
import type {
  LeaderboardKind,
  RelayDirectoryItem,
  RelayImportError,
} from "@/lib/api/relay";
import { crowdKeys } from "@/lib/query/crowd";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";
import { useSettings } from "@/hooks/useSettings";
import { extractErrorMessage } from "@/utils/errorUtils";

import {
  DIRECTORY_PAGE_SIZE,
  defaultDirectoryKind,
  filterDirectoryItems,
  pageDirectoryItems,
  reduceDirectoryView,
  visibleDirectoryRange,
} from "./directoryState";
import { RelayDirectoryRow } from "./RelayDirectoryRow";
import { CrowdNoticeDialog } from "../CrowdNoticeDialog";
import { FirstVisitDomainDialog } from "./FirstVisitDomainDialog";
import { TransitDetailDialog } from "./TransitDetailDialog";

const KINDS: LeaderboardKind[] = ["overall", "claude", "openai", "gemini"];

export interface RelayDirectoryPageProps {
  sourceAppId: AppId;
  initialKind?: LeaderboardKind;
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
  initialKind,
  onBack,
  onAuthenticated,
  embedded = false,
  firstVisit = false,
}: RelayDirectoryPageProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [view, dispatch] = useReducer(reduceDirectoryView, {
    kind: initialKind ?? defaultDirectoryKind(sourceAppId),
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
  // 共建告知弹窗（从实测区的加入入口唤起）。
  const [crowdNoticeOpen, setCrowdNoticeOpen] = useState(false);

  const { settings: appSettings } = useSettings();
  const crowdEnabled = appSettings?.crowdMetricsEnabled ?? false;
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
    queryKey: relayDirectoryKeys.byKind(view.kind),
    queryFn: () => relayApi.listDirectory(view.kind),
    staleTime: Infinity,
    gcTime: Infinity,
  });

  const refreshMutation = useMutation({
    mutationFn: () => relayApi.refreshDirectory(view.kind),
    onSuccess: (result) => {
      queryClient.setQueryData(relayDirectoryKeys.byKind(result.kind), result);
    },
    onError: (reason) => {
      toast.error(
        t("loongport.directory.refreshFailed", {
          reason: extractErrorMessage(reason),
        }),
      );
    },
  });

  const visibleLeaderboard = directoryQuery.data ?? null;
  const filtered = useMemo(
    () => filterDirectoryItems(visibleLeaderboard?.items ?? [], view.search),
    [visibleLeaderboard?.items, view.search],
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

  const syncedAt = visibleLeaderboard
    ? new Intl.DateTimeFormat(i18n.resolvedLanguage || undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(new Date(visibleLeaderboard.syncedAt * 1000))
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
              ? (crowdSnapshot?.sites[transitDetail.siteHost] ?? null)
              : null
          }
          crowdEnabled={crowdEnabled}
          onOpenCrowdNotice={() => setCrowdNoticeOpen(true)}
        />
      )}
      {/* 共建告知：从实测区的「加入共建」入口唤起，表态后设置与快照缓存
          都会被弹窗自己失效 —— 这里只负责开关。 */}
      <CrowdNoticeDialog
        open={crowdNoticeOpen}
        onDone={() => setCrowdNoticeOpen(false)}
      />
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
          {visibleLeaderboard && (
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
        </div>
      </div>

      <div className="flex items-center justify-between gap-4 py-4">
        <Tabs
          value={view.kind}
          onValueChange={(kind) =>
            dispatch({ type: "kind", kind: kind as LeaderboardKind })
          }
        >
          <TabsList className="h-9">
            {KINDS.map((kind) => (
              <TabsTrigger
                key={kind}
                value={kind}
                className="min-w-[88px] py-1"
              >
                {t(`loongport.directory.tabs.${kind}`)}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>

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
        {directoryQuery.isPending && !visibleLeaderboard ? (
          <div className="flex h-48 items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("loongport.directory.loading")}
          </div>
        ) : directoryQuery.isError && !visibleLeaderboard ? (
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
                key={`${item.siteHost}:${item.veridropHost}`}
                item={item}
                busy={authenticatingHost === item.siteHost}
                disabled={authenticatingHost !== null}
                onAuthenticate={(selected) => {
                  void authenticate(selected.entryUrl, selected.siteHost);
                }}
                onOpenTransit={setTransitDetail}
                measuredP50Ms={
                  crowdSnapshot?.sites[item.siteHost]?.w24?.ttftP50Ms ?? null
                }
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
