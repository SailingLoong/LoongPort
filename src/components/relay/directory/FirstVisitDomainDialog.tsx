import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/**
 * 新人首启的「手填域名直达」弹窗。
 *
 * 广场的推荐列表是动态加载的（要等远端目录 + 探针），新人在空窗期里最拿得
 * 到的信息其实是**站长给的域名**。这个弹窗让他们跳过等待：输入域名 →
 * 确定 → 走与广场搜索框完全同一条 manual 导入链（`RelayDirectoryPage` 的
 * `authenticate(domain, …, "manual")`），直接进该站的注册 / 登录。列表照常
 * 在后台加载，关掉弹窗就回到广场逛。
 *
 * 一次性（每进程一次）：由 `RelaySection.reloadStatus` 的新人分支在跳广场
 * 时带 `firstVisit` 进来；用户关闭后本进程不再主动弹 —— 广场列表此后就是
 * 常驻入口，不需要再问一遍。
 */
export function FirstVisitDomainDialog({
  open,
  onDismiss,
  onSubmit,
}: {
  open: boolean;
  /** 关闭（取消 / esc / 点遮罩）：回广场逛列表，本进程不再主动弹。 */
  onDismiss: () => void;
  /** 确认：参数是用户输入 trim 后的非空串；导入流程由调用方执行。 */
  onSubmit: (domain: string) => void;
}) {
  const { t } = useTranslation();
  const [domain, setDomain] = useState("");
  const trimmed = domain.trim();

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onDismiss();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogTitle>{t("loongport.firstSite.title")}</DialogTitle>
        <DialogDescription>{t("loongport.firstSite.body")}</DialogDescription>
        <Input
          autoFocus
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          placeholder={t("loongport.firstSite.placeholder")}
          onKeyDown={(event) => {
            if (event.key === "Enter" && trimmed) onSubmit(trimmed);
          }}
        />
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onDismiss}>
            {t("loongport.firstSite.cancel")}
          </Button>
          <Button
            disabled={!trimmed}
            onClick={() => onSubmit(trimmed)}
            data-testid="first-site-confirm"
          >
            {t("loongport.firstSite.confirm")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
