import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, ExternalLink, Star } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { GITHUB_REPO } from "@/config/constants";
import { copyText } from "@/lib/clipboard";
import { settingsApi, starRewardApi, type StarRewardOffer } from "@/lib/api";

/** 发放路径：文案差异全部来自它（gh 代点 / 检测到 / 未确认也发）。 */
type GrantedVia = "gh" | "detected" | "unverified";

type Phase =
  | { kind: "offer" }
  | { kind: "starring" }
  | { kind: "waiting" }
  | { kind: "verifying" }
  | { kind: "granted"; via: GrantedVia };

/**
 * 「点 Star 领注册礼」弹窗：新人引导事件与顶栏红点共用，状态机全在这里。
 *
 * 流程（设计定稿，见 i18n 文案）：
 * 确认 → 先试本机 gh 代点（没装/没登录返回 false）→ 通了直接发码；
 * 不通则开浏览器跳仓库，等用户点【我已点赞】→ 取一次新星数与基线比对 ——
 * 涨了说「已检测到」，没涨/取失败说「可能网络波动」，**两种都发码**
 * （防作弊有意放开：不要求 GitHub 登录态，校验只是文案差异）。
 *
 * 取消语义：全过程可退 —— 底部居中的取消按钮 + 右上角 X（esc 同 X），
 * 取消就是关窗，没有任何后续。发码后（granted）只剩 X：码已到手，
 * 没有可取消的东西。
 *
 * 奖励落地顺序是**先展示后持久化**：崩溃窗口里用户先看到码（`starRewardClaimed`
 * 没写成功 ⇒ 下次红点还在，最坏是多领一次，荣誉制下无害），反过来则会出现
 * 「标志写了、码没给」的死局。
 */
