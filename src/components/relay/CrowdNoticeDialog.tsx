import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { relayApi } from "@/lib/api/relay";
import { crowdKeys } from "@/lib/query/crowd";
import { settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

/** 广场实测区「加入共建」按钮唤起本弹窗用的窗口事件（拒绝过的用户再次加入的入口）。 */
export const CROWD_NOTICE_OPEN_EVENT = "loongport:crowd-notice-open";

/** 启动后的首次判查延迟（避开首屏）。 */
const FIRST_CHECK_DELAY_MS = 5_000;
/** 判查轮询间隔：覆盖「首次成功登录/注册中转站后」这个时刻（≤20s 内弹）。 */
const POLL_INTERVAL_MS = 20_000;
/** 轮询兜底上限：一直没加站点的用户，不让计时器跑一辈子。 */
const POLL_DEADLINE_MS = 15 * 60_000;

/**
 * 站点实测共建的告知弹窗。**一屏只问一件事**（维护者 2026-08-26 拍板，
 * 砍掉长文案）：愿不愿意看实测数据；一行小字交代代价（上传脱敏数据、可关）。
 * 边界细节不在这屏展开 —— 设置里开关的描述常驻完整边界。
 *
 * 弹窗时机（同日拍板）：**没表态过 且 已有中转站**。存量用户启动即满足；
 * 新用户在首次成功登录/注册站点后满足 —— 用轮询观测 `relay_list_sites`，
 * 不用在各登录流程里到处埋事件。
 *
 * 每进程只主动弹一次；广场锁定卡的「加入共建」仍可经
 * [@link CROWD_NOTICE_OPEN_EVENT] 再次唤起。
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

  // 主动告知：没表态 且 已有中转站 → 弹。setTimeout 链（先 5s、后每 20s），
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
        // 读不到就等下一轮；门禁在后端，没表态什么都不会发生。
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
      {/* 有意不给关闭途径：这一屏要一个明确表态，两个按钮都能让它消失。
          zIndex 用 top：可能从详情弹窗（也是 top）里经由事件唤起。 */}
      <DialogContent className="max-w-[24rem] gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {t("loongport.crowd.notice.title")}
        </DialogTitle>
        <DialogDescription className="mt-2 text-sm leading-relaxed">
          {t("loongport.crowd.notice.question")}
        </DialogDescription>

        {/* 一行小字：代价 + 退出通道。完整边界在设置开关的描述里常驻。 */}
        <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
          {t("loongport.crowd.notice.finePrint")}
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
