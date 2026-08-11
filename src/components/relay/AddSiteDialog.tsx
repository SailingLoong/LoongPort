import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { relayApi } from "@/lib/api";
import type { AppId } from "@/lib/api";
import type { RelayImportError } from "@/lib/api/relay";
// 类型从具体模块拿 —— `@/lib/api` 那个 barrel 只导出 `relayApi`
// （与 `RelayRow` / `RelayTierList` 同一写法）。
import type { SiteInfo, Sponsor } from "@/lib/api/relay";
import { reportProvision } from "./reportProvision";

/**
 * 「添加中转站」弹窗：输入站点 → 必要时由用户完成网页验证 → 登录并保存。
 *
 * ## 为什么抽成自带状态的独立组件
 *
 * 自带 `siteInput` / `busy` / `sponsors` / `sites` 四个状态 —— 它们只在这个弹窗的
 * 生命周期里有意义（关掉就该丢），宿主拿着它们除了往下传没有别的用途。
 * 宿主只管「开不开」与「成功后干什么」。
 */
export interface AddSiteDialogProps {
  open: boolean;
  /**
   * 当前 tab 的 app_type。
   *
   * 用来判断「这次 provision 拉到的分组有没有当前平台的」—— provision 一次探
   * 全部平台，报「已备好 N 个档位」时若拿全平台总数，用户面前那一屏可能还是空的。
   * 见 `reportProvision`。
   */
  appId: AppId;
  /** 关闭请求。 */
  onClose: () => void;
  /** 站点导入流程结束后调用，宿主据此 refresh。 */
  onAdded: () => void;
  /** 域名输入框的底纹词（来自 `relay_status.defaultSite`）。 */
  defaultSite: string;
  /**
   * 一个站都没有时那次（首启，或用户把站全删了之后再开）。
   *
   * **只改文案**：标题换成「选择服务站点」、说明补一句留空用默认 ——
   * 那时用户面对的是引导而不是「再加一个」。
   *
   * ⚠️ **不再连带「不给关」**（2026-08-04 改）。原来它同时堵掉取消按钮与
   * `onOpenChange`，理由是「一个站都没有时关掉它，用户就对着一个空面板」。
   * 那个理由站不住：面板上就有「添加站点」按钮，关掉之后随时能再进来 ——
   * 而堵死出口会让用户在还没决定用哪个站时被卡住。维护者明确要求所有弹窗可关闭。
   */
  isFirstRun?: boolean;
}