export function StarRewardDialog({
  offer,
  onClose,
  onClaimed,
}: {
  offer: StarRewardOffer;
  onClose: () => void;
  /** 码发放时刻回调（App 用它熄红点）；持久化在本组件里自己做。 */
  onClaimed: () => void;
}) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: "offer" });
  const [copied, setCopied] = useState(false);
  // 用户关窗后，在途的异步链（gh / 二次取数）不再推进 —— 检查点都看这个 ref。
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  // 只传数值：`$` 符号在各语言文案里（`${{amount}}`）——组件再拼一个 `$`
  // 会和文案里的叠成 `$$5`。
  const amount = String(offer.amountUsd);

  /** 发码：置 granted → 持久化 claimed → 通知 App → 开注册窗（码自动预填）。 */
  const grant = (via: GrantedVia) => {
    if (!aliveRef.current) return;
    setPhase({ kind: "granted", via });
    void (async () => {
      try {
        // claimed 是后端专有事实：窄命令 RMW 落库。不走全量 save —— 那条路
        // 对后端专有字段取现有值（2026-08-16 起，旧快照回写曾把它抹掉）。
        await starRewardApi.markClaimed();
      } catch {
        // 持久化失败可接受：码已展示给用户，最坏下次启动红点再现（荣誉制）。
      }
      if (!aliveRef.current) return;
      onClaimed();
      // 注册窗打开失败也不回滚：码可见可复制，用户可以自己去 bestapi 注册。
      starRewardApi.openRegisterWindow(offer.promoCode).catch((error) => {
        console.warn("[star-reward] 注册窗未能打开", error);
      });
    })();
  };

  const handleConfirm = async () => {
    setPhase({ kind: "starring" });
    // gh 通了就一步到位（PUT 幂等，早点过星的老用户也算成功）。
    try {
      if (await starRewardApi.starViaGh()) {
        grant("gh");
        return;
      }
    } catch {
      // 命令本身报错与返回 false 同义：这条路不通，走浏览器。
    }
    if (!aliveRef.current) return;
    // 打不开也继续等 —— 浏览器可能已经开着，用户自己去点也一样。
    await settingsApi.openExternal(GITHUB_REPO).catch(() => {});
    if (!aliveRef.current) return;
    setPhase({ kind: "waiting" });
  };

  const handleAlreadyStarred = async () => {
    setPhase({ kind: "verifying" });
    try {
      const current = await starRewardApi.starCount();
      if (!aliveRef.current) return;
      grant(current > offer.baselineStars ? "detected" : "unverified");
    } catch {
      if (!aliveRef.current) return;
      // 取失败与「没涨」同一口径：网络波动，码照发。
      grant("unverified");
    }
  };

  const handleCopy = async () => {
    try {
      await copyText(offer.promoCode);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // 复制失败不打扰：码本身一直可见，用户可以手动选中。
    }
  };

  // 关窗（X / esc / 取消按钮）在所有阶段都是「取消」：在途的 gh / 二次取数
  // 由 aliveRef 拦下，不会再发码、不会再开注册窗。
  const handleOpenChange = () => {
    onClose();
  };

  const busy = phase.kind === "starring" || phase.kind === "verifying";

  return (
    <Dialog open onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        {phase.kind === "granted" ? (
          <>
            <DialogTitle className="text-base font-semibold">
              {t("loongport.star.grantedTitle")}
            </DialogTitle>
            <DialogDescription className="mt-1.5 text-sm leading-relaxed">
              {t(
                phase.via === "gh"
                  ? "loongport.star.grantedViaGh"
                  : phase.via === "detected"
                    ? "loongport.star.grantedDetected"
                    : "loongport.star.grantedUnverified",
                { amount },
              )}
            </DialogDescription>

            <div className="mt-4 flex items-center justify-between gap-3 rounded-lg bg-muted/40 p-3.5">
              <div className="min-w-0">
                <p className="text-xs text-muted-foreground">
                  {t("loongport.star.codeLabel")}
                </p>
                <p className="truncate font-mono text-lg font-semibold tracking-wide">
                  {offer.promoCode}
                </p>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void handleCopy()}
                className="shrink-0"
              >
                {copied ? (
                  <Check className="h-3.5 w-3.5" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {copied ? t("loongport.star.copied") : t("loongport.star.copy")}
              </Button>
            </div>

            <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
              {t("loongport.star.registerNote")}
            </p>
          </>
        ) : phase.kind === "offer" ? (
          <>
            <DialogTitle className="text-base font-semibold">
              {t("loongport.star.dialogTitle", { amount })}
            </DialogTitle>
            <DialogDescription className="mt-1.5 text-sm leading-relaxed">
              {t("loongport.star.dialogBody", { amount })}
            </DialogDescription>

            {/* 底部居中：主行动 + 取消并排，都在正中间（设计要求）。 */}
            <div className="mt-5 flex justify-center gap-2">
              <Button size="sm" onClick={() => void handleConfirm()}>
                <Star className="h-3.5 w-3.5" />
                {t("loongport.star.confirm")}
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                {t("loongport.star.cancel")}
              </Button>
            </div>
          </>
        ) : (
          <>
            <DialogTitle className="text-base font-semibold">
              {t("loongport.star.waitTitle")}
            </DialogTitle>
            <DialogDescription className="mt-1.5 text-sm leading-relaxed">
              {t("loongport.star.waitBody")}
            </DialogDescription>

            <div className="mt-5 flex justify-center gap-2">
              <Button
                size="sm"
                disabled={busy}
                onClick={() => void handleAlreadyStarred()}
              >
                {busy
                  ? t("loongport.star.checking")
                  : t("loongport.star.waitAction")}
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                {t("loongport.star.cancel")}
              </Button>
            </div>

            <button
              type="button"
              className="mx-auto mt-3 flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
              onClick={() => void settingsApi.openExternal(GITHUB_REPO)}
            >
              <ExternalLink className="h-3 w-3" />
              {t("loongport.star.reopenRepo")}
            </button>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
