import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Check,
  ExternalLink,
  KeyRound,
  Loader2,
  LogOut,
  RefreshCw,
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
  const [tiers, setTiers] = useState<TierInfo[]>([]);
  const [balance, setBalance] = useState<OperatorBalance | null>(null);

  const [siteInput, setSiteInput] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmSwitch, setConfirmSwitch] = useState<TierInfo | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await operatorApi.status();
      setStatus(s);
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
      await refresh();
    } catch (e) {
      // 探测失败不清输入框 —— 用户可能只是打错一个字母，让他改而不是重打。
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
        toast.success("登录成功，正在获取密钥…");
        await refresh();
        // 登录成功后直接把密钥备好 —— 需求要的是「用户无感」，不该再让他点一次。
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

  // ── 第一步：还没选站 ───────────────────────────────────────────
  if (!status.siteOrigin) {
    return (
      <Dialog open>
        <DialogContent className="max-w-md" zIndex="top">
          <DialogHeader>
            <DialogTitle>选择服务站点</DialogTitle>
            <DialogDescription>
              输入你要使用的中转站域名，留空则用默认的{" "}
              <code className="text-xs">{status.defaultSite}</code>。
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
  }

  return (
    <div className="space-y-6 p-6">
      {/* 站点信息 */}
      <div className="flex items-center justify-between border-b border-border/40 pb-4">
        <div>
          <div className="text-sm font-medium">{status.siteName}</div>
          <div className="text-xs text-muted-foreground">
            {status.siteOrigin}
          </div>
        </div>
        <div className="flex items-center gap-2">
          {balance && (
            <div className="flex items-center gap-1.5 text-sm text-muted-foreground">
              <Wallet className="h-4 w-4" />${balance.balance.toFixed(2)}
            </div>
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
            <h3 className="text-sm font-medium">选择档位</h3>
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
                if (status.chatgptInstalled) {
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
    </div>
  );
}