export function AddSiteDialog({
  open,
  appId,
  onClose,
  onAdded,
  defaultSite,
  isFirstRun = false,
}: AddSiteDialogProps) {
  const { t } = useTranslation();
  const [siteInput, setSiteInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [sponsors, setSponsors] = useState<Sponsor[]>([]);
  /**
   * 已添加的站点行（含未登录的占位行）。用来算「点这个推荐站会不会撞上已有账号」。
   *
   * 组件自己取而不是让宿主传 —— 宿主本来不关心「推荐站会不会撞已有账号」这件事，
   * 让它传等于把这个弹窗的内部判据摊到外面。
   */
  const [sites, setSites] = useState<SiteInfo[]>([]);

  // 推荐列表只在弹窗打开时取一次。读的是本地缓存（后端不发网络请求），所以不加 loading 态 ——
  // 为一次读文件转圈是把内部实现的分层暴露给用户。
  //
  // **拿不到就当没有推荐**：`listSponsors` 本身不会 reject（后端返空数组而不是 Err），
  // 这里的 catch 只兜「invoke 层面出错」这种真异常，同样静默 —— 首启屏少几个按钮
  // 不该变成一条用户看不懂的报错。
  useEffect(() => {
    if (!open) return;
    let alive = true;
    void relayApi
      .listSponsors()
      .then((list) => {
        if (alive) setSponsors(list);
      })
      .catch(() => {
        // 静默：没有推荐时 UI 与这个功能上线前完全一致（只有手动输入框）。
      });
    // 已添加的站点：**每次打开都重取**，因为期间用户可能在别处加过/删过。
    void relayApi
      .listSites()
      .then((list) => {
        if (alive) setSites(list);
      })
      .catch(() => {
        // 取不到就当没有已添加的站 ⇒ 不弹那句确认，退化成直接添加（旧行为）。
      });
    return () => {
      alive = false;
    };
  }, [open]);

  /**
   * 这个站现有几个**已登录的账号**（`accountLabel` 非空）。
   *
   * 只用来在卡片上显示「已有 N 个，再加一个」，不参与任何分支判断 ——
   * 曾经拿它当「要不要先弹确认」的判据，而维护者那行鑫旺**有 token 但
   * `account_id` 为空**（因此 `accountLabel` 是空串）⇒ 算成 0 ⇒ 该提示的没提示。
   * 现在点卡片一律走「探测 + 登录」，不再有需要这个数目做判据的分支。
   */
  const accountCountFor = (siteOrigin: string) =>
    sites.filter((s) => s.siteOrigin === siteOrigin && s.accountLabel).length;

  const importErrorMessage = (error: unknown) => {
    const typed =
      typeof error === "object" && error !== null
        ? (error as Partial<RelayImportError>)
        : undefined;
    if (typed?.kind === "unsupported_site") {
      return t("loongport.addSite.unsupportedSite");
    }
    if (typed?.kind === "protocol_conflict") {
      return t("loongport.addSite.protocolConflict");
    }
    if (typed?.kind === "transport") {
      return t("loongport.addSite.transportError");
    }

    const raw =
      typeof typed?.message === "string"
        ? typed.message
        : error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : "";
    const clean = raw.replace(/^(?:配置错误:\s*)+/, "").trim();
    return clean || t("loongport.addSite.importFailed");
  };

  /**
   * 导入站点。`site` 省略时使用输入框值；空串仍由后端回落到默认站点。
   *
   * 打开网页、等待用户验证的阶段完全不知道站点协议。后端只在同一个 WebView
   * 会话中识别成功后才接入对应登录适配，前端不再拆成 probe + login 两条命令。
   */
  const handleImport = async (site?: string) => {
    const target = site ?? siteInput;
    setBusy(true);
    try {
      const result = await relayApi.importSite(target);
      toast.success(
        t("loongport.addSite.connected", { name: result.siteName }),
      );
      setSiteInput("");

      if (result.loggedIn) {
        toast.success(t("loongport.session.connected"));
        // 登录成功后必须等分组和密钥落库再通知宿主刷新；否则新行会短暂显示
        // “没有可用分组”。provision 失败不撤销已保存的站点和登录态，刷新后
        // 用户仍可从新行上的“获取密钥”重试。
        try {
          reportProvision(t, await relayApi.provision(result.relayId), appId);
        } catch (e) {
          toast.error(String(e));
        }
      }

      // loggedIn=false 表示用户在登录完成前主动关窗。若协议已经识别，站点行仍已
      // 保存，因此照常刷新列表；没有凭据则不发 provision 请求。
      onAdded();
      onClose();
    } catch (e) {
      // 发现、网页验证后的协议识别或登录失败时保留输入与弹窗，方便用户修正后重试。
      toast.error(importErrorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setSiteInput("");
          onClose();
        }
      }}
    >
      <DialogContent className="max-w-md" zIndex="top">
        <DialogHeader>
          <DialogTitle>
            {isFirstRun
              ? t("loongport.addSite.firstRunTitle")
              : t("loongport.addSite.title")}
          </DialogTitle>
          <DialogDescription>
            {/* 域名走 `{{site}}` 插值而不是拼一个 `<code>` 元素在后面：**句末标点与
                语序必须由译文决定**。原来是「…默认的 {code}。」硬编码那个「。」，
                en 下会渲染成「…the default bestapi.store。」（中文句号），
                ja 的语序也要求域名在句中而不是句尾。 */}
            {isFirstRun
              ? t("loongport.addSite.firstRunBody", { site: defaultSite })
              : t("loongport.addSite.body")}
          </DialogDescription>
        </DialogHeader>
        {/* ⚠️ **内容区必须自己带 `px-6`**：`DialogHeader` / `DialogFooter` 各自带
            `px-6 py-5`（`ui/dialog.tsx:99`、`:113`），而它们之间的 children 是裸的 ——
            原来这里直接放 `<Input>`，于是输入框**贴着弹窗左右边缘**，和上下两块对不齐。

            `px-6 py-4` 是仓里内容区的主流取值（`ModelsDevPickerDialog.tsx:189`、
            `ModelsDevAutoSyncPanel.tsx:225`、`UnifiedSkillsPanel.tsx:648` 三处同形）。
            判据是 CLAUDE.md §一「和旧页面放一起看不出是两个人写的」。 */}
        <div className="flex flex-col gap-4 px-6 py-4">
          {/* 推荐中转站。**列表为空时整块不渲染** —— 首启那几秒还没拉到、没网、
              或维护者撤空了列表，三种情形都会空，那时 UI 与这个功能上线前一致
              （只有手动输入框），而不是显示一个空标题或「暂无推荐」。

              放在输入框**上方**：它是主路径，手动输入是退路。 */}
          {sponsors.length > 0 && (
            <div className="flex flex-col gap-2">
              <Label className="text-sm">
                {t("loongport.addSite.sponsorsLabel")}
              </Label>
              <div className="flex flex-col gap-1.5">
                {sponsors.map((s) => {
                  const accounts = accountCountFor(s.siteOrigin);
                  return (
                    <div key={s.siteOrigin} className="flex flex-col">
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void handleImport(s.siteOrigin)}
                        // 视觉抄上游的可选行样式（hover 变底色 + 圆角），
                        // 不新造一套 —— 判据是 CLAUDE.md §一「放一起看不出是两个人写的」。
                        className="flex flex-col items-start gap-0.5 rounded-md border border-border px-3 py-2 text-left transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        <span className="flex w-full items-center justify-between gap-2">
                          <span className="text-sm font-medium">
                            {s.displayName}
                          </span>
                          {/* 已有账号就说清有几个 + 点下去是**再加一个**。
                              少了这句，用户对着一个已经在用的站不知道点它会怎样。 */}
                          {accounts > 0 && (
                            <span className="shrink-0 text-xs text-muted-foreground">
                              {t("loongport.addSite.sponsorAddAnother", {
                                count: accounts,
                              })}
                            </span>
                          )}
                        </span>
                        {/* tagline 可能是空串（`#[serde(default)]`），空就不占一行。 */}
                        {s.tagline && (
                          <span className="text-xs text-muted-foreground">
                            {s.tagline}
                          </span>
                        )}
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          <div className="flex flex-col gap-2">
            <Label htmlFor="loongport-add-site" className="text-sm">
              {/* 有推荐时换成「或手动输入」—— 不然两块内容并列而下面那块没有交代
                  它与上面的关系。 */}
              {sponsors.length > 0
                ? t("loongport.addSite.inputLabelWithSponsors")
                : t("loongport.addSite.inputLabel")}
            </Label>
            <Input
              id="loongport-add-site"
              placeholder={defaultSite}
              value={siteInput}
              onChange={(e) => setSiteInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !busy) void handleImport();
              }}
              // 有推荐时不抢焦点 —— 那会让光标停在退路上而不是主路径上。
              autoFocus={sponsors.length === 0}
            />
            {/* 举一个**带 www. 和路径**的例子。
                后端的 `normalize_site_origin` 会把它们全剥掉（那是实测抓出的：
                用户从浏览器地址栏复制过来的多半长这样），但用户不知道 ——
                不写出来他会先手工删成裸域，而删错就连不上。 */}
            <p className="text-xs text-muted-foreground">
              {t("loongport.addSite.inputHint")}
            </p>
          </div>
        </div>
        <DialogFooter>
          {/* 取消按钮。与 `ConfirmDialog` / `SwitchTierConfirmDialog` 等 15 处同形
              （取消在左、主操作在右），文案复用上游的 `common.cancel`。

              **右上角的 X 是另一个出口**（`DialogContent` 现在默认渲染它）——
              两个并存是有意的：X 是「算了」的快捷手势，取消按钮是显式出口。
              本仓 `onInteractOutside` 仍 `preventDefault()`（不让点遮罩误关），
              所以这两个出口都不能再省。 */}
          <Button
            variant="outline"
            onClick={() => {
              setSiteInput("");
              onClose();
            }}
            disabled={busy}
          >
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void handleImport()} disabled={busy}>
            {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("common.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
