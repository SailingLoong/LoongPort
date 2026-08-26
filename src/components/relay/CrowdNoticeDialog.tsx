import { useEffect, useState } from "react";
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

interface CrowdNoticeDialogProps {
  open: boolean;
  /** 表态完成（无论参与与否）后回调 —— 调用方关弹窗、刷新展示。 */
  onDone: () => void;
}

/**
 * 站点实测共建的告知弹窗（结构克隆 `StatsNoticeDialog`，文案换血）。
 *
 * 与匿名统计那条告知的三点不同，都是**条款本身**的差异：
 * 1. **默认关 + 对等条款**：不参与则不能查看其他用户的实测数据 —— 拒绝不是
 *    「少贡献」而是「少获得」，所以这一条必须出现在弹窗里，不能只写在文案里；
 * 2. **标识每日轮换**：没有持久安装 id 可披露，说的是「每日轮换的随机标识」；
 * 3. **数据是聚合指标**：小时桶里没有任何请求明细，边界讲「站点级聚合」即可。
 *
 * 受控组件（`open` 由调用方持有）：触发点是实测区的「加入共建」入口与设置开关，
 * 不是首启全局弹窗 —— 用户带着「我要看什么」的上下文来理解「我付出什么」。
 */
export function CrowdNoticeDialog({ open, onDone }: CrowdNoticeDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // 打开时才取设置：save 要回写整份对象，取失败就这一轮不弹（下次再问）。
    settingsApi
      .get()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {
        if (!cancelled) onDone();
      });
    return () => {
      cancelled = true;
    };
    // onDone 是父组件的稳定回调；settings 变化不该重取。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

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
      onDone();
    } catch {
      // 存不进去：不置 confirmed（下次还能问），弹窗先收起别卡死用户。
      onDone();
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <Dialog open onOpenChange={() => {}}>
      {/* 有意不给关闭途径（结构同 StatsNoticeDialog）：这一屏要一个明确表态，
          两个按钮都能让它消失。zIndex 用 top：本弹窗从详情弹窗（也是 top）里
          唤起，DOM 后挂载者在上层。 */}
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
