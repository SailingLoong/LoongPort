import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  relayApi,
  PURCHASE_CLOSED,
  VENDOR_LOGIN_ERROR,
  PROVIDER_SWITCHED,
} from "@/lib/api";
import type { AppId, ProviderSwitchEvent } from "@/lib/api";
import type {
  RelayRow as RelayRowData,
  ProvisionSummary,
  TierInfo,
} from "@/lib/api/relay";
import {
  modelVerificationApi,
  type VerificationReport,
  type VerificationScope,
  type VerificationTarget,
  type VerificationVerdict,
} from "@/lib/api/modelVerification";
import { MODEL_VERIFICATION_CHANGED } from "@/lib/api/events";
import {
  vendorApi,
  vendorSupportsApp,
  DEEPSEEK_API_KEYS_URL,
  DEEPSEEK_VENDOR_ID,
  type VendorAccountRow,
} from "@/lib/api/vendor";
import { useStreamCheck } from "@/hooks/useStreamCheck";
import { useTauriEvent } from "@/hooks/useTauriEvent";

import { AddSiteDialog } from "./AddSiteDialog";
import { ModelVerificationDialog } from "./model-verification/ModelVerificationDialog";
import { openInBrowser } from "./openInBrowser";
import { ImageTabNotice } from "./ImageTabNotice";
import { RelayTierList } from "./RelayTierList";
import { balanceRowsKey, parseBalanceRowsKey } from "./balanceRowsKey";
import { sumTiersForApp } from "./provisionScope";
import { removeConfirmMessageKey } from "./removeConfirmWording";
import { reportProvision } from "./reportProvision";
import { type RowKey, rowKey } from "./rowKey";
import { SwitchTierConfirmDialog } from "./SwitchTierConfirmDialog";
import { useRowBusy } from "./useRowBusy";
import { useTierEditGuard } from "./useTierEditGuard";
import { VendorBlock } from "./VendorBlock";
import { vendorBusyKey } from "./VendorRow";

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
 * 原来还有个 LoongPort 独立页（`OperatorPanel`）并存，它管首启引导、站点切换器、
 * 登出；本组件只管「选档位用起来」。那个页面已删 —— 加站 / 登录 / 获取密钥 /
 * 切档位现在全在这一区，用户不必为任何一步跳页。
 *
 * 删它的理由：站点切换器与「登出」都是那个页面自造的概念（凭据按
 * `site_origin × account_id` 去重，换账号直接登录就是新增一行，不需要先登出），
 * 而剩下的能力这一区本来就有。唯一不可替代的「切回官方登录」搬去了设置页 auth tab。
 *
 * ## 中转站之间没有依赖（2026-08-03 修）
 *
 * 三个行内命令（login / provision / listTierRates）**全部显式带 relayId**，
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
}

