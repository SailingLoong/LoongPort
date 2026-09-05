import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Layers3, Loader2, RefreshCw } from "lucide-react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  relayApi,
  PURCHASE_CLOSED,
  VENDOR_LOGIN_ERROR,
  VENDOR_ACCOUNTS_CHANGED,
  PROVIDER_SWITCHED,
} from "@/lib/api";
import type { AppId, ProviderSwitchEvent } from "@/lib/api";
import type {
  RefreshResult,
  RelayRow as RelayRowData,
  RelayUsageBlocker,
  TierInfo,
} from "@/lib/api/relay";
import { ONBOARDING_REGISTER_COMPLETED } from "@/lib/api/events";
import { type OnboardingRegisterCompleted } from "@/lib/onboarding";
import {
  vendorApi,
  type VendorAccountRow,
  type VendorPlanInfo,
} from "@/lib/api/vendor";
import { getAppLabel } from "@/config/appConfig";
import { useStreamCheck } from "@/hooks/useStreamCheck";
import { useTauriEvent } from "@/hooks/useTauriEvent";

import { ImageTabNotice } from "./ImageTabNotice";
import { RelayTierList } from "./RelayTierList";
import { SwitchTierConfirmDialog } from "./SwitchTierConfirmDialog";
import { TierVerificationProvider } from "./model-verification/TierVerificationProvider";
import { rowBalanceKeys } from "./useRowBalanceQuery";
import { openInBrowser } from "./openInBrowser";
import { useRowBusy } from "./useRowBusy";
import { useTierEditGuard } from "./useTierEditGuard";
import { VendorBlock } from "./VendorBlock";
import { vendorBusyKey, vendorPlanBusyKey } from "./VendorRow";

/**
 * 「中转站 × 分组」区，**自带全部状态**，供 provider 页顶部直接挂载。
 *
 * ## 为什么是自带状态的容器
 *
 * 它要挂进 `App.tsx`（上游文件）。如果把 `relays` / `busy` / 那几个行内 handler
 * 摊在那里，等于把 relay 的实现搬进上游文件 —— 将来 merge 上游时那一片全是冲突。
 * 收在这个组件里，`App.tsx` 只需一行 `<RelaySection appId={activeApp} />`
 * （CLAUDE.md §一「改上游文件时改动面越小越好」）。
 *
 * ## 它是中转站链路唯一的宿主（2026-08-04）
 *
 * 原来还有个 LoongPort 独立管理页（`OperatorPanel`）并存，它管站点切换器、登出等
 * 第二套状态。那个页面已删；现在只有“中转站广场”承担发现和首次认证，认证后的
 * 登录、获取密钥与切档位仍由本组件统一持有，不再有平行的账号管理实现。
 *
 * 删它的理由：站点切换器与「登出」都是那个页面自造的概念（凭据按
 * `site_origin × account_id` 去重，换账号直接登录就是新增一行，不需要先登出），
 * 而剩下的能力这一区本来就有。唯一不可替代的「切回官方登录」搬去了设置页 auth tab。
 *
 * ## 中转站之间没有依赖（2026-08-03 修）
 *
 * 两个行内命令（login / provision）**全部显式带 relayId**，
 * 不再靠改全局「当前站」来定位。所以：
 *
 * - 禁用只作用于自己那一行（`useRowBusy`），别的行照常可点
 * - 多个中转站可以真并发获取密钥，互不干扰
 * - 不再需要 `focusRelay`（那个函数已删）—— 它靠 `set_current` 副作用定位，
 *   既会串目标，又会因为排序而让行序跳动
 */
export interface RelaySectionProps {
  /**
   * 当前 tab 的 app_type。决定拉哪个 app 下的档位、切换写哪个 app 的配置。
   *
   * **类型是 `AppId` 而不是 `string`**：连通检测的 `useStreamCheck(appId)` 要求 `AppId`，
   * 而唯一的调用点（`App.tsx`）传的 `activeApp` 本来就是 `AppId` —— 收窄它零风险，
   * 且把「这是个受限取值域」这个事实写进类型里。
   */
  appId: AppId;
  /** 打开统一添加聚合页的指定标签。首启引导落「中转站」（综合榜）；
   * 两个区块的空态占位也经它指名跳转（中转站→directory / 官方 API→official）。 */
  onOpenAddHub: (
    tab: "directory" | "official",
    opts?: { firstVisit?: boolean },
  ) => void;
}

/**
 * 「这个进程里已经自动打开过广场了」——**模块级变量，有意不是 state / ref**。
 *
 * 需求是「以进程被打开为计算，进程不消亡就不重复跳」。而这个组件会**反复挂载卸载**：
 * `App.tsx` 只在 provider 视图下渲染它，用户切到设置页再切回来、或切换 app tab
 * （`appId` 变化本身不重挂，但视图切换会）都会走一次新的挂载。
 *
 * 所以标志不能放组件内：`useState` / `useRef` 随卸载一起丢 ⇒ 用户每次回到这一页
 * 都被弹一次。放模块作用域，生命周期正好等于「这个 JS 上下文」= 这个进程
 * （app 重启 / 更新后 WebView 重新加载模块，标志自然回到 false —— 那正是要的）。
 *
 * ⚠️ 用 `localStorage` 是**错的**：那会跨进程持久化 ⇒ 用户第一次关掉引导之后，
 * 以后每次启动都不再提醒，即使他一个站都还没配。
 */
