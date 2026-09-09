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
import { generateUUID } from "@/utils/uuid";
import type { Settings } from "@/types";

/**
 * 匿名使用统计的告知弹窗（2026-09-09 拍板：纯告知形状，与共建告知同款）。
 *
 * 统计默认参与（维护者 2026-08-03 拍板，VS Code / Homebrew 同款模式）。这屏
 * 只做两件事：告知「已默认参与 + 分享什么」，指明关闭入口（设置 → 通用 →
 * 使用统计）。不设选择按钮 —— 拒绝只走设置开关。上传的知情不变式在后端：
 * `stats_notice_confirmed` 过 + `enable_anonymous_stats` 过 + 有 install_id
 * 才发（见 lib.rs 上报任务与 `relay::stats` 模块文档）。
 *
 * ## 端点没配时这一屏不弹
 *
 * 触发条件之一是「端点已配」（`relayApi.statsEndpointConfigured`，后端与
 * 上报任务共用同一个 `stats::is_configured`）。端点 2026-09-09 已切生产 ——
 * 这条判据保留是给将来回退 / 预发环境用的，两处共用一个事实不会漂移。
 *
 * ## installId 在点「知道了」那一刻才生成
 *
 * 不在装机时预生成：机器上不躺一个为统计准备的 id。且只在**当前仍在参与**
 * （`enableAnonymousStats !== false`）时才生成 —— 已在设置里关过的用户，
 * 确认标记照写（不再弹），但 id 不落地：关了就是真的什么都没有。
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
    // - 端点已配 —— 没配时同意与不同意的后果相同，问了是白问
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
      // 任何一个读失败就不弹：不能让统计功能在启动时弹报错。门禁在后端
      // （没看过告知一个字节都不发），跳过这次告知无副作用。
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
      // 只写确认标记，不动 enabled：设置里显式关过的不能被这屏翻回参与
      // （「拒绝只走设置」的另一面）。id 只在仍参与时生成（见组件文档）。
      await settingsApi.save({
        ...rest,
        statsNoticeConfirmed: true,
        ...(settings.enableAnonymousStats !== false
          ? { statsInstallId: settings.statsInstallId ?? generateUUID() }
          : {}),
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
      {/* 有意不给关闭途径：「知道了」同时是知情标记（后端门禁认 confirmed），
          别的路子关掉等于没看过告知。 */}
      <DialogContent className="max-w-md" zIndex="top">
        <DialogHeader>
          <DialogTitle>{t("loongport.stats.title")}</DialogTitle>
          <DialogDescription>
            {t("loongport.stats.body")}
            <br />
            {/* 两行小字：持久安装标识的诚实披露（不声称「完全匿名」）+ 关闭入口。
                边界细节在设置开关的描述里常驻。 */}
            <span className="text-xs">{t("loongport.stats.idNote")}</span>
            <br />
            <span className="text-xs">{t("loongport.stats.turnOffHint")}</span>
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2">
          <Button disabled={saving} onClick={() => void acknowledge()}>
            {t("loongport.stats.ok")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
