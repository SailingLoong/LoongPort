import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";

import { relayApi } from "@/lib/api/relay";

/**
 * 「导入站点配置」弹窗：贴站长给的配置链接或内容（URL / JSON / base64 均可，
 * 后端自动判别），同步到该站点的托管档位。
 *
 * 格式基准 = `SwitchTierConfirmDialog`（`max-w-md` + Header/Footer + 三按钮权重
 * 梯度）——这是仓内确认/输入类弹窗的统一形状，新弹窗从这里抄骨架，别自己拼
 * flex 排布（那条路长出来过一版「左边孤零零一个 ghost」的怪布局）。
 *
 * 前端只做收集与反馈——三形态判别、双重同源校验、deny-list 过滤全在后端
 * （`relay_apply_site_config`），错误消息也是后端给的用户向文案，这里原样 toast。
 *
 * 「恢复默认」与「应用」对称：一个把站长声明合进来，一个退回内置默认
 * （sk / 端点 / 当前模型保留，不是换钥匙）。
 */
export interface SiteConfigDialogProps {
  relayId: number;
  /** 行显示名（站名 / 账号）。弹窗描述里定位「这是哪个账号的配置」。 */
  relayLabel: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 应用 / 恢复成功后刷新列表（档位 settings 与标注都变了）。 */
  onApplied: () => void | Promise<void>;
}

export function SiteConfigDialog({
  relayId,
  relayLabel,
  open,
  onOpenChange,
  onApplied,
}: SiteConfigDialogProps) {
  const { t } = useTranslation();
  const [input, setInput] = useState("");
  const [applying, setApplying] = useState(false);
  const [resetting, setResetting] = useState(false);
  const busy = applying || resetting;

  const close = () => {
    if (busy) return;
    setInput("");
    onOpenChange(false);
  };

  const apply = async () => {
    setApplying(true);
    try {
      const summary = await relayApi.applySiteConfig(relayId, input);
      toast.success(
        t("loongport.siteConfig.appliedToast", {
          count: summary.applied.length,
        }),
      );
      await onApplied();
      setInput("");
      onOpenChange(false);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setApplying(false);
    }
  };

  const reset = async () => {
    setResetting(true);
    try {
      const summary = await relayApi.resetSiteConfig(relayId);
      toast.success(
        t("loongport.siteConfig.resetToast", { count: summary.applied.length }),
      );
      await onApplied();
      onOpenChange(false);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setResetting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => (next ? onOpenChange(true) : close())}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("loongport.siteConfig.title")}</DialogTitle>
          <DialogDescription>
            {t("loongport.siteConfig.description", { label: relayLabel })}
          </DialogDescription>
        </DialogHeader>
        <Textarea
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder={t("loongport.siteConfig.placeholder")}
          className="min-h-28 font-mono text-xs"
          spellCheck={false}
          disabled={busy}
        />
        {/* 三按钮，视觉权重从低到高（左 → 右），与 `SwitchTierConfirmDialog`
            同一梯度：**取消**（ghost）/ 恢复默认（outline，次要动作）/ 应用（主）。
            「恢复默认」不是退路也不是主流程——它是与应用对称的另一个方向，
            放中间档正合适。 */}
        <DialogFooter className="gap-2">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => void reset()}
          >
            {resetting ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              t("loongport.siteConfig.reset")
            )}
          </Button>
          <Button
            disabled={busy || input.trim().length === 0}
            onClick={() => void apply()}
          >
            {applying ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              t("loongport.siteConfig.apply")
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
