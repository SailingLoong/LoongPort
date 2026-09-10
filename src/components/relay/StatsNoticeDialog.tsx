import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { relayApi, settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

/**
 * 匿名使用统计的告知弹窗（2026-09-09 拍板：纯告知形状，与共建告知同款）。
 *
 * 统计默认参与（维护者 2026-08-03 拍板，VS Code / Homebrew 同款模式）。这屏
 * 只做两件事：告知「已默认参与 + 分享什么」，指明关闭入口（设置 → 通用 →
 * 使用统计）。不设选择按钮 —— 拒绝只走设置开关。
 *
 * 这屏**是知情标记，不是上报闸**（2026-09-09 细化）：上报从首次启动就发生，
 * 只受设置开关 `enable_anonymous_stats` 控制；install id 由后端在首次上报时
 * 自生成（见 lib.rs 上报任务与 `relay::stats` 模块文档），与这屏无关。
 *
 * ## 端点没配时这一屏不弹
 *
 * 触发条件之一是「端点已配」（`relayApi.statsEndpointConfigured`，后端与
 * 上报任务共用同一个 `stats::is_configured`）。端点 2026-09-09 已切生产 ——
 * 这条判据保留是给将来回退 / 预发环境用的，两处共用一个事实不会漂移。
 *
 * ## 只对首装机弹（2026-09-10 拍板）
 *
 * 存量升级用户由后端启动回填 `stats_notice_confirmed`（lib.rs 1.5 节，判据
 * 唯源 `fresh_install_at_startup`），这屏因此只为新装机首次启动弹一次；
 * 「知道了」落标记后永不再弹。
 */
export function StatsNoticeDialog() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 三个条件同时成立才弹：
    // - 端点已配 —— 没配时没有数据流，问了是白问
    // - 还没看过告知 —— 看过的不该再被打扰
    // - 统计仍开着 —— 已在设置里显式关过的用户，弹「已默认参与」是错报
    Promise.all([relayApi.statsEndpointConfigured(), settingsApi.get()])
      .then(([endpointConfigured, s]) => {
        if (cancelled) return;
        setSettings(s);
        setOpen(
          endpointConfigured &&
            s.statsNoticeConfirmed === undefined &&
            s.enableAnonymousStats !== false,
        );
      })
      // 任何一个读失败就不弹：不能让统计功能在启动时弹报错，跳过这次告知
      // 无副作用（这屏不是上报闸）。
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const acknowledge = async () => {
    if (!settings || saving) return;
    setSaving(true);
    try {
      const { webdavSync: _webdavSync, ...rest } = settings;
      // 只写「看过告知」标记，别的字段一概不碰：enabled 不动（「拒绝只走设置」），
      // install id 由后端自管，这屏碰它只会制造第二个写入者。
      await settingsApi.save({
        ...rest,
        statsNoticeConfirmed: true,
      });
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      setOpen(false);
    } catch {
      // 存不进去就不置 confirmed（下次还能弹），弹窗先收起别卡死用户。
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <Dialog open onOpenChange={() => {}}>
      {/* 有意不给关闭途径：「知道了」同时是知情标记，别的路子关掉等于没看过告知。 */}
      <DialogContent className="max-w-md" zIndex="top">
        {/* 通知型弹窗：标题/正文/按钮全部居中（与 CrowdNoticeDialog 同款）。 */}
        <DialogHeader className="sm:text-center">
          <DialogTitle>{t("loongport.stats.title")}</DialogTitle>
          <DialogDescription>
            {t("loongport.stats.body")}
            <br />
            {/* 一行小字指明关闭入口；安装标识等边界细节在设置开关描述里常驻。 */}
            <span className="text-xs">{t("loongport.stats.turnOffHint")}</span>
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="sm:justify-center">
          <Button disabled={saving} onClick={() => void acknowledge()}>
            {t("loongport.stats.ok")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
