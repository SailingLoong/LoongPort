import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Check, X } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { crowdKeys } from "@/lib/query/crowd";
import { settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

/** 广场实测区「加入共建」按钮唤起本弹窗用的窗口事件（拒绝过的用户再次加入的入口）。 */
export const CROWD_NOTICE_OPEN_EVENT = "loongport:crowd-notice-open";

/** 主动告知的延迟：避开首屏渲染与其它启动弹窗的拥挤（数值是体感值，无硬约束）。 */
const AUTO_OPEN_DELAY_MS = 5_000;

/**
 * 站点实测共建的告知弹窗（结构克隆 `StatsNoticeDialog`，文案换血）。
 *
 * ## 主动告知（维护者 2026-08-26 拍板，推翻早前「功能入口处弹」的方案）
 *
 * 早前方案把弹窗放在「首次点开实测区」—— 升级用户根本走不到那个入口，
 * 功能对他们等于隐形；而数据共享的**同意必须先于功能可见**。所以现在
 * 启动后若从未表态（`crowdMetricsNoticeConfirmed === undefined`）就主动弹一次。
 *
 * 每进程只主动弹一次；拒绝过/参与过都不再打扰，但广场锁定卡的「加入共建」
 * 仍可通过 [`CROWD_NOTICE_OPEN_EVENT`] 再次唤起（改主意的入口）。
 *
 * 与匿名统计那条告知的三点不同，都是**条款本身**的差异：
 * 1. **默认关 + 对等条款**：不参与则不能查看其他用户的实测数据 —— 拒绝不是
 *    「少贡献」而是「少获得」，所以这一条必须出现在弹窗里，不能只写在文案里；
 * 2. **标识每日轮换**：没有持久安装 id 可披露，说的是「每日轮换的随机标识」；
 * 3. **数据是聚合指标**：小时桶里没有任何请求明细，边界讲「站点级聚合」即可。
 */
export function CrowdNoticeDialog() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const autoAskDone = useRef(false);

  // 每次打开前取最新设置：save 要回写整份对象。取失败就这一轮关掉（下次再问）。
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    settingsApi
      .get()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {
        if (!cancelled) setOpen(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  // 主动告知：启动后延迟弹一次，条件是「从未表态」。表过态（含拒绝）永不主动再弹。
  useEffect(() => {
    const timer = setTimeout(() => {
      if (autoAskDone.current) return;
      autoAskDone.current = true;
      settingsApi
        .get()
        .then((s) => {
          if (s.crowdMetricsNoticeConfirmed === undefined) {
            setSettings(s);
            setOpen(true);
          }
        })
        .catch(() => {
          // 读不到设置就不弹 —— 门禁在后端，没表态什么都不会发生。
        });
    }, AUTO_OPEN_DELAY_MS);
    return () => clearTimeout(timer);
  }, []);

  // 广场「加入共建」的再入口（拒绝过的用户改主意）。
  useEffect(() => {
    const onOpenEvent = () => setOpen(true);
    window.addEventListener(CROWD_NOTICE_OPEN_EVENT, onOpenEvent);
    return () =>
      window.removeEventListener(CROWD_NOTICE_OPEN_EVENT, onOpenEvent);
  }, []);

  const respond = async (participate: boolean) => {
    if (!settings || saving) return;
    setSaving(true);
    try {
      const { webdavSync: _webdavSync, ...rest } = settings;
      await settingsApi.save({
        ...rest,
        crowdMetricsNoticeConfirmed: true,
        crowdMetricsEnabled: participate,
      });
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      // 参与的那一刻快照才有意义 —— 失效让它立即现拉（命令层门禁刚开）。
      await queryClient.invalidateQueries({ queryKey: crowdKeys.all });
      setOpen(false);
    } catch {
      // 存不进去：不置 confirmed（下次还能问），弹窗先收起别卡死用户。
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <Dialog open onOpenChange={() => {}}>
      {/* 有意不给关闭途径（结构同 StatsNoticeDialog）：这一屏要一个明确表态，
          两个按钮都能让它消失。zIndex 用 top：可能从详情弹窗（也是 top）里
          经由事件唤起，DOM 后挂载者在上层。 */}
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {t("loongport.crowd.notice.title")}
        </DialogTitle>
        <DialogDescription className="mt-1.5 text-sm leading-relaxed">
          {t("loongport.crowd.notice.intro")}
        </DialogDescription>

        <div className="mt-4 space-y-2.5 rounded-lg bg-muted/40 p-3.5">
          <div className="flex gap-2.5">
            <Check className="mt-0.5 h-4 w-4 shrink-0 text-green-500" />
            <p className="text-[13px] leading-relaxed">
              <span className="font-medium">
                {t("loongport.crowd.notice.sendsLabel")}
              </span>
              <span className="text-muted-foreground">
                {" "}
                {t("loongport.crowd.notice.sendsBody")}
              </span>
            </p>
          </div>
          <div className="flex gap-2.5">
            <X className="mt-0.5 h-4 w-4 shrink-0 text-red-500" />
            <p className="text-[13px] leading-relaxed">
              <span className="font-medium">
                {t("loongport.crowd.notice.neverLabel")}
              </span>
              <span className="text-muted-foreground">
                {" "}
                {t("loongport.crowd.notice.neverBody")}
              </span>
            </p>
          </div>
        </div>

        {/* 对等条款单独一行：它改变「拒绝」的后果（不只是少贡献，而是少获得），
            藏在正文里等于没告知。 */}
        <p className="mt-3 text-xs font-medium leading-relaxed text-foreground">
          {t("loongport.crowd.notice.reciprocity")}
        </p>
        <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
          {t("loongport.crowd.notice.idNote")}
        </p>
        <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
          {t("loongport.crowd.notice.canChange")}
        </p>

        <div className="mt-5 flex justify-end gap-2">
          <Button
            variant="ghost"
            size="sm"
            disabled={saving}
            onClick={() => void respond(false)}
          >
            {t("loongport.crowd.notice.decline")}
          </Button>
          <Button
            size="sm"
            disabled={saving}
            onClick={() => void respond(true)}
          >
            {t("loongport.crowd.notice.accept")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
