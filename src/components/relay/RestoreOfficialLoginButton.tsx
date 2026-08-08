import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, RotateCcw } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { relayApi } from "@/lib/api";
import { isWindows } from "@/lib/platform";

/**
 * 「切回 ChatGPT 官方登录」。
 *
 * ## 为什么这个按钮不可替代
 *
 * LoongPort 把 codex 配成 **provider auth 模式**（`experimental_bearer_token`
 * 写在 `config.toml` 里），鉴权压根不看 `auth.json` ⇒ **用户在 ChatGPT 里点
 * 「注销」没有任何反应**，请求照样带着中转站的 sk 打出去。
 *
 * 所以「回到官方登录」只能由我们做，而且必须原子地做四件事（全在后端）：
 * 退 ChatGPT → 备份 `auth.json` → 切到 `codex-official` → 删 `auth.json`。
 * 用户在 ChatGPT 的退出确认框里点取消 ⇒ 整个命令 reject 且**一个文件都没碰**。
 *
 * ## 为什么在设置页而不在 provider 页（2026-08-04 搬过来）
 *
 * 它是**全局单例**动作（只影响 codex 那一份 `auth.json`），与「哪个中转站」
 * 无关 —— 摆在多站并列的 provider 页里会让人以为它作用于某一行。
 *
 * 原来它在 `OperatorPanel`（LoongPort 独立页）底部。那个页面已删：它的其余能力
 * （加站 / 登录 / 获取密钥 / 余额 / 删除）provider 页都有且更准确，而
 * 站点切换器与登出是旧设计的遗留（见那次删除的 commit）。这个按钮是唯一
 * 需要另找去处的。
 *
 * 放在 auth tab 的「ChatGPT (Codex OAuth)」那一节下 —— 语义正好对口：
 * 那一节管的就是 ChatGPT 账号。
 *
 * ## 前端只做三件事
 *
 * 确认、播报、刷新。**顺序约束是后端的不变量**，前端不该也不能重排。
 * 备份路径要显示出来：那里面是 OAuth refresh token，用户手滑点了确认时
 * 得知道去哪儿捞。
 */
export function RestoreOfficialLoginButton() {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const handleRestore = async () => {
    setConfirming(false);
    setBusy(true);
    try {
      const r = await relayApi.restoreOfficialLogin();
      toast.success(
        r.backupPath
          ? t("loongport.official.doneWithBackup", { path: r.backupPath })
          : t("loongport.official.done"),
      );
      for (const w of r.warnings) toast.warning(w);
    } catch (e) {
      // 用户在 ChatGPT 的退出确认框点了取消也走这里 —— 那条错误文案已说明
      // 「配置未改动」，如实转达就对了。
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mt-4 flex flex-col items-start gap-1.5 border-t border-border/40 pt-4">
      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground hover:text-foreground"
        onClick={() => setConfirming(true)}
        disabled={busy}
      >
        {busy ? (
          <Loader2 className="mr-1.5 h-4 w-4 animate-spin" />
        ) : (
          <RotateCcw className="mr-1.5 h-4 w-4" />
        )}
        {t("loongport.official.button")}
      </Button>
      <p className="text-xs text-muted-foreground">
        {t("loongport.official.hint")}
      </p>

      {/* 切回官方登录前的确认 —— 它会删掉用户的 codex 登录态（删前自动备份）。
          ⚠️ 这条路与切档位一样走 `chatgpt_app::around`，**Windows 上是强制关闭**
          （那边没有优雅手段，见 `chatgpt_app` 模块文档）⇒ 必须一并给出强杀警告，
          否则同一个动作在两个入口给的知情程度不一致。 */}
      <ConfirmDialog
        isOpen={confirming}
        title={t("loongport.official.confirmTitle")}
        message={
          isWindows()
            ? `${t("loongport.official.confirmMessage")}\n\n${t("loongport.quitConfirm.forceKillWarning")}`
            : t("loongport.official.confirmMessage")
        }
        confirmText={t("loongport.official.confirmButton")}
        onConfirm={() => void handleRestore()}
        onCancel={() => setConfirming(false)}
      />
    </div>
  );
}