let autoOpenedHubThisProcess = false;

export function RelaySection({ appId, onOpenAddHub }: RelaySectionProps) {
  /**
   * 当前这一屏是不是生图页。
   *
   * 生图页与其它页的差异只有两点（都在 UI 层）：顶部多一段说明、空态不引导「添加站点」。
   * 数据链路完全一样 —— 档位、切换、连通检测、恢复默认全部走同一套命令，只是
   * `appId` 是 `"codex-image"`（后端据它查 `providers` 表那一栏）。
   *
   * 这正是分栏这个做法的好处：**没有一条平行实现**。上一版为生图另做了一套按钮 +
   * 一对命令 + 一个 settings 键，那些现在全删了。
   */
  const isImageTab = appId === "codex-image";
  const [relays, setRelays] = useState<RelayRowData[]>([]);
  const [confirmSwitch, setConfirmSwitch] = useState<{
    name: string;
    run: (quitChatgpt: boolean) => void;
  } | null>(null);
  // 余额**不在这里** —— 它由每一行自己的 `useRowBalanceQuery`（react-query）拉，
  // 见 `RowBalance`。曾经这里有两份 `Record<RowKey, …>` state 加两个 effect，
  // 而那个形状有个死路：effect 的依赖键是 `id:accountLabel`，某一行拉失败过一次、
  // 键不变 ⇒ 它永远不会再跑 ⇒ 那一行整个会话都没有余额；而充值按钮只在有余额时
  // 渲染 ⇒ 用户连重试的入口都看不到。换成 react-query 后「重查」就是行上那个
  // 刷新按钮。
  //
  // 这里保留 `relaysRef`：它还有别的消费者（下面几处要在异步返回时读最新的行）。
  const relaysRef = useRef<RelayRowData[]>([]);
  relaysRef.current = relays;

  // 验真 summaries 的拉取范围：本 tab 档位的 provider 去重集。拉取、弹窗与
  // 结果变化订阅由模型验证模块的 Provider 自持，这里只提供稳定的 id 集。
  const verificationProviderIds = useMemo(
    () => [
      ...new Set(
        relays.flatMap((row) =>
          row.tiers
            .filter((tier) => tier.appId === appId)
            .map((tier) => tier.providerId),
        ),
      ),
    ],
    [relays, appId],
  );

  // ── 官网直连账号（vendor）──────────────────────────────────────────
  //
  // **与 relay 平级并列的一份状态**，不合进上面那些：两边的命令、DTO 与余额
  // 类型全不同（余额那边是后端格式化好的字符串），合起来只会让每处多一个分支。
  const [vendors, setVendors] = useState<VendorAccountRow[]>([]);
  const [vendorSupported, setVendorSupported] = useState(false);
  const [confirmRemoveVendor, setConfirmRemoveVendor] =
    useState<VendorAccountRow | null>(null);
  // 异步编辑、恢复默认与切换动作返回时，需要按 id 读取最新的行数据。
  const vendorsRef = useRef<VendorAccountRow[]>([]);
  vendorsRef.current = vendors;
  // reload 的请求序号 —— 只让最后一次的结果落地，见 `reload` 里的说明。
  const reloadSeqRef = useRef(0);
  const vendorReloadSeqRef = useRef(0);
  const { t, i18n } = useTranslation();
  // 余额由各行自己的 query 持有；这里只在「充值窗关了」「刷新」时让它们失效。
  const queryClient = useQueryClient();
  const { busy, run } = useRowBusy();
  // 待确认「恢复默认配置」的目标。
  //
  // **两类行共用一个确认框**（文案、按钮、语义完全相同 —— 都是「用默认配置覆盖你的
  // 编辑，密钥保留」），只有真正执行时走的命令不同。分成两个 state + 两个
  // `<ConfirmDialog>` 会让同一句文案有两处副本，改一处漏一处。
  //
  // `kind` 决定调 `relayApi.resetTierConfig` 还是 `vendorApi.resetTierConfig`；
  // `busyKey` 由调用方给（两类行的 busy key 规则不同，见 `vendorBusyKey`）。
  const [confirmReset, setConfirmReset] = useState<{
    kind: "tier" | "vendor";
    providerId: string;
    displayName: string;
    busyKey: string;
  } | null>(null);
  // 待确认删除的中转站行。存整行：确认框里要显示它的名字与档位数。
  const [confirmRemove, setConfirmRemove] = useState<RelayRowData | null>(null);
  // 连通检测整套复用上游的 hook —— 它自带 toast、i18n 与 per-id 的 checking 状态。
  const { checkProvider, isChecking } = useStreamCheck(appId);

  /**
   * 拉官网账号列表。**只读本地不发网络**（与 `listRelays` 同一条契约）。
   *
   * `appId` 传给后端用于计算当前平台的支持状态、配置状态与当前项；前端直接消费
   * `supported/accounts`，不复制厂商支持列表。
   */
  const reloadVendors = useCallback(async () => {
    const seq = ++vendorReloadSeqRef.current;
    const isStale = () => seq !== vendorReloadSeqRef.current;

    try {
      const result = await vendorApi.list(appId);
      if (isStale()) return;
      setVendorSupported(result.supported);
      setVendors(result.accounts);
    } catch (e) {
      if (isStale()) return;
      toast.error(String(e));
    }
  }, [appId]);

  const reloadStatus = useCallback(async () => {
    try {
      const status = await relayApi.status();
      if (
        isImageTab ||
        !status.shouldPromptAddSite ||
        autoOpenedHubThisProcess
      ) {
        return;
      }
      autoOpenedHubThisProcess = true;
      // 新人（还没有任何站点账号）落到中转站广场挑站点 —— 落点 + 一次
      // 「手填域名直达」弹窗（广场列表动态加载，站长给的域名先走）。
      // 「点 Star 领注册礼」推迟到首个站点接入成功之后由后端直接发事件
      // （见 `commands::onboarding`），用户有使用感觉再邀请。
      onOpenAddHub("directory", { firstVisit: true });
    } catch {
      // 状态读不到时不猜业务事实；保留最后一次完整后端视图。
    }
  }, [isImageTab, onOpenAddHub]);

  const presentRefreshResult = useCallback(
    (result: RefreshResult) => {
      for (const balance of result.balances) {
        queryClient.setQueryData(
          rowBalanceKeys.row(balance.kind, balance.rowId),
          balance.result,
        );
      }

      const summary = result.summary;
      if (summary.notice === "otherPlatforms") {
        toast.info(
          t("loongport.provision.landedOnOtherPlatforms", {
            count: summary.otherPlatformTiers,
          }),
        );
      } else if (summary.notice === "updatedWithKeys") {
        toast.success(
          t("loongport.provision.refreshedWithKeys", {
            relays: summary.refreshedAccounts,
            tiers: summary.tiers,
            keys: summary.keysCreated,
          }),
        );
      } else if (summary.notice === "updated") {
        toast.success(
          t("loongport.provision.refreshed", {
            relays: summary.refreshedAccounts,
            tiers: summary.tiers,
          }),
        );
      }

      if (summary.mergedProviders > 0) {
        toast.info(
          t("loongport.provision.mergedProviders", {
            count: summary.mergedProviders,
          }),
        );
      }
      for (const failure of summary.failures) {
        const message = t("loongport.provision.refreshFailedWithReason", {
          name: failure.name,
          reason: failure.reason,
        });
        if (failure.kind === "key_limit" && failure.helpUrl) {
          toast.error(message, {
            action: {
              label: t("loongport.vendor.openKeyPage"),
              onClick: () => openInBrowser(failure.helpUrl!),
            },
          });
        } else {
          toast.error(message);
        }
      }
    },
    [queryClient, t],
  );

  /**
   * 读本地档位列表。**不发 provision**（不重拉分组），也**不发任何网络请求**。
   *
   * ## 为什么这里一个请求都不该有（2026-08-13 改）
   *
   * 这个函数挂在**每一个动作**后面（登录 / 获取密钥 / 切档位 / 保存编辑 / 托盘切
   * provider）。它原来还会异步调 `listTierRates` 去补倍率，而那条命令**每个档位一次
   * HTTP** ⇒ 用户切一次档位就把所有档位的倍率重查一遍。
   *
   * 倍率是服务端定价，不是实时量。现在它在 provision 时就算好落库、由 `listRelays`
   * 一并返回 ⇒ 刷新倍率 = 重新拉分组（页面级刷新 / 行级统一刷新 / 登录成功），
   * 正好是用户主动表达「把这页弄成最新」的那几下。
   *
   * 真正需要实时的是**余额**，那条路独立：它由每一行自己的 `useRowBalanceQuery`
   * 拉（react-query），不经过这个函数。
   *
   * ⚠️ **顺带刷新官网账号列表**（见函数体末尾）：两类行的「当前在用」现在都由后端
   * 现算（`tier.isCurrent` 与 vendor 的 `isCurrent` 同源），一次动作后必须把两类行
   * 一起刷齐，否则切档位后 DeepSeek 行会停在旧高亮上（2026-08-07 修的互斥 bug）。
   */
  const reload = useCallback(async () => {
    // ⚠️ **请求序号：只让最后一次 reload 的结果落地。**
    //
    // 这一区在每个动作后都 reload，而它们会重叠 —— 典型的一串是
    // 「保存编辑 → reload B」撞上更早开始的「获取密钥 → reload A」。
    // 没有守卫的话 A 后返回就用**旧行**覆盖 B 的新行 ⇒ 用户刚保存的编辑在界面上
    // 「没生效」（`userEdited` 标记闪一下又消失），而库里其实是对的。（review 抓出）
    const seq = ++reloadSeqRef.current;
    const isStale = () => seq !== reloadSeqRef.current;

    try {
      const rows = await relayApi.listRelays(appId);
      if (isStale()) return;
      setRelays(rows);
    } catch (e) {
      toast.error(String(e));
    }
    // ⚠️ **官网行必须跟档位一起刷**（见上方 doc）：两类行的「当前在用」同源，
    // 只刷一边就会让切完档位后 DeepSeek 行继续显示旧的「在用」高亮。
    void reloadVendors();
    void reloadStatus();
  }, [appId, reloadStatus, reloadVendors]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 「编辑配置」的事前警告 + 编辑页 + 保存后刷新（见 useTierEditGuard）。
  // 保存后必须 reload：「已手动维护」标记由后端按当前配置现算，不刷新拿不到新值。
  const { requestEdit, editDialogs } = useTierEditGuard(appId, reload);

  /**
   * 充值窗关掉了 → 刷那一行的余额（充完钱余额该涨）。
   *
   * **有意不做支付成功感知**（维护者裁决）：不认订单状态、不轮询、不判 tab。
   * 关窗刷一次就够 —— 用户没充值的话数字不变，也没有副作用。
   *
   * payload 带 relayId，所以只刷那一行，不整页 reload（那会连带重查所有倍率）。
   */
  useTauriEvent<number | null>(PURCHASE_CLOSED, (relayId) => {
    if (typeof relayId !== "number") return;
    // 让那一行的余额 query 失效，react-query 自己去重拉。
    // ⚠️ 行已经不在了（用户删掉了它）也无所谓 —— 没有组件订阅那个 key，
    // invalidate 是个空操作，不像原来那样会白打一次必定报错的请求。
    void queryClient.invalidateQueries({
      queryKey: rowBalanceKeys.row("relay", relayId),
    });
  });

  /**
   * 供应商切换后重新拉行 —— 「当前在用」那个高亮靠它更新。
   *
   * ## 为什么必须监听（2026-08-04 加，`OperatorPanel` 删除时接过来的）
   *
   * 本组件自己那个「使用」按钮走 `handleUse`，切完会 `reload()`。但**还有三条
   * 路径绕过前端**（都在 Rust 侧直接调 `ProviderService::switch`）：
   * 托盘快切、deeplink 导入、项目快照。
   *
   * 那三条路径过后，界面上的「当前在用」还指着旧的那一个 —— 用户从托盘切完
   * 回到这一页，看到的是错的状态。原来这个监听在 `OperatorPanel` 里，
   * 那个页面删掉之后就没人接了。
   *
   * ## 只在事件属于本 tab 那个 app 时才刷
   *
   * 不过滤会让「切 claude 的供应商」也触发 codex 这一区重新拉一遍 ——
   * 多余的网络请求（每个档位一次倍率查询）。
   */
  useTauriEvent<ProviderSwitchEvent>(PROVIDER_SWITCHED, (payload) => {
    if (payload?.appType !== appId) return;
    void reload();
  });

  /**
   * 启动时探一次凭据是不是真的还活着。
   *
   * 首屏行状态只看本地凭据。凭据在网页端被撤销、账号被禁用、
   * 会话被踢掉时它仍是 true ⇒ 用户看到界面一切正常，点任何操作才报错。
   * 这一次探活把那种状态提前暴露出来（后端探到失效会清掉本地凭据）。
   *
   * 有意不 await 进 `reload`：首屏该立刻渲染，不该卡在网络请求上。
   *
   * ⚠️ 2026-08-04 从已删的 LoongPort 独立页接过来 —— 那个页面删掉之后
   * 这个探活一度没人调，于是「凭据在服务端已失效」这件事又只能靠用户撞错误发现。
   */
  useEffect(() => {
    let cancelled = false;
    relayApi
      .checkSession()
      .then((expiredIds) => {
        // 返回的是**这次被清掉凭据的行 id**，空数组 = 全都还好。
        if (expiredIds.length === 0 || cancelled) return;
        // 一条 toast 说清有几个账号需要重新登录 —— 逐行弹会在多行同时过期时
        // 糊满屏幕，而具体是哪几行界面上已经各自标出来了（`sessionExpired` 分支）。
        toast.info(
          t("loongport.session.expired", { count: expiredIds.length }),
        );
        void reload();
      })
      // 探活自身失败（网络不通）不打扰用户 —— 凭据没被清掉，操作时会自然报错。
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [reload, t]);

  // ══ 官网直连账号（vendor）══════════════════════════════════════════

  /**
   * 登录窗的凭据回传解析失败了。
   *
   * **必须报出来**：这条路径上用户看到的现象是「走完登录流程，界面什么都没发生」——
   * 不说的话他会反复重登。事件名是 vendor 自己那条（与 relay 有意不同）。
   */
  useTauriEvent<string>(VENDOR_LOGIN_ERROR, (message) => {
    toast.error(t("loongport.vendor.loginFailed", { reason: message }));
  });

  /**
   * 官网账号行集合变化（App 级「官方 API」页里登录成功后由后端广播）。
   * 这页够不到这里的本地状态，与 relay 侧靠 provider-switched 刷新同一机制。
   */
  useTauriEvent<null>(VENDOR_ACCOUNTS_CHANGED, () => {
    void reloadVendors();
  });

  /**
   * 新人引导注册窗里注册成功（凭据已由后端入库）。收尾对齐目录页手动导入成功的
   * 那一串（`RelayDirectoryPage.authenticate`）：toast + 给当前 app 预配档位 +
   * 整区刷新 —— 用户关掉注册窗回到主界面时，「注册即用」已经成立。
   */
  useTauriEvent<OnboardingRegisterCompleted>(
    ONBOARDING_REGISTER_COMPLETED,
    async (payload) => {
      toast.success(
        t("loongport.addSite.connected", { name: payload.siteName }),
      );
      try {
        presentRefreshResult(await relayApi.refresh(payload.relayId, appId));
      } catch (reason) {
        toast.error(
          t("loongport.directory.provisionFailed", { reason: String(reason) }),
        );
      }
      void reload();
    },
  );

  /**
   * 重新登录一个已有的官网账号（新增账号的登录在 `OfficialApiPage` 里，
   * 那边直接调 `vendor_open_login` —— 同账号重登会靠唯一索引
   * `(vendor_id, account_id)` 合并回同一行）。
   */
  const handleVendorLogin = (vendorId: string, rowId: number) =>
    run(vendorBusyKey("login", rowId), async () => {
      try {
        const result = await vendorApi.openLogin(vendorId, appId);
        // null = 用户自己关了窗或超时，不出提示（他知道自己干了什么）。
        if (result === null) return;
        toast.success(t("loongport.session.connected"));
        presentRefreshResult(result.refresh);
        await reloadVendors();
      } catch (e) {
        toast.error(String(e));
      }
    });

  const handleVendorProvision = (rowId: number) =>
    run(vendorBusyKey("provision", rowId), async () => {
      try {
        presentRefreshResult(await vendorApi.refresh(rowId, appId));
        await reload();
      } catch (error) {
        toast.error(String(error));
      }
    });

  const handleVendorUse = (rowId: number, plan: VendorPlanInfo) =>
    void doVendorSwitch(rowId, plan);

  const doVendorSwitch = (
    rowId: number,
    plan: VendorPlanInfo,
    quitChatgpt?: boolean,
  ) => {
    setConfirmSwitch(null);
    return run(vendorPlanBusyKey("switch", plan.providerId), async () => {
      try {
        const result = await vendorApi.switch(
          rowId,
          plan.planId,
          appId,
          quitChatgpt,
        );
        if (result.status === "confirmationRequired") {
          setConfirmSwitch({
            name: result.targetName,
            run: (choice) => void doVendorSwitch(rowId, plan, choice),
          });
          return;
        }
        const name = plan.planName;
        toast.success(
          result.chatgptRelaunched
            ? t("loongport.switch.doneRelaunched", { name })
            : result.chatgptWasRunning
              ? t("loongport.switch.doneNeedsRestart", { name })
              : t("loongport.switch.done", { name }),
        );
        for (const warning of result.warnings) toast.warning(warning);
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });
  };

  const doRemoveVendor = (row: VendorAccountRow) =>
    run(vendorBusyKey("removeVendor", row.id), async () => {
      try {
        await vendorApi.remove(row.id);
        toast.success(
          t("loongport.vendor.removed", {
            label: row.accountLabel || row.vendorName,
          }),
        );
        // 删掉的可能正是当前在用的那条 ⇒ 全量重读（`reload` 顺带刷 vendors），
        // 否则高亮会停在一个不存在的行上。
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });

  /**
   * 保存官网行的顺序。**只传官网行的 id**（走 `vendor_reorder`）——
   * 两类行的 `sort_index` 各自存在自己的表里，没有共同的序。
   */
  const handleVendorReorder = async (ids: number[]) => {
    try {
      await vendorApi.reorder(ids);
      await reloadVendors();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleLogin = (relayId: number) =>
    run(`login:${relayId}`, async () => {
      try {
        // 显式传 id —— 不传会作用到「当前站」，可能是别的行。
        const result = await relayApi.login(relayId, appId);
        if (result) {
          // 登录窗不会自动关闭（它已跳到 dashboard，用户可能要在那儿充值或看用量）。
          toast.success(t("loongport.session.connected"));
          presentRefreshResult(result);
        }
        // ok === false 是用户自己关了窗口，不出提示（他知道自己干了什么）。
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });

  /** 重新拉这个中转站的可用分组（真的打 sub2api 的 `/groups/available`）。 */
  const handleProvision = (relayId: number) =>
    run(`provision:${relayId}`, async () => {
      try {
        presentRefreshResult(await relayApi.refresh(relayId, appId));
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });

  const handleRefreshAll = () =>
    run("refresh:all", async () => {
      try {
        presentRefreshResult(await relayApi.refreshAll(appId));
        await reload();
      } catch (error) {
        toast.error(String(error));
      }
    });

  /**
   * 把一个档位 / 官网账号的配置恢复成默认值。
   *
   * 「编辑配置」那条路的回头路 —— 用户接手维护后改坏了，这是唯一的退路
   * （`useTierEditGuard` 那道事前警告就是拿它做承诺的）。
   *
   * 两类行合在一处：除了调哪条命令，其余（清确认态、busy、toast、reload）完全相同。
   * 官网那条**只恢复当前 tab 那个平台** —— 一行背后六条记录，一次全恢复会把用户
   * 在别的 tab 里的编辑一起冲掉（见 `vendorApi.resetTierConfig`）。
   */
  const handleResetTier = (target: NonNullable<typeof confirmReset>) => {
    setConfirmReset(null);
    return run(target.busyKey, async () => {
      try {
        if (target.kind === "vendor") {
          await vendorApi.resetTierConfig(target.providerId, appId);
        } else {
          await relayApi.resetTierConfig(target.providerId, appId);
        }
        toast.success(
          t("loongport.tier.resetDone", { name: target.displayName }),
        );
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });
  };

  /**
   * 把「谁在用」拼成弹窗里那串清单：「BestApi · Pro（Codex）、xxx（Claude）」。
   *
   * 每项一条 i18n key（括号样式随 locale 走），连接符交给 `Intl.ListFormat`
   * （zh 用「、」、en 用 ", "，各自正确）—— 不在组件里手写分隔符。
   */
  const formatUsageBlockers = (blockers: RelayUsageBlocker[]) =>
    new Intl.ListFormat(i18n.resolvedLanguage ?? i18n.language).format(
      blockers.map((b) =>
        t("loongport.row.removeConfirmUsageItem", {
          tier: b.tierName,
          app: getAppLabel(b.app),
        }),
      ),
    );

  /**
   * 删掉一行中转站（连带档位）。
   *
   * `force` 只来自「点名 app」的强删确认变体 —— 名下有档位在用的行现在也能删，
   * 但必须经那道弹窗知情确认；后端默认路径仍会拦（闸在后端，见 `relay_remove_site`）。
   */
  const doRemoveRelay = (row: RelayRowData, force = false) =>
    run(`removeRelay:${row.id}`, async () => {
      try {
        await relayApi.removeSite(row.id, force);
        toast.success(
          t("loongport.site.removed", {
            label: row.accountLabel || row.siteName || row.siteOrigin,
          }),
        );
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });

  const doSwitch = (tier: TierInfo, quitChatgpt?: boolean) => {
    setConfirmSwitch(null);
    return run(`switch:${tier.providerId}`, async () => {
      try {
        const result = await relayApi.switchTier(
          tier.providerId,
          appId,
          quitChatgpt,
        );
        if (result.status === "confirmationRequired") {
          setConfirmSwitch({
            name: result.targetName,
            run: (choice) => void doSwitch(tier, choice),
          });
          return;
        }
        // 三个分支各取一个**完整句**的 key，不拼后缀 —— 与 `provision.ready*` 同理
        // （中文靠前置逗号粘接，英/日语序下会散架）。
        toast.success(
          result.chatgptRelaunched
            ? t("loongport.switch.doneRelaunched", {
                name: result.providerName,
              })
            : result.chatgptWasRunning
              ? t("loongport.switch.doneNeedsRestart", {
                  name: result.providerName,
                })
              : t("loongport.switch.done", { name: result.providerName }),
        );
        for (const warning of result.warnings) toast.warning(warning);
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });
  };

  /**
   * 保存用户拖出来的中转站行序。
   *
   * 立刻落库（schema v20 的 `sort_index`）——排序不是纯 UI 状态，
   * 换台机器/重开 app 都该记得。失败要提示：用户明确做了一个动作，
   * 静默失败会让他下次打开发现顺序没变、以为是 bug。
   */
  const handleReorder = async (relayIds: number[]) => {
    try {
      await relayApi.reorder(relayIds);
      await reload();
    } catch (e) {
      toast.error(String(e));
    }
  };

  /**
   * 带登录态开这一行的充值页。
   *
   * busy 标记只覆盖「开窗」这一小段（取一次 profile + 建窗），**不等用户付完钱** ——
   * 窗口开出来之后命令就返回了。所以这个转圈是短的，它防的是连点开出两个窗
   * （后端也会 destroy 残留窗口兜一层）。
   */
  const handlePurchase = (relayId: number) =>
    run(`purchase:${relayId}`, async () => {
      try {
        await relayApi.purchase(relayId);
      } catch (e) {
        // 这个失败要说出来：用户明确点了「充值」，窗口没开出来他得知道为什么
        // （常见原因是登录过期 —— 那时该去重新登录，而不是盯着没反应的界面）。
        toast.error(String(e));
      }
    });

  /**
   * 带登录态开这一行的用量页（「查看用量」）。与充值同一纪律：失败要说出来
   * （常见原因是登录过期），busy 只盖「开窗」那一段。
   */
  const handleOpenUsage = (relayId: number) =>
    run(`openUsage:${relayId}`, async () => {
      try {
        await relayApi.openUsage(relayId);
      } catch (e) {
        toast.error(String(e));
      }
    });

  const handleSwitchTier = (_relayId: number, tier: TierInfo) => {
    if (tier.isCurrent) return;
    void doSwitch(tier);
  };

  const doSelectTierModel = (
    tier: TierInfo,
    model: string,
    quitChatgpt?: boolean,
  ) => {
    setConfirmSwitch(null);
    return run(`model:${tier.providerId}`, async () => {
      try {
        const result = await relayApi.switchTierModel(
          tier.providerId,
          appId,
          model,
          quitChatgpt,
        );
        if (result.status === "confirmationRequired") {
          setConfirmSwitch({
            name: result.targetName,
            run: (choice) => void doSelectTierModel(tier, model, choice),
          });
          return;
        }
        toast.success(
          result.chatgptRelaunched
            ? t("loongport.switch.modelDoneRelaunched", {
                name: result.providerName,
                model,
              })
            : result.chatgptWasRunning
              ? t("loongport.switch.modelDoneNeedsRestart", {
                  name: result.providerName,
                  model,
                })
              : t("loongport.switch.modelDone", {
                  name: result.providerName,
                  model,
                }),
        );
        for (const warning of result.warnings) toast.warning(warning);
        await reload();
      } catch (e) {
        toast.error(String(e));
      }
    });
  };

  const handleSelectTierModel = (tier: TierInfo, model: string) => {
    if (tier.model === model) return;
    void doSelectTierModel(tier, model);
  };

  // 添加入口已全部上收到顶栏大「+」（`AddEntryMenu`），区块内不再有按钮，
  // 空态由各区块内部的占位承接。
  const bothEmpty = relays.length === 0 && vendors.length === 0;

  // 生图页的空态**不引导「添加站点」** —— 生图档位不是单独添加的，它由现有站点里
  // 「只挂 gpt-image 模型的分组」自动产生（`provision::image_tier_app_type`）。
  // 在这里摆一个「添加中转站」按钮会让用户以为要再加一个站，而正确的动作是
  // 去 codex 页点「获取密钥」，或者根本不做（他那个站可能没有生图分组）。
  // codex-image 整页保持改动前的形态，不套三大块布局。
  if (isImageTab && bothEmpty) {
    return <ImageTabNotice empty />;
  }

  return (
    <>
      {/* 生图页顶部的说明。**只在这一页出现** —— 见 `ImageTabNotice` 的文档：
          它是唯一一个不能独立使用的标签，那件事必须写出来。 */}
      {isImageTab && <ImageTabNotice empty={false} />}
      <div className="mb-3 flex justify-end">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="relative h-7 w-7"
          disabled={busy.has("refresh:all")}
          onClick={() => void handleRefreshAll()}
          title={t("loongport.refreshAll")}
          aria-label={t("loongport.refreshAll")}
        >
          {busy.has("refresh:all") ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <>
              <RefreshCw className="h-4 w-4" />
              <Layers3 className="absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-sm bg-background" />
            </>
          )}
        </Button>
      </div>
      {/* 模型验证的行级宿主：summaries 拉取、验真弹窗与结果变化订阅全在
          Provider 内部；下线时它对外不可见（不拉取、入口/徽章不渲染）。 */}
      <TierVerificationProvider
        appId={appId}
        providerIds={verificationProviderIds}
      >
        <RelayTierList
          relays={relays}
          busy={busy}
          onAddSite={() => onOpenAddHub("directory")}
          onLogin={(relayId) => void handleLogin(relayId)}
          onProvision={handleProvision}
          onSiteConfigApplied={() => void reload()}
          onReorder={(ids) => void handleReorder(ids)}
          onSwitchTier={(relayId, tier) => void handleSwitchTier(relayId, tier)}
          onSelectTierModel={(tier, model) =>
            void handleSelectTierModel(tier, model)
          }
          onPurchase={(relayId) => void handlePurchase(relayId)}
          onOpenUsage={(relayId) => void handleOpenUsage(relayId)}
          // 档位的 providerId 就是 provider 表的主键，直接喂给上游那条命令。
          // 名字用 displayName（那是用户在这一行看到的），检测结果的 toast 里会带它。
          onCheckTier={(tier) =>
            void checkProvider(tier.providerId, tier.displayName)
          }
          isCheckingTier={isChecking}
          onResetTier={(tier) =>
            setConfirmReset({
              kind: "tier",
              providerId: tier.providerId,
              displayName: tier.displayName,
              busyKey: `reset:${tier.providerId}`,
            })
          }
          onEditTier={requestEdit}
          onRemoveRelay={(relayId) => {
            // ⚠️ **这处 `find` 保持 number 不动**：`relayId` 从 `RelayRow` 的
            // `onDelete` 一路传回来，只在 relay 这一类里流转。官网行走的是
            // `onRemoveVendor` 那条独立回调，不经过这里。
            const row = relays.find((op) => op.id === relayId);
            if (row) setConfirmRemove(row);
          }}
        />
      </TierVerificationProvider>

      {/* 官网直连账号块 —— 只在支持厂商的 tab 出现（gemini / grokbuild 无 preset，
          摆了也是骗人）。添加入口在顶栏大「+」。 */}
      {vendorSupported && (
        <VendorBlock
          vendor={{
            accounts: vendors,
            onLogin: (rowId) => {
              const row = vendors.find((v) => v.id === rowId);
              if (row) void handleVendorLogin(row.vendorId, rowId);
            },
            onProvision: handleVendorProvision,
            onUse: (rowId, plan) => handleVendorUse(rowId, plan),
            onRemove: (rowId) => {
              const row = vendors.find((v) => v.id === rowId);
              if (row) setConfirmRemoveVendor(row);
            },
            // 编辑走与档位**同一个** `useTierEditGuard`（同一道事前警告、同一个
            // cc-switch 编辑页）。按 plan 编辑 —— 一行（opencode）背后每 plan
            // 各六条记录，各改各的。
            onEdit: (_account, plan) =>
              requestEdit({
                kind: "vendor",
                providerId: plan.providerId,
                displayName: plan.planName,
                isCurrent: plan.isCurrent,
              }),
            onReset: (_account, plan) =>
              setConfirmReset({
                kind: "vendor",
                providerId: plan.providerId,
                displayName: plan.planName,
                busyKey: vendorPlanBusyKey("resetVendor", plan.providerId),
              }),
            onReorder: (ids) => void handleVendorReorder(ids),
          }}
          busy={busy}
          onAddAccount={() => onOpenAddHub("official")}
        />
      )}

      <ConfirmDialog
        isOpen={confirmRemoveVendor !== null}
        title={t("loongport.vendor.removeConfirmTitle")}
        message={t("loongport.vendor.removeConfirmMessage", {
          label:
            confirmRemoveVendor?.accountLabel ||
            confirmRemoveVendor?.vendorName ||
            "",
        })}
        confirmText={t("common.delete")}
        onConfirm={() => {
          if (confirmRemoveVendor) void doRemoveVendor(confirmRemoveVendor);
          setConfirmRemoveVendor(null);
        }}
        onCancel={() => setConfirmRemoveVendor(null)}
      />

      <ConfirmDialog
        isOpen={confirmRemove !== null}
        title={t("loongport.row.removeConfirmTitle")}
        // 文案三个变体，前端只按后端给的事实选：名下有档位在用（强删确认，
        // 点名哪些 app）> 已登录 > 从未登录。`confirmRemove` 为 null 时弹窗
        // 不显示，兜底值不会被看到。
        message={
          confirmRemove && confirmRemove.usageBlockers.length > 0
            ? t("loongport.row.removeConfirmMessageInUse", {
                label:
                  confirmRemove.accountLabel ||
                  confirmRemove.siteName ||
                  confirmRemove.siteOrigin ||
                  "",
                count: confirmRemove.tiers.length,
                usages: formatUsageBlockers(confirmRemove.usageBlockers),
              })
            : t(
                confirmRemove?.removeConfirmation === "configured"
                  ? "loongport.row.removeConfirmMessage"
                  : "loongport.row.removeConfirmMessageNeverLoggedIn",
                {
                  label:
                    confirmRemove?.accountLabel ||
                    confirmRemove?.siteName ||
                    confirmRemove?.siteOrigin ||
                    "",
                  count: confirmRemove?.tiers.length ?? 0,
                },
              )
        }
        confirmText={t("common.delete")}
        onConfirm={() => {
          if (confirmRemove) {
            // 有档位在用 ⇒ 走的是强删变体弹窗，确认即用户知情，带 force 删。
            void doRemoveRelay(
              confirmRemove,
              confirmRemove.usageBlockers.length > 0,
            );
          }
          setConfirmRemove(null);
        }}
        onCancel={() => setConfirmRemove(null)}
      />

      <ConfirmDialog
        isOpen={confirmReset !== null}
        title={t("loongport.tier.resetConfirmTitle")}
        message={t("loongport.tier.resetConfirmMessage", {
          name: confirmReset?.displayName ?? "",
        })}
        confirmText={t("loongport.tier.resetConfirmButton")}
        onConfirm={() => confirmReset && void handleResetTier(confirmReset)}
        onCancel={() => setConfirmReset(null)}
      />

      {/* 「编辑配置」的警告 + cc-switch 编辑页（见 useTierEditGuard）。 */}
      {editDialogs}

      <SwitchTierConfirmDialog
        targetName={confirmSwitch?.name ?? null}
        onCancel={() => setConfirmSwitch(null)}
        onSwitch={(quitChatgpt) => confirmSwitch?.run(quitChatgpt)}
      />
    </>
  );
}
