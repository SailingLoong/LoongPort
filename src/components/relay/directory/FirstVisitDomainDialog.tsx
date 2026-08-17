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
 * **暗纹即默认值**：输入框的 placeholder（当前 `bestapi.store`）同时是留空
 * 时的取值 —— 确认按钮常可点，不逼用户先敲字（维护者 2026-08-17 定）。
 * 占位符与默认值同源（都读 `loongport.firstSite.placeholder`），改一处两处动。
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
  /** 确认：参数是用户输入 trim 后的值，留空时回退暗纹默认值（恒非空）。 */
  onSubmit: (domain: string) => void;
}) {
  const { t } = useTranslation();
  const [domain, setDomain] = useState("");
  const placeholder = t("loongport.firstSite.placeholder");
  // 留空 = 用暗纹里的默认站（官方站），与按钮常可点的行为配套。
  const effective = domain.trim() || placeholder;

  const submit = () => onSubmit(effective);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onDismiss();
      }}
    >
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {t("loongport.firstSite.title")}
        </DialogTitle>
        <DialogDescription className="mt-1.5 text-sm leading-relaxed">
          {t("loongport.firstSite.body")}
        </DialogDescription>

        <Input
          autoFocus
          className="mt-4"
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          placeholder={placeholder}
          onKeyDown={(event) => {
            if (event.key === "Enter") submit();
          }}
        />

        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={onDismiss}>
            {t("loongport.firstSite.cancel")}
          </Button>
          <Button size="sm" data-testid="first-site-confirm" onClick={submit}>
            {t("loongport.firstSite.confirm")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
