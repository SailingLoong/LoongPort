import { useEffect, useRef, useState } from "react";
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
import { relayApi } from "@/lib/api/relay";
import { settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

/** 启动后的首次判查延迟（避开首屏）。 */
const FIRST_CHECK_DELAY_MS = 5_000;
/** 判查轮询间隔：覆盖「首次成功登录/注册中转站后」这个时刻（≤20s 内弹）。 */
const POLL_INTERVAL_MS = 20_000;
/** 轮询兜底上限：一直没加站点的用户，不让计时器跑一辈子。 */
const POLL_DEADLINE_MS = 15 * 60_000;

/**
 * 站点实测共建的告知弹窗（2026-09-09 拍板：纯告知形状）。
 *
 * 共建自 2026-09-07 起默认参与，这屏只做两件事：告知「已默认参与 + 传什么」，
 * 指明关闭入口（设置 → 通用 → 站点实测数据共建）。不设选择按钮 —— 拒绝只走
 * 设置开关，显式关过的永不被默认值翻回。上传的知情不变式在后端（看过告知才
 * 发字节，见 crowd::uploader::upload_allowed）。
 *
 * 弹窗时机（2026-08-26 拍板，沿用）：**没确认过 且 已有中转站**。存量用户启动
 * 即满足；新用户在首次成功登录/注册站点后满足 —— 用轮询观测 `relay_list_sites`，
 * 不用在各登录流程里到处埋事件。每进程只主动弹一次。
 */
export function CrowdNoticeDialog() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const autoAskDone = useRef(false);

  // 每次打开前取最新设置：save 要回写整份对象。取失败就这一轮关掉（下次再弹）。
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

  // 主动告知：没确认过 且 已有中转站 → 弹。setTimeout 链（先 5s、后每 20s），
  // 弹过即停；超过兜底上限也停。
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const startedAt = Date.now();
    const tick = async () => {
      if (stopped || autoAskDone.current) return;
      try {
        const [s, sites] = await Promise.all([
          settingsApi.get(),
          relayApi.listSites(),
        ]);
        if (stopped || autoAskDone.current) return;
        if (s.crowdMetricsNoticeConfirmed === undefined && sites.length > 0) {
          autoAskDone.current = true;
          setSettings(s);
          setOpen(true);
          return;
        }
      } catch {
        // 读不到就等下一轮；门禁在后端，没看过告知什么都不会发生。
      }
      if (!stopped && Date.now() - startedAt < POLL_DEADLINE_MS) {
        timer = setTimeout(tick, POLL_INTERVAL_MS);
      }
    };
    timer = setTimeout(tick, FIRST_CHECK_DELAY_MS);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);

  const acknowledge = async () => {
    if (!settings || saving) return;
    setSaving(true);
    try {
      const { webdavSync: _webdavSync, ...rest } = settings;
      // 只写确认标记，不动 enabled：用户若已在设置里显式关过（enabled=false、
      // confirmed 未置），告知不能把他翻回参与 —— 「拒绝只走设置」的另一面。
      await settingsApi.save({
        ...rest,
        crowdMetricsNoticeConfirmed: true,
      });
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      setOpen(false);
    } catch {
      // 存不进去：不置 confirmed（下次还能弹），弹窗先收起别卡死用户。
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <Dialog open onOpenChange={() => {}}>
      {/* 有意不给关闭途径：「知道了」同时是知情标记（后端门禁认 confirmed），
          别的路子关掉等于没看过告知。zIndex 用 top：判查轮询可能落在用户已停在
          任何弹窗里的时候首弹。 */}
      <DialogContent className="max-w-md" zIndex="top">
        <DialogHeader>
          <DialogTitle>{t("loongport.crowd.notice.title")}</DialogTitle>
          <DialogDescription>
            {t("loongport.crowd.notice.body")}
            <br />
            {/* 一行小字指明关闭入口；边界细节在设置开关的描述里常驻。 */}
            <span className="text-xs">
              {t("loongport.crowd.notice.turnOffHint")}
            </span>
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2">
          <Button disabled={saving} onClick={() => void acknowledge()}>
            {t("loongport.crowd.notice.ok")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