/**
 * 「这个进程里已经自动弹过引导了」——**模块级变量，有意不是 state / ref**。
 *
 * 需求是「以进程被打开为计算，进程不消亡就不重复弹」。而这个组件会**反复挂载卸载**：
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
let autoPromptedThisProcess = false;

type TierVerificationVerdict = Extract<
  VerificationVerdict,
  "trusted" | "suspicious" | "anomaly"
>;

function verificationReportKey({
  providerId,
  appType,
  model,
}: VerificationTarget): string {
  return `${providerId}\u0000${appType}\u0000${model}`;
}

function reduceTierVerificationVerdicts(
  reports: Readonly<Record<string, VerificationReport>>,
): Readonly<Record<string, TierVerificationVerdict>> {
  const verdicts: Record<string, TierVerificationVerdict> = {};
  for (const report of Object.values(reports)) {
    if (report.verdict === "anomaly") {
      verdicts[report.target.providerId] = "anomaly";
    } else if (
      report.verdict === "suspicious" &&
      verdicts[report.target.providerId] !== "anomaly"
    ) {
      verdicts[report.target.providerId] = "suspicious";
    } else if (
      report.verdict === "trusted" &&
      verdicts[report.target.providerId] === undefined
    ) {
      verdicts[report.target.providerId] = "trusted";
    }
  }
  return verdicts;
}

function highestSeverityReportForTier(
  reports: Readonly<Record<string, VerificationReport>>,
  providerId: string,
): VerificationReport | null {
  const severity: Record<VerificationVerdict, number> = {
    trusted: 0,
    inconclusive: 1,
    suspicious: 2,
    anomaly: 3,
  };
  return (
    Object.values(reports)
      .filter((report) => report.target.providerId === providerId)
      .sort(
        (left, right) => severity[right.verdict] - severity[left.verdict],
      )[0] ?? null
  );
}

export function RelaySection({ appId }: RelaySectionProps) {
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
  /**
   * 这一屏切档位会不会动到 `~/.codex/` —— 也就是**要不要理 ChatGPT 桌面版**。
   *
   * ## 为什么必须按 app 判，而不是只看 `chatgptNeedsAttention`
   *
   * `chatgptNeedsAttention` 的语义是「这台机器上要不要提示处理 ChatGPT」——
   * 它只关**平台与安装状态**（非 macOS 恒为 true，见 `chatgpt_app::needs_user_attention`），
   * **完全不含「切的是哪个 app」**。
   *
   * 于是 2026-08-05 维护者实测到：**在 claude 页面切档位，也会弹「要不要退出 ChatGPT」
   * 的确认框，选完还提示「请手动重启 ChatGPT」** —— 而 claude 档位写的是
   * `~/.claude/settings.json`，跟 ChatGPT 毫无关系。用户被要求为一件不存在的因果做决定。
   *
   * （这一处此前有句注释说 `chatgptNeedsAttention`「已经包含这个事实」，那是不属实的。）
   *
   * ## 判据
   *
   * ChatGPT 桌面版与命令行 codex **共用 `~/.codex`**，而它只在启动时读那个目录 ⇒
   * 只有会改到 codex 主配置的那一屏才需要这道编排。
   *
   * ⚠️ **`codex-image` 有意不算在内。** 生图档位落的是 MCP 条目、不改 codex 的主模型
   * 与 `base_url`，ChatGPT 桌面版并不消费它 —— 而首次注册那一步本来就要用户新开终端
   * （见 README「在 CLI 里生图」那节），不需要再借道这个确认框。若将来发现桌面版
   * 确实受影响，把它加进来即可，但要先有实测依据，不靠推测。
   */
  const touchesCodexConfig = appId === "codex";
  const supportsModelVerification = appId === "codex" || appId === "claude";
  const [relays, setRelays] = useState<RelayRowData[]>([]);
  const [verificationReports, setVerificationReports] = useState<
    Record<string, VerificationReport>
  >({});
  const [selectedVerificationTier, setSelectedVerificationTier] =
    useState<TierInfo | null>(null);
  const [verificationDialogOpen, setVerificationDialogOpen] = useState(false);
  const [verifyingProviderId, setVerifyingProviderId] = useState<string | null>(
    null,
  );
  /**
   * 待确认的切换：**显示名 + 真正执行它的函数**，`null` = 不弹。
   *
   * ## 为什么不存 `TierInfo`（2026-08-04 改）
   *
   * 原来它是 `TierInfo | null`，于是这道「要不要先退 ChatGPT」的确认框**只有中转站
   * 档位那条路能用**。官网直连账号（vendor / DeepSeek）手上是 `rowId` 不是 `TierInfo`，
   * 塞不进来 ⇒ `handleVendorUse` 当初就硬编码了 `quitChatgpt: false`
   * ⇒ **用户在 codex tab 切到 DeepSeek，ChatGPT 桌面版永远不会被重启**，
   * 而它只在启动时读 `~/.codex/config.toml` ⇒ 新配置对它完全不生效，且不报任何错。
   *
   * 那道编排该绑在「切到某个 codex 配置」这个**动作**上，不绑在「切的是哪一类账号」上
   * —— 两类账号写的是同一个文件、面对的是同一个 ChatGPT 进程。
   * `SwitchTierConfirmDialog` 早就为 cc-switch 那条路解绑成「显示名 + 回调」了，
   * 这里跟上它：存一个闭包，谁调用都行。
   */
  const [confirmSwitch, setConfirmSwitch] = useState<{
    name: string;
    run: (quitChatgpt: boolean) => void;
  } | null>(null);
  const [chatgptNeedsAttention, setChatgptNeedsAttention] = useState(false);
  const [addingSite, setAddingSite] = useState(false);
  // 域名输入框的底纹词，来自 relay_status。
  const [defaultSite, setDefaultSite] = useState("");
  // 各行的余额。**与倍率同一个模式**：不进 listRelays（那条命令只读本地、
  // 首屏不卡网络），渲染完再异步逐行补。
  //
  // 键是**判别式 RowKey**（`"relay:3"`）而不是裸 number：这个 map 与官网行的
  // 那个共处一个列表，而两张表的自增 id 必然重叠 —— 用 number 会让 DeepSeek 的
  // 余额显示到同 id 的中转站行上，且没有任何报错。
  const [balances, setBalances] = useState<Record<RowKey, number | null>>({});
  // `loadBalance` 要在**请求返回时**读到最新的 relays（判账号还是不是同一个），
  // 而它是 useCallback([]) —— 闭包里的 relays 会是旧值。用 ref 取当前值。
  const relaysRef = useRef<RelayRowData[]>([]);
  relaysRef.current = relays;

  // ── 官网直连账号（vendor）──────────────────────────────────────────
  //
  // **与 relay 平级并列的一份状态**，不合进上面那些：两边的命令、DTO 与余额
  // 类型全不同（余额那边是后端格式化好的字符串），合起来只会让每处多一个分支。
  const [vendors, setVendors] = useState<VendorAccountRow[]>([]);
  // 官网行的余额。**值是 string**（`"¥547.08"`，后端已格式化）—— 与上面那条
  // `number` 契约有意分开：改 relay 那条要动 sub2api 那半边，属范围蔓延。
  const [vendorBalances, setVendorBalances] = useState<
    Record<RowKey, string | null>
  >({});
  // 每个官网账号行对应的 provider id。**六个平台共用一个**，由 `vendor_provision`
  // 返回 —— 前端算不出（它是 `sha256(vendor_id + "/" + account_id)`，而行 DTO 里
  // 没有 account_id）。所以「切换」这条路必须先 provision 拿 id 再切。
  //
  // **是 state 不是 ref**：切换时要在它里面找 id，变了得触发重渲染。
  // ⚠️ 它不参与「在用」高亮 —— 那件事由后端 `vendor.list` 返回的 `isCurrent` 现算
  // （与中转站档位同源），前端不再自维护当前态。
  const [vendorProviderIds, setVendorProviderIds] = useState<
    Record<number, string>
  >({});
  const [confirmRemoveVendor, setConfirmRemoveVendor] =
    useState<VendorAccountRow | null>(null);
  // 与 `relaysRef` 同理：`loadVendorBalance` 请求返回时要判这一行还是不是同一个账号。
  const vendorsRef = useRef<VendorAccountRow[]>([]);
  vendorsRef.current = vendors;
  // reload 的请求序号 —— 只让最后一次的结果落地，见 `reload` 里的说明。
  const reloadSeqRef = useRef(0);
  const verificationRequestRef = useRef(0);
  const { t } = useTranslation();
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

  const loadVerificationReports = useCallback(
    async (rows: RelayRowData[]) => {
      const request = ++verificationRequestRef.current;
      if (!supportsModelVerification) {
        setVerificationReports({});
        return;
      }

      const providerIds = [
        ...new Set(
          rows.flatMap((row) =>
            row.tiers
              .filter((tier) => tier.appId === appId)
              .map((tier) => tier.providerId),
          ),
        ),
      ];
      if (providerIds.length === 0) {
        setVerificationReports({});
        return;
      }

      try {
        const reports = await modelVerificationApi.listResults(providerIds);
        if (request !== verificationRequestRef.current) return;
        setVerificationReports(
          Object.fromEntries(
            reports
              .filter(
                (report) =>
                  report.target.appType === appId &&
                  providerIds.includes(report.target.providerId),
              )
              .map((report) => [verificationReportKey(report.target), report]),
          ),
        );
      } catch {
        // Verification summaries are secondary status; retain the last complete backend view.
      }
    },
    [appId, supportsModelVerification],
  );

  const verificationVerdicts = useMemo(
    () => reduceTierVerificationVerdicts(verificationReports),
    [verificationReports],
  );

  const verificationVerdictForTier = useCallback(
    (tier: TierInfo) => verificationVerdicts[tier.providerId],
    [verificationVerdicts],
  );

  const handleVerifyTier = useCallback(
    (tier: TierInfo) => {
      if (
        verifyingProviderId !== null &&
        selectedVerificationTier?.providerId !== tier.providerId
      ) {
        return;
      }
      setSelectedVerificationTier(tier);
      setVerificationDialogOpen(true);
    },
    [selectedVerificationTier, verifyingProviderId],
  );

  const selectedVerificationReport = useMemo(
    () =>
      selectedVerificationTier
        ? highestSeverityReportForTier(
            verificationReports,
            selectedVerificationTier.providerId,
          )
        : null,
    [selectedVerificationTier, verificationReports],
  );

  const handleVerificationRunningChange = useCallback(
    (running: boolean) => {
      setVerifyingProviderId(
        running ? (selectedVerificationTier?.providerId ?? null) : null,
      );
    },
    [selectedVerificationTier],
  );

  /**
   * 拉官网账号列表。**只读本地不发网络**（与 `listRelays` 同一条契约）。
   *
   * 在不支持 DeepSeek 的两个 tab（gemini / grokbuild）下**压根不调它** ——
   * 那两个 tab 里官网行不该出现，拉回来也只能扔掉。
   *
   * `appId` 传给后端**只为算 `userEdited` / `isCurrent`**（一行背后六条 provider
   * 记录，「改过没有」「是不是在用」必须按平台问）。**不是用它过滤行** —— 一把 sk
   * 展开到全部平台，「这一行在哪些 tab 出现」仍由上面那个 `vendorSupportsApp` 判。
   */
  const reloadVendors = useCallback(async () => {
    if (!vendorSupportsApp(appId)) {
      setVendors([]);
      return;
    }
    try {
      setVendors(await vendorApi.list(appId));
    } catch (e) {
      toast.error(String(e));
    }
  }, [appId]);

  /**
   * 读本地档位列表 + 异步补倍率。**不发 provision**（不重拉分组）。
   *
   * `onlySite` 限定只查哪个站的倍率 —— 每个档位一次 HTTP，用户给账号 A 获取
   * 密钥时不该把 B / C 的也全重查一遍。
   *
   * ⚠️ **顺带刷新官网账号列表**（见函数体末尾）：两类行的「当前在用」现在都由后端
   * 现算（`tier.isCurrent` 与 vendor 的 `isCurrent` 同源），一次动作后必须把两类行
   * 一起刷齐，否则切档位后 DeepSeek 行会停在旧高亮上（2026-08-07 修的互斥 bug）。
   */
  const reload = useCallback(
    async (onlySite?: string) => {
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
        void loadVerificationReports(rows);

        // 倍率单独异步补：listRelays 只读本地（首屏不卡网络），倍率必须发请求。
        // **有意不 await** —— 先渲染出来，倍率随后把「倍率未知」换成数字。
        // 失败不提示：倍率是附加信息，为它弹 toast 会打断主流程。
        if (rows.some((op) => op.tiers.length > 0)) {
          relayApi
            .listTierRates(appId, onlySite)
            .then((rates) => {
              // 倍率回来时可能已经有新一轮 reload 换掉了行 —— 那时这些倍率
              // 属于旧的一批档位，往新行上贴是错的。
              if (isStale()) return;
              const byId = new Map(rates.map((r) => [r.providerId, r]));
              setRelays((prev) =>
                prev.map((op) => ({
                  ...op,
                  tiers: op.tiers.map((tier) => {
                    const hit = byId.get(tier.providerId);
                    // 查不到的保持 null（继续显示「倍率未知」），别覆盖成 0。
                    return hit
                      ? { ...tier, rateMultiplier: hit.rateMultiplier }
                      : tier;
                  }),
                })),
              );
            })
            .catch(() => {});
        }
      } catch (e) {
        toast.error(String(e));
      }
      // ⚠️ **官网行必须跟档位一起刷**（见上方 doc）：两类行的「当前在用」同源，
      // 只刷一边就会让切完档位后 DeepSeek 行继续显示旧的「在用」高亮。
      void reloadVendors();
    },
    [appId, loadVerificationReports, reloadVendors],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  // 「编辑配置」的事前警告 + 编辑页 + 保存后刷新（见 useTierEditGuard）。
  // 保存后必须 reload：「已手动维护」标记由后端按当前配置现算，不刷新拿不到新值。
  const { requestEdit, editDialogs } = useTierEditGuard(appId, reload);

  /**
   * 拉某一行的余额。**逐行拉，而且只拉已登录的行。**
   *
   * 为什么不在 `reload` 里对所有行无条件拉：每行一次 HTTP，而 `reload` 在每次登录 /
   * 获取密钥 / 切档位后都会跑 —— 那会把「刷新一次 = N 个请求」放大到每个动作上。
   * 已登录才拉：没登录的行必然拿不到（`usable_relay` 会 Err），白打一次还报错。
   *
   * 失败静默存 `null`：中转站可能关了用户面板、这一行可能刚过期。
   * **余额是附加信息，为它弹 toast 会打断主流程**（与倍率同一条纪律）。
   *
   * ## `accountLabel` 快照防的是一个真实竞态（review 抓出）
   *
   * 同一个 id 可以先后属于两个账号：用户在这一行登出 A、再登录 B，行 id 不变。
   * 若 A 那次慢请求在 B 登录之后才返回，就会把 **A 的余额显示在 B 的行上**。
   * 所以落状态前比一次账号标签，变了就丢弃这次结果。
   */
  const loadBalance = useCallback(
    async (relayId: number, accountAtRequest: string) => {
      // ⚠️ **这处 `find` 保持 number 不动**：`relayId` 只在 relay 这一类里
      // 流转（调用方传的就是 `op.id`），不参与跨类索引。改成 RowKey 只是扩大
      // 改动面。真正需要判别式键的是下面 `setBalances` 的那个 Record。
      const stillSameAccount = () =>
        relaysRef.current.find((op) => op.id === relayId)?.accountLabel ===
        accountAtRequest;
      const key = rowKey("relay", relayId);
      try {
        const b = await relayApi.balance(relayId);
        if (!stillSameAccount()) return;
        setBalances((prev) => ({ ...prev, [key]: b.balance }));
      } catch {
        if (!stillSameAccount()) return;
        setBalances((prev) => ({ ...prev, [key]: null }));
      }
    },
    [],
  );

  // 行列表变了就补齐余额。依赖是**已登录行的摘要字符串**而不是 `relays` ——
  // 后者每次 reload 都是新对象引用，会让这个 effect 每次都跑、把 N 个请求重发一遍。
  // 编解码收在 `balanceRowsKey`（那里写了为什么不能用逗号拼接：昵称含逗号会造出一个
  // id 为 NaN 的伪造条目，症状是那一行永远没有余额、于是也没有充值入口）。
  const loggedInRowsKey = balanceRowsKey(
    relays.filter((op) => op.loggedIn).map((op) => [op.id, op.accountLabel]),
  );
  useEffect(() => {
    for (const [id, accountLabel] of parseBalanceRowsKey(loggedInRowsKey)) {
      void loadBalance(id, accountLabel);
    }
  }, [loggedInRowsKey, loadBalance]);

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
    const row = relaysRef.current.find((op) => op.id === relayId);
    // 行已经不在了（用户删掉了它）就别拉 —— 那次请求必然报错，且没有地方显示结果。
    if (row) void loadBalance(relayId, row.accountLabel);
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

  useTauriEvent<VerificationScope>(MODEL_VERIFICATION_CHANGED, (scope) => {
    if (
      !supportsModelVerification ||
      scope?.appType !== appId ||
      !relaysRef.current.some((row) =>
        row.tiers.some((tier) => tier.providerId === scope.providerId),
      )
    ) {
      return;
    }
    void loadVerificationReports(relaysRef.current);
  });

  // 切换档位前要不要问「先退 ChatGPT 吗」。只读一次（它探的是「装了没有」这类事实，
  // 不随操作变化），失败当作「不必问」—— 那时切换照常，只是不弹确认框。
  useEffect(() => {
    relayApi
      .status()
      .then((s) => {
        setChatgptNeedsAttention(s.chatgptNeedsAttention);
        setDefaultSite(s.defaultSite);
      })
      .catch(() => {});
  }, []);

  /**
   * 启动时探一次凭据是不是真的还活着。
   *
   * `status.loggedIn` 只看本地记的**过期时间**。凭据在网页端被撤销、账号被禁用、
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
   * 一个站点都没有时自动弹「添加站点」引导。**每个进程只弹一次。**
   *
   * ## 判据为什么是「全局有没有站点」而不是这一区渲染出了几行
   *
   * 这个组件是 per-tab 的（`appId`），而两类行都会被 tab 过滤掉：
   * `relays` 只含**当前 app 下**有档位的中转站，`vendors` 在 gemini /
   * grokbuild 两个 tab 下**恒为空数组**（`reloadVendors` 里直接短路，官网行在那两个
   * tab 不该出现）。拿它们当判据的话，用户在 gemini tab 下会被弹一次引导 ——
   * 而他明明已经配好了 DeepSeek。
   *
   * 所以判据走两条**不吃 app 参数**的命令：`relay_list_sites`（含未登录的占位行 ——
   * 「加了站但还没登录」也算配过，不该再弹引导）与 `vendor_list_accounts`。
   *
   * ## 失败时不弹
   *
   * 两条命令读的都是本地 SQLite，失败基本只有「库坏了」。那时弹引导是错的方向 ——
   * 用户加站也会失败，只会收到第二条错误。
   */
  useEffect(() => {
    if (autoPromptedThisProcess) return;
    let cancelled = false;
    void (async () => {
      try {
        const [sites, vendorRows] = await Promise.all([
          relayApi.listSites(),
          // 这里只数 `length`（判「一个都没配过」），`userEdited` 用不上 ——
          // 但参数是必填的，给当前 tab 就行。
          vendorApi.list(appId),
        ]);
        if (cancelled || autoPromptedThisProcess) return;
        if (sites.length === 0 && vendorRows.length === 0) {
          // 先置标志再开弹窗：用户关掉之后这个 effect 可能因为重挂再跑一次，
          // 标志已经是 true 就不会再弹。
          autoPromptedThisProcess = true;
          setAddingSite(true);
        }
      } catch {
        // 见上：读不出来时什么都不做。
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  /**
   * 拉一行官网账号的余额。
   *
   * 与 relay 那条同形（逐行拉、失败静默存 null、落状态前比一次账号标签防串行），
   * 差别只有两处：值是**已格式化的字符串**（前端不碰）、判据是 `loggedIn`
   * 而不是 `keyReady` —— 余额要网页登录态，sk 有效但登录过期的行拉不到。
   */
  const loadVendorBalance = useCallback(
    async (rowId: number, accountAtRequest: string) => {
      const stillSameAccount = () =>
        vendorsRef.current.find((v) => v.id === rowId)?.accountLabel ===
        accountAtRequest;
      const key = rowKey("vendor", rowId);
      try {
        const b = await vendorApi.balance(rowId);
        if (!stillSameAccount()) return;
        setVendorBalances((prev) => ({ ...prev, [key]: b }));
      } catch {
        if (!stillSameAccount()) return;
        setVendorBalances((prev) => ({ ...prev, [key]: null }));
      }
    },
    [],
  );

  // 与 relay 侧那个 effect 同形，编解码共用 `balanceRowsKey`（同一份往返逻辑，
  // 别在这里另写一遍 —— 两份里有一份没跟上正是那个逗号 bug 的形状）。
  const loggedInVendorsKey = balanceRowsKey(
    vendors.filter((v) => v.loggedIn).map((v) => [v.id, v.accountLabel]),
  );
  useEffect(() => {
    for (const [id, accountLabel] of parseBalanceRowsKey(loggedInVendorsKey)) {
      void loadVendorBalance(id, accountLabel);
    }
  }, [loggedInVendorsKey, loadVendorBalance]);

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
   * 备好一行官网账号的密钥。返回 provider id（切换要用）。
   *
   * ⚠️ **只在 `keyCreated` 时提示「已在官网新建密钥」** —— 本地已有明文时这条命令
   * 是零请求的正常路径，每次都提示会让用户以为在重复建 key。
   *
   * 超上限（官网 100 把）时 toast 带一个「去官网删」的入口 —— 指路而不是只说不允许。
   * ⚠️ 判据只能靠文案匹配：`vendor_provision` 把 `VendorError` 经 `AppError` 拍成了
   * 字符串，前端拿不到变体名。所以这里认后端那句文案里的「100」+「官网」两个特征，
   * 匹配不上就退化成普通错误 toast（不会误报，最坏是少一个按钮）。
   */
  const doVendorProvision = useCallback(
    async (rowId: number): Promise<string | null> => {
      try {
        const r = await vendorApi.provision(rowId);
        setVendorProviderIds((prev) => ({ ...prev, [rowId]: r.providerId }));
        toast.success(
          r.keyCreated
            ? t("loongport.vendor.keyCreated", { count: r.platforms.length })
            : t("loongport.vendor.keyReady", { count: r.platforms.length }),
        );
        if (r.mergedProviders.length > 0) {
          toast.info(
            t("loongport.provision.mergedProviders", {
              count: r.mergedProviders.length,
            }),
          );
        }
        return r.providerId;
      } catch (e) {
        const msg = String(e);
        if (msg.includes("100") && msg.includes("官网")) {
          // ⚠️ **外链必须走真实的 `<a target="_blank">` 点击**，不能用 `window.open`：
          // Tauri 的 opener 插件是在 Rust 侧接管 **DOM 里的链接点击**的，而它的
          // **JS 包（`@tauri-apps/plugin-opener`）本仓没装** —— 所以既没有
          // `openUrl()` 可调，`window.open` 在 WebView 里也不保证被送到系统浏览器
          // （最坏是被吞掉，按钮点了什么都不发生）。仓里既有的四处外链
          // （`ApiKeySection` / `CodexOAuthSection` / …）全是 `<a target="_blank">`，
          // 那是本仓唯一验证过的路子，照它做。
          //
          // toast 的 action 只吃 onClick ⇒ 在 onClick 里合成一次 <a> 点击。
          // 这不是 hack 而是同一条路径的程序化触发：走的还是 DOM 点击那条链。
          toast.error(msg, {
            action: {
              label: t("loongport.vendor.openKeyPage"),
              onClick: () => openInBrowser(DEEPSEEK_API_KEYS_URL),
            },
          });
        } else {
          toast.error(msg);
        }
        return null;
      }
    },
    [t],
  );

  /**
   * 登录（或重新登录）一个官网账号。
   *
   * ⚠️ **入口是「添加官网账号」按钮时没有行**（行是登录成功后由后端建的），所以
   * `rowId` 为 null 表示新增。`vendor_open_login` 吃的是 `vendorId` 而不是行 id，
   * 天然支持这两种情形 —— 同账号重登会合并回同一行（唯一索引 `(vendor_id, account_id)`）。
   */
  const handleVendorLogin = (vendorId: string, rowId: number | null) =>
    run(
      rowId === null ? "vendorLogin:new" : vendorBusyKey("login", rowId),
      async () => {
        // 新增那条路：**登录前先记下已有的行 id**，登录后靠差集认出新建的那行。
        //
        // ⚠️ 不能靠「列表里最后一个该 vendorId 的行」—— `creds::list` 按
        // `sort_index, id` 排序，用户拖动过之后新行不一定在末尾 ⇒ 会给**别的账号**
        // 备密钥（那个账号的 sk 被换成新建的，而新登录的账号什么也没拿到）。
        //
        // 差集在「新增」这条路上是准的：唯一索引是 `(vendor_id, account_id)`，
        // 所以要么多出一行（新账号），要么行数不变（登录的是已存在的账号 ——
        // 那种情况下差集为空，落到下面的 `existing` 分支按 account_label 找回来）。
        const idsBefore = new Set(vendorsRef.current.map((v) => v.id));
        try {
          const ok = await vendorApi.openLogin(vendorId);
          // false = 用户自己关了窗或超时，不出提示（他知道自己干了什么）。
          if (!ok) return;
          toast.success(t("loongport.session.connected"));
          // 先刷列表再 provision：登录成功后行才存在（新增那条路），而
          // provision 要拿行 id。
          const rows = await vendorApi.list(appId);
          setVendors(rows);

          const target =
            rowId !== null
              ? // 重登：就是原来那一行（后端按 `(vendor_id, account_id)` 合并回它）。
                rows.find((v) => v.id === rowId)
              : // 新增：差集里那一行。用户点「添加账号」但登的是已存在的账号时
                // 差集为空 —— 那种情况下 `openLogin` 已经把 token 更新到那一行了，
                // 靠 vendorId + 有登录态定位（同厂商多账号时可能有多行满足，
                // 但它们的 sk 各自独立、provision 是幂等的，挑错也不会串账号）。
                (rows.find((v) => !idsBefore.has(v.id)) ??
                rows.find((v) => v.vendorId === vendorId && v.loggedIn));
          if (!target) return;
          // 直接把密钥备好 —— 不该再让用户点一次。
          await doVendorProvision(target.id);
          await reloadVendors();
        } catch (e) {
          toast.error(String(e));
        }
      },
    );

  const handleVendorProvision = (rowId: number) =>
    run(vendorBusyKey("provision", rowId), async () => {
      await doVendorProvision(rowId);
      await reloadVendors();
    });

  /**
   * 切到某个官网账号的配置。
   *
   * ## ⚠️ 必须走 `relay_switch_tier`，**不能**走上游的 `switch_provider`
   *
   * 初版写的是 `providersApi.switch`，论证是「vendor 的产物就是普通 provider 记录，
   * 切换零改动复用上游」。**那条路 100% 走不通**（final review 实测抓出）：
   *
   * `switch_provider`（`commands/provider.rs:153`）第一件事就是
   * `reject_if_managed(id)`，而 vendor 的 id 是 `loongport-vendor-<hash>`、
   * 命中 `MANAGED_ID_PREFIX` ⇒ 直接返回守卫那条「请在中转站区操作」——
   * 而用户**就在**中转站区，那句指路等于告诉他「去你已经在的地方」，
   * 他没有任何路径能切到 DeepSeek。
   *
   * 实测：`reject_if_managed("loongport-vendor-0c0a4a3c49b25d60")` → `Err`。
   *
   * `relay_switch_tier` 在守卫**之内**（它就是那个「中转站区里的操作」），
   * 且顺带拿到「退 ChatGPT → 切 → 重开」那套编排 —— codex 是 DeepSeek 六平台之一、
   * ChatGPT 桌面版与命令行 codex 共用 `~/.codex`，所以那道编排对 vendor 同样成立。
   *
   * ⚠️ 先 `provision` 拿 provider id：那个 id 是 `sha256(vendor_id + "/" + account_id)`，
   * 前端算不出（行 DTO 里没有 account_id，也没有 sha256）。这一步在本地已有明文时
   * 是**零请求**的，所以不是额外的网络开销。
   *
   * ## ⚠️ 「要不要先退 ChatGPT」这道确认框对官网账号同样成立（2026-08-04 修的 bug）
   *
   * 这里原来硬编码 `quitChatgpt: false`，理由写的是「那道确认框归中转站档位那条路」。
   * **那个理由是错的**：两类账号写的是同一个 `~/.codex/config.toml`、面对的是同一个
   * ChatGPT 桌面版进程，而它**只在启动时读那个文件**。所以不重启它 ⇒ 用户在 codex tab
   * 切到 DeepSeek 之后，桌面版仍连着旧配置，且**不报任何错**（静默失效）。
   *
   * 判据该是「切的是不是 codex 配置」（`chatgptNeedsAttention` 已经包含这个事实），
   * 不是「切的是哪一类账号」。所以走与 `handleSwitchTier` 完全同一条路。
   */
  const handleVendorUse = (rowId: number) => {
    const row = vendorsRef.current.find((v) => v.id === rowId);
    const name = row?.vendorName ?? String(rowId);
    // 两个条件都要成立才问：这一屏会动 codex 配置，且这台机器上装着 ChatGPT。
    // 少了前者就会在 claude 页面问一件无关的事（见 `touchesCodexConfig` 的说明）。
    if (touchesCodexConfig && chatgptNeedsAttention) {
      setConfirmSwitch({
        name,
        run: (quitChatgpt) => void doVendorSwitch(rowId, quitChatgpt),
      });
    } else {
      void doVendorSwitch(rowId, false);
    }
  };

  /** `handleVendorUse` 确认之后真正执行的那一步（与 `doSwitch` 对位）。 */
  const doVendorSwitch = (rowId: number, quitChatgpt: boolean) => {
    setConfirmSwitch(null);
    return run(vendorBusyKey("switch", rowId), async () => {
      try {
        // 优先用行 DTO 的 id（后端派生、app 重启后仍有效）；
        // 空串说明还没登录过 ⇒ 回落到 provision（它本地有明文时是零请求）。
        const row0 = vendorsRef.current.find((v) => v.id === rowId);
        const providerId =
          row0?.providerId ||
          vendorProviderIds[rowId] ||
          (await doVendorProvision(rowId));
        if (!providerId) return;
        const r = await relayApi.switchTier(providerId, appId, quitChatgpt);
        const row = vendorsRef.current.find((v) => v.id === rowId);
        const name = row?.vendorName ?? providerId;
        // 三个分支与 `doSwitch` 同形 —— 原来这里恒用 `switch.done`，
        // 于是替用户重开了 ChatGPT 也不说、没重开也不提醒他自己重启。
        toast.success(
          r.chatgptRelaunched
            ? t("loongport.switch.doneRelaunched", { name })
            : r.chatgptWasRunning
              ? t("loongport.switch.doneNeedsRestart", { name })
              : t("loongport.switch.done", { name }),
        );
        for (const w of r.warnings) toast.warning(w);
        // 切到官网行会同时改两类行的「在用」：`reload` 顺带刷 vendors（见其 doc）。
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
        setVendorProviderIds((prev) => {
          const next = { ...prev };
          delete next[row.id];
          return next;
        });
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

  /**
   * 这一行的配置是不是当前 tab 正在用的那个。
   *
   * ⚠️ **直接读后端算好的 `isCurrent`** —— `vendorApi.list(appId)` 返回的 DTO 里，
   * 后端已按 `providers.is_current` 现算（与中转站档位的 `tier.isCurrent` 同源）。
   * 前端**不再自维护 / 不自比较** current 值 —— 曾经历过「前端自己算 current」导致
   * 切档位后 DeepSeek 行高亮停在上一个值、与档位同时显示「在用」的 bug（2026-08-07）。
   *
   * 空 id / 未登录的行后端给 `false` —— 那种行本来也不该高亮。
   */
  const isVendorCurrent = useCallback(
    (rowId: number) => {
      const row = vendors.find((v) => v.id === rowId);
      return row?.isCurrent ?? false;
    },
    [vendors],
  );

  const handleLogin = (relayId: number) =>
    run(`login:${relayId}`, async () => {
      try {
        // 显式传 id —— 不传会作用到「当前站」，可能是别的行。
        const ok = await relayApi.login(relayId);
        if (ok) {
          // 登录窗不会自动关闭（它已跳到 dashboard，用户可能要在那儿充值或看用量）。
          toast.success(t("loongport.session.connected"));
          // 直接把密钥备好 —— 不该再让用户点一次。
          reportProvision(t, await relayApi.provision(relayId), appId);
        }
        // ok === false 是用户自己关了窗口，不出提示（他知道自己干了什么）。
        await reload(relays.find((op) => op.id === relayId)?.siteOrigin);
      } catch (e) {
        toast.error(String(e));
      }
    });

  /** 重新拉这个中转站的可用分组（真的打 sub2api 的 `/groups/available`）。 */
  const handleProvision = (relayId: number) =>
    run(`provision:${relayId}`, async () => {
      try {
        reportProvision(t, await relayApi.provision(relayId), appId);
        // 只刷这一个中转站的倍率 —— 别的账号没变，重查它们纯属浪费请求。
        await reload(relays.find((op) => op.id === relayId)?.siteOrigin);
      } catch (e) {
        toast.error(String(e));
      }
    });

  /**
   * 顶部「刷新」：**对所有已登录的中转站重新拉分组**。
   *
   * ## 为什么它必须真的重拉（用户实测发现）
   *
   * 原来这个按钮只跑 `listRelays`（读本地 DB）+ `listTierRates`（只查倍率），
   * **没有任何一条路会重新拉 `/groups/available`** ⇒ 中转站在网页端新增了一个
   * 分组，点「刷新」永远看不到；而「获取密钥」按钮只在 `tiers.length === 0`
   * 时才显示，已有档位的行压根没有重拉入口。一个叫「刷新」的按钮刷不出新数据，
   * 是名不符实。
   *
   * 并发跑（`Promise.allSettled`）而不是串行：中转站之间无依赖，串行会让 N 个
   * 站的等待时间叠加。用 allSettled 而不是 all —— 一个站失败（网络/登录过期）
   * 不该让别的站的结果一起丢掉。
   *
   * 未登录的行跳过：它必然没有分组，白打一次请求还会报错。
   */
  const handleRefreshAll = () =>
    run("refresh:all", async () => {
      const targets = relays.filter((op) => op.loggedIn);
      if (targets.length === 0) {
        // 一个都没登录时退化成纯本地重载 —— 至少把别的 tab 里 provision 出来的
        // 档位显示出来（那种情况本地 DB 确实有新数据）。
        await reload();
        return;
      }

      const results = await Promise.allSettled(
        targets.map((op) => relayApi.provision(op.id)),
      );

      let keysCreated = 0;
      // ⚠️ **成功数必须自己数，不能用 `targets.length`**（review 抓出）。
      //
      // 那是「发起了几个请求」，不是「成功了几个」⇒ 全部失败时也会先弹一句
      // 「已刷新 3 个中转站」，紧接着再弹 3 条错误。用户看到的第一句话是假的，
      // 而那句恰好是绿色的成功提示 —— 比不提示更糟。
      let succeeded = 0;
      // ⚠️ **连原因一起收**（维护者实测抓出）：原来只 push 站名、把 `r.reason`
      // 整个丢掉，而后端那条路径也不落日志 ⇒ 两处一叠，用户只看到「<站名> 刷新失败」，
      // 定位一次要手工从 DB 取 token 逐个端点 curl。
      const failed: { name: string; reason: string }[] = [];
      // 成功项单独收一份 —— 档位数的累加交给 `sumTiersForApp`（见它的文档：
      // 内联 `+=` 那种写法没有任何闸钉得住，实测改错了 678 条测试全绿）。
      const ok: ProvisionSummary[] = [];
      results.forEach((r, i) => {
        if (r.status === "fulfilled") {
          succeeded += 1;
          ok.push(r.value);
          keysCreated += r.value.keysCreated;
          for (const f of r.value.failures) {
            toast.warning(
              t("loongport.provision.groupFailed", {
                group: f.groupName,
                reason: f.reason,
              }),
            );
          }
        } else {
          failed.push({
            name: targets[i].siteName || targets[i].siteOrigin,
            reason: String(r.reason),
          });
        }
      });

      // ⚠️ **只数当前平台的** —— `tiers` 是全平台的（provision 一次探全部平台）。
      // 累总数会说出「共 9 个档位」而用户眼前那一屏只有 3 个，且那句是绿色的
      // 成功语气 ⇒ 他分不清是提示错了还是界面漏了。
      const tierTotal = sumTiersForApp(ok, appId);

      // ⚠️ 这四条文案原来是**中文硬编码**（en/ja/zh-TW 用户看到中文），已接进 i18n
      // （复用 `provision.*` 那批按语义命名的 key，见 `reportProvision` 上方）。
      //
      // `readyWithKeys` / `refreshed` 分开取而不是拼一个「，新建 N 把密钥」后缀：
      // 那种拼法在英/日语序下会散架（那也是当初 `provision.*` 拆成完整句分支的理由）。
      //
      // **一个都没成功时整句不弹**：那种情况下面的错误 toast 已经把每一条都点名了，
      // 再来一句「已刷新 0 个中转站」纯属噪音（而且是绿色的）。
      if (succeeded > 0) {
        toast.success(
          keysCreated > 0
            ? t("loongport.provision.refreshedWithKeys", {
                relays: succeeded,
                tiers: tierTotal,
                keys: keysCreated,
              })
            : t("loongport.provision.refreshed", {
                relays: succeeded,
                tiers: tierTotal,
              }),
        );
      }
      // 失败的如实点名**并带原因** —— 只说「刷新失败」等于让用户去猜，
      // 而他能做的处置（重新登录 / 检查网络 / 等中转站恢复）完全取决于原因。
      for (const { name, reason } of failed) {
        toast.error(
          t("loongport.provision.refreshFailedWithReason", { name, reason }),
        );
      }

      // 全量重载（含倍率）—— 这是显式的全局刷新，用户愿意等。
      await reload();

      // ⚠️ **余额也要重拉**（review 抓出的死路）。
      //
      // 余额只由那个 `loggedInRowsKey` effect 触发，而它的依赖是 `id:accountLabel` ——
      // 一旦某行的余额请求失败过（网络抖动），那个键不变 ⇒ **effect 永远不会再跑**，
      // 那一行整个会话都没有余额；而充值按钮只在有余额时才存在 ⇒ 用户连入口都看不到，
      // 点「刷新」也没用。这里补上正是因为「刷新」就是用户表达「把这页弄成最新」的动作。
      for (const op of targets) {
        void loadBalance(op.id, op.accountLabel);
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

  /** 删掉一行中转站（连带档位）。有档位在用的行按钮不可点，走不到这里。 */
  const doRemoveRelay = (row: RelayRowData) =>
    run(`removeRelay:${row.id}`, async () => {
      try {
        await relayApi.removeSite(row.id);
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

  const doSwitch = (tier: TierInfo, quitChatgpt: boolean) => {
    setConfirmSwitch(null);
    return run(`switch:${tier.providerId}`, async () => {
      try {
        const r = await relayApi.switchTier(
          tier.providerId,
          appId,
          quitChatgpt,
        );
        // 三个分支各取一个**完整句**的 key，不拼后缀 —— 与 `provision.ready*` 同理
        // （中文靠前置逗号粘接，英/日语序下会散架）。
        toast.success(
          r.chatgptRelaunched
            ? t("loongport.switch.doneRelaunched", { name: r.providerName })
            : r.chatgptWasRunning
              ? t("loongport.switch.doneNeedsRestart", {
                  name: r.providerName,
                })
              : t("loongport.switch.done", { name: r.providerName }),
        );
        for (const w of r.warnings) toast.warning(w);
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

  const handleSwitchTier = (_relayId: number, tier: TierInfo) => {
    if (tier.isCurrent) return;
    // 两个条件都要成立才问，见 `touchesCodexConfig` 的说明 ——
    // 只看 `chatgptNeedsAttention` 会在 claude / gemini 页面问一件无关的事。
    if (touchesCodexConfig && chatgptNeedsAttention) {
      setConfirmSwitch({
        name: tier.displayName,
        run: (quitChatgpt) => void doSwitch(tier, quitChatgpt),
      });
    } else {
      void doSwitch(tier, false);
    }
  };

  const doSelectTierModel = (
    tier: TierInfo,
    model: string,
    quitChatgpt: boolean,
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
    if (!touchesCodexConfig || tier.model === model) return;
    const name = `${tier.displayName} · ${model}`;
    if (touchesCodexConfig && chatgptNeedsAttention) {
      setConfirmSwitch({
        name,
        run: (quitChatgpt) => void doSelectTierModel(tier, model, quitChatgpt),
      });
    } else {
      void doSelectTierModel(tier, model, false);
    }
  };

  // 两个区块的添加入口都在各自区块头（`RelayTierList` 的 + / `VendorBlock` 的 +），
  // 所以不再有「两类都空时单摆按钮」的空态分支 —— 空态由各区块内部的占位承接。
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
      <RelayTierList
        relays={relays}
        busy={busy}
        onAddSite={() => setAddingSite(true)}
        onRefresh={() => void handleRefreshAll()}
        onLogin={(relayId) => void handleLogin(relayId)}
        onProvision={(relayId) => void handleProvision(relayId)}
        onReorder={(ids) => void handleReorder(ids)}
        onSwitchTier={(relayId, tier) => void handleSwitchTier(relayId, tier)}
        onSelectTierModel={(tier, model) =>
          void handleSelectTierModel(tier, model)
        }
        balances={balances}
        onPurchase={(relayId) => void handlePurchase(relayId)}
        // 档位的 providerId 就是 provider 表的主键，直接喂给上游那条命令。
        // 名字用 displayName（那是用户在这一行看到的），检测结果的 toast 里会带它。
        onCheckTier={(tier) =>
          void checkProvider(tier.providerId, tier.displayName)
        }
        isCheckingTier={isChecking}
        verificationVerdictForTier={verificationVerdictForTier}
        onVerifyTier={supportsModelVerification ? handleVerifyTier : undefined}
        isVerifyingTier={(providerId) => providerId === verifyingProviderId}
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

      {selectedVerificationTier && (
        <ModelVerificationDialog
          key={`${selectedVerificationTier.providerId}:${appId}`}
          providerId={selectedVerificationTier.providerId}
          appType={appId}
          tierDisplayName={selectedVerificationTier.displayName}
          open={verificationDialogOpen}
          onOpenChange={setVerificationDialogOpen}
          onRunningChange={handleVerificationRunningChange}
          report={selectedVerificationReport}
        />
      )}

      {/* 官网直连账号块 —— 只在支持厂商的 tab 出现（gemini / grokbuild 无 preset，
          摆了也是骗人）。「添加官网账号」入口在它自己的区块头。 */}
      {vendorSupportsApp(appId) && (
        <VendorBlock
          vendor={{
            accounts: vendors,
            balances: vendorBalances,
            isCurrent: isVendorCurrent,
            onLogin: (rowId) => {
              const row = vendors.find((v) => v.id === rowId);
              if (row) void handleVendorLogin(row.vendorId, rowId);
            },
            onProvision: (rowId) => void handleVendorProvision(rowId),
            onUse: (rowId) => void handleVendorUse(rowId),
            onRemove: (rowId) => {
              const row = vendors.find((v) => v.id === rowId);
              if (row) setConfirmRemoveVendor(row);
            },
            // 编辑走与档位**同一个** `useTierEditGuard`（同一道事前警告、同一个
            // cc-switch 编辑页）。`accountLabel` 空时回落厂商名 —— 弹窗标题里
            // 空字符串会读成「手动编辑「」的配置」。
            onEdit: (account) =>
              requestEdit({
                kind: "vendor",
                providerId: account.providerId,
                displayName: account.accountLabel || account.vendorName,
                isCurrent: isVendorCurrent(account.id),
              }),
            onReset: (account) =>
              setConfirmReset({
                kind: "vendor",
                providerId: account.providerId,
                displayName: account.accountLabel || account.vendorName,
                busyKey: vendorBusyKey("resetVendor", account.id),
              }),
            onReorder: (ids) => void handleVendorReorder(ids),
          }}
          busy={busy}
          onAddVendor={() => void handleVendorLogin(DEEPSEEK_VENDOR_ID, null)}
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
        // 文案按「这一行登录过没有」分两句 —— 判据见 `removeConfirmWording.ts`
        // （从没登录的行既没有登录态也没有余额，无条件那句话会说错两处）。
        // `confirmRemove` 为 null 时弹窗不显示，此处的兜底值不会被看到。
        message={t(
          removeConfirmMessageKey({
            loggedIn: confirmRemove?.loggedIn ?? false,
            sessionExpired: confirmRemove?.sessionExpired ?? false,
          }),
          {
            label:
              confirmRemove?.accountLabel ||
              confirmRemove?.siteName ||
              confirmRemove?.siteOrigin ||
              "",
            count: confirmRemove?.tiers.length ?? 0,
          },
        )}
        confirmText={t("common.delete")}
        onConfirm={() => {
          if (confirmRemove) void doRemoveRelay(confirmRemove);
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

      {/* `isFirstRun`：一个站都没有时（两个区块都空）弹窗出引导文案
          （「选择服务站点」而不是「添加另一个中转站」）。**它只换文案，仍然可关闭**
          —— 见 `AddSiteDialogProps.isFirstRun` 的文档。 */}
      <AddSiteDialog
        open={addingSite}
        onClose={() => setAddingSite(false)}
        onAdded={() => void reload()}
        defaultSite={defaultSite}
        appId={appId}
        isFirstRun={bothEmpty}
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
