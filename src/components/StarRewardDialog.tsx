import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Star } from "lucide-react";

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

/**
 * 「点 Star 领注册礼」弹窗：新人引导事件与顶栏红点共用，状态机全在这里。
 *
 * 流程（荣誉制）：确认 → 打开默认浏览器跳仓库页 + **当场发码**。不做任何
 * Star 校验 —— 校验（gh 代点 / 前后星数比对）都要打 GitHub API，国内网络下
 * 是长达 20s 的无反馈等待，用户感知是「卡死」；点不点星靠用户自觉。
 *
 * 取消语义：确认前底部居中的取消按钮 + 右上角 X（esc 同 X）都能退，取消就是
 * 关窗，没有任何后续。发码后（granted）只剩 X：码已到手，没有可取消的东西。
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
  /** 发码时刻回调（App 用它熄红点）；持久化在本组件里自己做。 */
  onClaimed: () => void;
}) {
  const { t } = useTranslation();
  const [granted, setGranted] = useState(false);
  const [copied, setCopied] = useState(false);

  // 只传数值：`$` 符号在各语言文案里（`${{amount}}`）——组件再拼一个 `$`
  // 会和文案里的叠成 `$$5`。
  const amount = String(offer.amountUsd);

  /** 发码：置 granted → 持久化 claimed → 开注册窗（码自动预填）。 */
  const grant = () => {
    setGranted(true);
    void (async () => {
      try {
        // claimed 是后端专有事实：窄命令 RMW 落库。不走全量 save —— 那条路
        // 对后端专有字段取现有值（2026-08-16 起，旧快照回写曾把它抹掉）。
        await starRewardApi.markClaimed();
      } catch {
        // 持久化失败可接受：码已展示给用户，最坏下次启动红点再现（荣誉制）。
      }
      // 注册窗打开失败也不回滚：码可见可复制，用户可以自己去 bestapi 注册。
      starRewardApi.openRegisterWindow(offer.promoCode).catch((error) => {
        console.warn("[star-reward] 注册窗未能打开", error);
      });
    })();
  };

  const handleConfirm = () => {
    // 红点在**点「领取」这一刻**熄灭（2026-08-17 维护者定）：之后领没领成
    // 功都不再亮 —— 用户已应征，不再用红点追着提醒。取消 / 叉号不走这里，
    // 红点保留为「稍后再说」入口。durable 的熄灭靠发码时的 markClaimed；
    // 这里只是会话内立即熄灭。
    onClaimed();
    // 开默认浏览器（系统 handler 自带焦点切换）。打不开也照样发码 ——
    // 用户仍可自己去仓库页点星。
    settingsApi.openExternal(GITHUB_REPO).catch(() => {});
    grant();
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

  return (
    <Dialog open onOpenChange={onClose}>
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        {granted ? (
          <>
            <DialogTitle className="text-base font-semibold">
              {t("loongport.star.grantedTitle")}
            </DialogTitle>
            <DialogDescription className="mt-1.5 text-sm leading-relaxed">
              {t("loongport.star.grantedBody")}
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
        ) : (
          <>
            <DialogTitle className="text-base font-semibold">
              {t("loongport.star.dialogTitle", { amount })}
            </DialogTitle>
            <DialogDescription className="mt-1.5 text-sm leading-relaxed">
              {t("loongport.star.dialogBody", { amount })}
            </DialogDescription>

            {/* 底部居中：主行动 + 取消并排，都在正中间（设计要求）。 */}
            <div className="mt-5 flex justify-center gap-2">
              <Button size="sm" onClick={handleConfirm}>
                <Star className="h-3.5 w-3.5" />
                {t("loongport.star.confirm")}
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                {t("loongport.star.cancel")}
              </Button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
