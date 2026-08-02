import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Check,
  ExternalLink,
  KeyRound,
  Loader2,
  LogOut,
  Plus,
  RefreshCw,
  Trash2,
  Wallet,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { operatorApi } from "@/lib/api";
import type {
  OperatorBalance,
  OperatorStatus,
  SiteInfo,
  TierInfo,
} from "@/lib/api/operator";

/**
 * LoongPort 主面板：一屏走完「选站 → 登录 → 备好密钥 → 选档位用起来」。
 *
 * 状态机只有四态，由后端的 `operator_status` 决定，前端不自己记：
 *
 * ```text
 * siteOrigin == null            → 域名输入弹窗
 * !loggedIn                     → 「去登录」
 * loggedIn && tierCount == 0    → 「获取密钥」
 * tierCount > 0                 → 档位列表（可切换）
 * ```
 *
 * 为什么状态在后端：刷新页面、重开 app、切到别的窗口再回来，都不该丢进度。前端记状态就得
 * 处理这些同步问题，而这些事实本来就都在数据库里。
 */
export function OperatorPanel() {
  const [status, setStatus] = useState<OperatorStatus | null>(null);
  const [sites, setSites] = useState<SiteInfo[]>([]);
  const [tiers, setTiers] = useState<TierInfo[]>([]);
  const [balance, setBalance] = useState<OperatorBalance | null>(null);

  const [siteInput, setSiteInput] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmSwitch, setConfirmSwitch] = useState<TierInfo | null>(null);
  // 「添加站点」弹窗。首启那次不用它 —— 那时整屏就是输入框（没有站点可看）。
  const [addingSite, setAddingSite] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const s = await operatorApi.status();
      setStatus(s);
      setSites(await operatorApi.listSites());
      if (s.tierCount > 0) {
        setTiers(await operatorApi.listTiers());
      } else {
        setTiers([]);
      }
      // 余额是附加信息，拿不到不该让整屏报错（可能是运营商关了用户面板）。
      if (s.loggedIn) {
        operatorApi
          .balance()
          .then(setBalance)
          .catch(() => setBalance(null));
      }
    } catch (e) {
      toast.error(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 启动时探一次凭据是不是真的还活着。
  //
  // status 的 loggedIn 只看本地记的过期时间 —— 凭据在网页端被撤销、账号被禁用时它仍是 true，
  // 用户会看到界面一切正常、点任何操作才报错。这一次探活把那种状态提前暴露出来。
  //
  // 有意不 await 在 refresh 里：首屏该立刻渲染，不该卡在网络请求上。
  useEffect(() => {
    let cancelled = false;
    operatorApi
      .checkSession()
      .then((alive) => {
        // false 表示后端已经清掉失效凭据了，重新读一次状态就会回到登录入口。
        if (!alive && !cancelled) {
          toast.info("登录已失效，请重新登录");
          void refresh();
        }
      })
      // 探活自身失败（网络不通）不打扰用户 —— 凭据没被清掉，操作时会自然报错。
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  const handleProbe = async () => {
    setBusy("probe");
    try {
      const r = await operatorApi.probeSite(siteInput);
      toast.success(`已连上 ${r.siteName}`);
      // 探测成功即成为当前站。清掉输入、关掉添加弹窗。
      setSiteInput("");
      setAddingSite(false);
      await refresh();
    } catch (e) {
      // 探测失败**不清输入框、不关弹窗** —— 用户可能只是打错一个字母，让他改而不是重打。
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleSwitchSite = async (id: number) => {
    setBusy(`site:${id}`);
    try {
      await operatorApi.switchSite(id);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleRemoveSite = async (site: SiteInfo) => {
    setBusy(`site:${site.id}`);
    try {
      await operatorApi.removeSite(site.id);
      toast.success(`已移除 ${site.label}`);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleLogin = async () => {
    setBusy("login");
    try {
      const ok = await operatorApi.login();
      if (ok) {
        // 登录窗**不会自动关闭**（它已经跳到 dashboard，用户可能要在那儿充值或看用量）。
        // 所以这条提示要说清窗口还开着、可以自己关。
        toast.success("已连接，正在获取密钥…（登录窗口可以自己关掉）");
        await refresh();
        // 直接把密钥备好 —— 需求要的是「用户无感」，不该再让他点一次。
        await handleProvision();
      }
      // ok === false 是用户自己关了窗口，不出提示（他知道自己干了什么）。
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleProvision = async () => {
    setBusy("provision");
    try {
      const r = await operatorApi.provision();
      const created = r.keysCreated > 0 ? `，新建 ${r.keysCreated} 把密钥` : "";
      toast.success(`已备好 ${r.tiers.length} 个档位${created}`);
      // 部分失败如实说出来，但不阻断 —— 成功的那些能用。
      for (const f of r.failures) {
        toast.warning(`${f.groupName}：${f.reason}`);
      }
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const doSwitch = async (tier: TierInfo, quitChatgpt: boolean) => {
    setConfirmSwitch(null);
    setBusy(`switch:${tier.providerId}`);
    try {
      const r = await operatorApi.switchTier(tier.providerId, quitChatgpt);
      const tail = r.chatgptRelaunched
        ? "，已重新打开 ChatGPT"
        : r.chatgptWasRunning
          ? "，请手动重启 ChatGPT"
          : "";
      toast.success(`已切换到 ${r.providerName}${tail}`);
      for (const w of r.warnings) toast.warning(w);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleLogout = async () => {
    try {
      await operatorApi.logout();
      setBalance(null);
      await refresh();
      toast.success("已退出登录");
    } catch (e) {
      toast.error(String(e));
    }
  };

  if (!status) {
    return (
      <div className="flex items-center justify-center p-12 text-muted-foreground">
        <Loader2 className="h-5 w-5 animate-spin" />
      </div>
    );
  }

  // 域名输入弹窗。首启（一个站都没有）与「加一个站」共用同一份 —— 差别只在标题与能不能关。
  const siteDialog = (isFirstRun: boolean) => (
    <Dialog
      open
      onOpenChange={(open) => {
        // 首启那次不给关：一个站都没有的话，关掉它用户就对着空面板了。
        if (!open && !isFirstRun) {
          setAddingSite(false);
          setSiteInput("");
        }
      }}
    >
      <DialogContent className="max-w-md" zIndex="top">
        <DialogHeader>
          <DialogTitle>
            {isFirstRun ? "选择服务站点" : "添加中转站"}
          </DialogTitle>
          <DialogDescription>
            {isFirstRun ? (
              <>
                输入你要使用的中转站域名，留空则用默认的{" "}
                <code className="text-xs">{status.defaultSite}</code>。
              </>
            ) : (
              <>
                输入另一个中转站的域名。同一个站可以挂多个账号 ——
                登录不同账号即可， 重复的会自动合并。
              </>
            )}
          </DialogDescription>
        </DialogHeader>
        <Input
          placeholder={status.defaultSite}
          value={siteInput}
          onChange={(e) => setSiteInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !busy) void handleProbe();
          }}
          autoFocus
        />
        <DialogFooter>
          <Button onClick={handleProbe} disabled={busy === "probe"}>
            {busy === "probe" && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            确定
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );

  // ── 第一步：还没选站 ───────────────────────────────────────────
  if (!status.siteOrigin) {
    return siteDialog(true);
  }

  return (
    <div className="space-y-6 p-6">
      {/* 站点切换器 + 添加入口 */}
      <div className="flex items-start justify-between gap-3 border-b border-border/40 pb-4">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
          {sites.map((site) => (
            <button
              key={site.id}
              onClick={() => {
                if (!site.isCurrent) void handleSwitchSite(site.id);
              }}
              disabled={site.isCurrent || busy === `site:${site.id}`}
              className={`group inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-sm transition-colors ${
                site.isCurrent
                  ? "border-blue-500/50 bg-blue-500/5 font-medium"
                  : "border-border/60 text-muted-foreground hover:bg-muted/50 hover:text-foreground"
              }`}
              title={site.siteOrigin}
            >
              {busy === `site:${site.id}` ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                // 未登录的站点给个提示点 —— 否则用户不知道为什么切过去没有档位。
                !site.loggedIn && (
                  <span
                    className="h-1.5 w-1.5 rounded-full bg-amber-500"
                    title="这个站还没登录"
                  />
                )
              )}
              <span className="truncate">{site.label}</span>
              {/* 删除按钮只在非当前站上出现：删当前站会连带把档位列表抽掉，
                  用户更可能是想先切走再删。 */}
              {!site.isCurrent && (
                <span
                  role="button"
                  tabIndex={-1}
                  onClick={(e) => {
                    e.stopPropagation();
                    void handleRemoveSite(site);
                  }}
                  className="opacity-0 transition-opacity hover:text-destructive group-hover:opacity-60"
                  title="移除这个站点"
                >
                  <Trash2 className="h-3 w-3" />
                </span>
              )}
            </button>
          ))}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setAddingSite(true)}
            title="添加另一个中转站"
          >
            <Plus className="h-3.5 w-3.5" />
          </Button>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {balance && (
            <div className="flex items-center gap-1.5 text-sm text-muted-foreground">
              <Wallet className="h-4 w-4" />${balance.balance.toFixed(2)}
            </div>
          )}
          {/* 常驻的「回运营商网页」入口。
              登录窗关掉之后，充值 / 看用量 / 查渠道状态就没有别的路了 —— 这些都在运营商
              的网页上，我们没做也不该做。用普通 <a target="_blank">：Tauri 的 opener 插件
              会接管它送到系统浏览器（与仓里既有那几处外链同一个写法）。 */}
          {status.loggedIn && (
            <a
              href={status.siteOrigin}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
              title="在浏览器里打开，可以充值、看用量"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              网页端
            </a>
          )}
          {status.loggedIn && (
            <Button variant="ghost" size="sm" onClick={handleLogout}>
              <LogOut className="mr-1.5 h-3.5 w-3.5" />
              退出
            </Button>
          )}
        </div>
      </div>

      {/* ── 第二步：未登录 ── */}
      {!status.loggedIn && (
        <div className="flex flex-col items-center gap-3 py-8">
          <p className="text-sm text-muted-foreground">
            还没登录 {status.siteName}
          </p>
          <Button onClick={handleLogin} disabled={busy === "login"}>
            {busy === "login" ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <ExternalLink className="mr-2 h-4 w-4" />
            )}
            登录 / 注册
          </Button>
          <p className="text-xs text-muted-foreground">
            会打开一个窗口加载官方登录页，没有账号可以在那里注册
          </p>
        </div>
      )}

      {/* ── 第三步：已登录但还没备密钥 ── */}
      {status.loggedIn && tiers.length === 0 && (
        <div className="flex flex-col items-center gap-3 py-8">
          <p className="text-sm text-muted-foreground">还没有可用的档位</p>
          <Button onClick={handleProvision} disabled={busy === "provision"}>
            {busy === "provision" ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <KeyRound className="mr-2 h-4 w-4" />
            )}
            获取密钥
          </Button>
        </div>
      )}

      {/* ── 第四步：档位列表 ── */}
      {tiers.length > 0 && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">
              选择档位
              {/* 同一个站可以挂多个账号，得说清这些档位属于哪个账号。 */}
              {status.accountLabel && (
                <span className="ml-2 text-xs font-normal text-muted-foreground">
                  {status.accountLabel}
                </span>
              )}
            </h3>
            <Button
              variant="ghost"
              size="sm"
              onClick={handleProvision}
              disabled={busy === "provision"}
            >
              <RefreshCw
                className={`mr-1.5 h-3.5 w-3.5 ${
                  busy === "provision" ? "animate-spin" : ""
                }`}
              />
              刷新
            </Button>
          </div>
          {tiers.map((tier) => (
            <button
              key={tier.providerId}
              onClick={() => {
                if (tier.isCurrent) return;
                // ChatGPT 没装就不必问「要不要关它」，直接切。
                if (status.chatgptNeedsAttention) {
                  setConfirmSwitch(tier);
                } else {
                  void doSwitch(tier, false);
                }
              }}
              disabled={tier.isCurrent || busy === `switch:${tier.providerId}`}
              className={`flex w-full items-center justify-between rounded-lg border p-3 text-left transition-colors ${
                tier.isCurrent
                  ? "border-blue-500/50 bg-blue-500/5"
                  : "border-border/60 hover:bg-muted/50"
              }`}
            >
              <div>
                <div className="text-sm font-medium">{tier.displayName}</div>
                {/* rateMultiplier 为 null 时什么都不显示 —— 编一个 0 会让用户以为是免费的 */}
                {tier.rateMultiplier != null && (
                  <div className="text-xs text-muted-foreground">
                    倍率 {tier.rateMultiplier}
                  </div>
                )}
              </div>
              {busy === `switch:${tier.providerId}` ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : tier.isCurrent ? (
                <Check className="h-4 w-4 text-blue-500" />
              ) : null}
            </button>
          ))}
        </div>
      )}

      {/* 切换前的确认：要退出 ChatGPT */}
      <Dialog
        open={confirmSwitch != null}
        onOpenChange={(open) => {
          if (!open) setConfirmSwitch(null);
        }}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>切换到 {confirmSwitch?.displayName}</DialogTitle>
            <DialogDescription>
              ChatGPT 启动时才读取配置，所以要先退出它、切换完再打开。
              <br />
              如果它有进行中的对话，会弹出确认框 —— 在那里点取消的话，
              本次切换会中止、配置不会改动。
              <br />
              {/* 「自动退出」目前只有 macOS 实现。其它平台点这个按钮也不会报错：
                  配置照样切好，只是会提示用户自己重启 ChatGPT。文案要说清这件事，
                  不然 Windows 用户点了「退出并切换」发现 ChatGPT 还开着，会以为坏了。 */}
              <span className="text-xs">
                部分系统上无法代为退出，那时配置仍会切好，只需你手动重启
                ChatGPT。
              </span>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2">
            <Button
              variant="outline"
              onClick={() =>
                confirmSwitch && void doSwitch(confirmSwitch, false)
              }
            >
              只切换，我自己重启
            </Button>
            <Button
              onClick={() =>
                confirmSwitch && void doSwitch(confirmSwitch, true)
              }
            >
              退出并切换
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 「添加中转站」弹窗 —— 与首启那个共用同一份，只是可以关掉。 */}
      {addingSite && siteDialog(false)}
    </div>
  );
}
