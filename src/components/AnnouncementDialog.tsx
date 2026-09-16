import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { announcementsApi } from "@/lib/api/announcements";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/**
 * 远端公告弹窗（2026-09-16 预留）：数据来自签名远端配置，维护者改文案零发版。
 *
 * - 启动后查一次待展示公告，有就弹；窗口聚焦时重查（首启远端配置可能还没拉到，
 *   当次会话内补弹；拉不到/没有 = 静默）
 * - 任何关闭方式（点确认、Esc、点遮罩）都记「已确认」，不再重弹；要再弹必须
 *   远端换 id
 * - 多条公告排队逐条展示（一次一条，确认后下一条顶上）
 * - 弹窗形状照 SwitchTierConfirmDialog 基准（max-w-md、Header→Footer、
 *   主操作唯一主梯度按钮）
 */
export function AnnouncementDialog() {
  const { t } = useTranslation();
  const client = useQueryClient();
  const { data } = useQuery({
    queryKey: ["pendingAnnouncements"],
    queryFn: announcementsApi.getPending,
    refetchOnWindowFocus: true,
  });
  const current = data?.[0] ?? null;
  const close = () => {
    if (!current) return;
    void announcementsApi
      .acknowledge(current.id)
      .then(() =>
        client.invalidateQueries({ queryKey: ["pendingAnnouncements"] }),
      )
      .catch(() => undefined);
  };
  return (
    <Dialog
      open={current != null}
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{current?.title}</DialogTitle>
          <DialogDescription>
            <span className="whitespace-pre-line text-sm">{current?.body}</span>
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2">
          <Button onClick={close}>{t("common.confirm")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
