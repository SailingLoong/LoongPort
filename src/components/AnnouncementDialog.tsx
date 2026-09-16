import { useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { announcementsApi } from "@/lib/api/announcements";
import { settingsApi } from "@/lib/api";
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
 * - 启动后查一次待展示公告；窗口聚焦时重查（首启远端配置可能还没拉到，当次会话
 *   内补弹；拉不到/没有 = 静默）
 * - **每次启动最多弹一条**（2026-09-16 用户定调）：确认或关闭一条后本次会话
 *   不再弹，剩余的等下次启动轮到，不连环轰炸
 * - 任何关闭方式（点确认、Esc、点遮罩）都记「已确认」不再重弹；要再弹远端换 id
 * - 正文支持 markdown 子集（加粗/列表/链接）。**不渲染原始 HTML**——
 *   react-markdown 默认剥离标签，签名内容也不开脚本执行的口子；链接强制
 *   https 且交系统浏览器打开（`openExternal`）；单换行按公告习惯渲染为换行
 *   （两空格硬换行预处理，免得维护者被 markdown 段落规则坑）
 * - 弹窗形状照 SwitchTierConfirmDialog 基准（max-w-md、Header→Footer、
 *   主操作唯一主梯度按钮）
 */
export function AnnouncementDialog() {
  const { t } = useTranslation();
  const client = useQueryClient();
  // 会话闩：本次启动关闭过一条就不再弹；剩余公告下次启动轮到。
  const closedThisSession = useRef(false);
  const { data } = useQuery({
    queryKey: ["pendingAnnouncements"],
    queryFn: announcementsApi.getPending,
    refetchOnWindowFocus: true,
  });
  const current = closedThisSession.current ? null : (data?.[0] ?? null);
  const close = () => {
    const id = current?.id;
    closedThisSession.current = true;
    if (!id) return;
    void announcementsApi
      .acknowledge(id)
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
            <span className="text-sm">
              <ReactMarkdown
                components={{
                  a: ({ href, children }) =>
                    href?.startsWith("https://") ? (
                      <a
                        href={href}
                        rel="noreferrer"
                        onClick={(event) => {
                          // 应用内不开网页：链接一律交系统浏览器。
                          event.preventDefault();
                          void settingsApi.openExternal(href);
                        }}
                      >
                        {children}
                      </a>
                    ) : (
                      <span>{children}</span>
                    ),
                  // 图片与链接同一纪律：只认 https；限宽限高防撑爆弹窗。
                  img: ({ src, alt }) =>
                    typeof src === "string" && src.startsWith("https://") ? (
                      <img
                        src={src}
                        alt={alt ?? ""}
                        className="max-h-64 max-w-full rounded-md"
                      />
                    ) : null,
                }}
              >
                {current?.body.replaceAll("\n", "  \n") ?? ""}
              </ReactMarkdown>
            </span>
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2">
          <Button onClick={close}>{t("common.confirm")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
