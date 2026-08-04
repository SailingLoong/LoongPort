import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
// 复用上游已有的平台检测，别新写一份（CLAUDE.md §一）。
//
// 它靠 UA 判断 —— 在**主窗口**里是可靠的：本仓只给登录窗与充值窗设过自定义 UA
// （`operator::login::WEBVIEW_USER_AGENT`），主窗口用的是系统默认 UA。
import { isWindows } from "@/lib/platform";
/**
 * 切换档位前的确认框：要不要替用户退出 ChatGPT。
 *
 * 抽成独立组件是因为**它现在有两个宿主**（`OperatorSection` 的托管档位切换，
 * 以及 provider 页那条通用切换）—— 各写一份就迟早分叉，而这段文案里有三条来自实测的
 * 说明，分叉后其中一份会变成错的。
 *
 * ## 为什么吃「显示名」而不是 `TierInfo`
 *
 * 原来它绑死 `tier: TierInfo | null`，只服务 LoongPort 的托管档位。但**切 cc-switch
 * 自带的 codex 供应商有完全一样的问题** —— ChatGPT 桌面版自带 codex 核心、与命令行
 * codex 共用同一个 `~/.codex`，它在跑的时候切任何 codex 供应商都是「启动时读的旧配置
 * 还在生效，而且它退出时会回写 config.toml」。那条路手上是 `Provider` 不是 `TierInfo`，
 * 而弹窗真正需要的只有一个显示名 —— 绑死类型纯属偶然。
 */
export interface SwitchTierConfirmDialogProps {
  /** 要切到的目标显示名；`null` = 不显示弹窗。 */
  targetName: string | null;
  onCancel: () => void;
  /** `quitChatgpt` = 用户选了「退出并切换」还是「只切换，我自己重启」。 */
  onSwitch: (quitChatgpt: boolean) => void;
}

export function SwitchTierConfirmDialog({
  targetName,
  onCancel,
  onSwitch,
}: SwitchTierConfirmDialogProps) {
  const { t } = useTranslation();
  // 在渲染期算而不是 useState/useEffect：平台在一次会话里不会变，
  // 上钩子只会多一次渲染。
  const onWindows = isWindows();

  return (
    <Dialog
      open={targetName != null}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {t("loongport.quitConfirm.title", { name: targetName ?? "" })}
          </DialogTitle>
          <DialogDescription>
            {t("loongport.quitConfirm.body")}
            <br />
            {/* ⚠️ **两个平台的退出方式不同，文案必须跟着分** —— 这条弹窗是
                「强制关闭」的知情同意书，说错了就是骗用户签字：

                - macOS：AppleScript `quit`，走 app 自己的退出流程。它有进行中的对话时
                  会弹自己的确认框，用户在那里点取消则本次切换中止 ⇒ 说的是 declineNote。
                - Windows：没有任何优雅手段（`WM_CLOSE` 被 minimize-to-tray 吃掉、
                  官方无 reload 接口、当不了父进程 —— 逐条实测见 `chatgpt_app` 模块文档），
                  只能 `taskkill /F` **强制关闭**，不给 app 任何拒绝或保存的机会
                  ⇒ 必须明说「会强制关闭、未保存内容可能丢失」。

                用 `declineNote` 那句去糊 Windows 是错的：那句承诺「会弹确认框、可以取消」，
                而那边压根不会弹 —— 用户以为还有一道保险，实际点下去就直接杀了。 */}
            {onWindows
              ? t("loongport.quitConfirm.forceKillWarning")
              : t("loongport.quitConfirm.declineNote")}
            <br />
            {/* 「代为退出」失败时（macOS 权限被拒 / Windows 杀不掉）配置照样切好，
                只是要用户自己重启。不说清这件事，用户点了「退出并切换」发现
                ChatGPT 还开着会以为坏了。 */}
            <span className="text-xs">
              {t("loongport.quitConfirm.platformNote")}
            </span>
          </DialogDescription>
        </DialogHeader>
        {/* 三个按钮，视觉权重从低到高（左 → 右）：**取消** / 只切换 / 退出并切换。

            ⚠️ **「取消」不可省** —— 原来只有两个按钮，两个都会执行切换，
            用户改主意了只剩 Esc 或点遮罩。对「会强制关闭 ChatGPT」这种不可逆动作，
            退出路径必须是个看得见的按钮：藏在快捷键里等于没有。

            `variant="ghost"` 而不是 `outline`：让它比「只切换」更轻，
            于是三个按钮的权重一眼可辨，而不是两个 outline 挤在一起分不清哪个是退路。 */}
        <DialogFooter className="gap-2">
          <Button variant="ghost" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button variant="outline" onClick={() => onSwitch(false)}>
            {t("loongport.quitConfirm.switchOnly")}
          </Button>
          <Button onClick={() => onSwitch(true)}>
            {t("loongport.quitConfirm.quitAndSwitch")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
