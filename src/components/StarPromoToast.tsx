import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { GITHUB_REPO } from "@/config/constants";
import { settingsApi } from "@/lib/api";
import { ONBOARDING_REGISTER_COMPLETED } from "@/lib/api/events";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { Settings } from "@/types";

/** 注册成功 toast（`RelaySection` 发）与本条之间的间隔：别两条叠在一起。 */
const SHOW_DELAY_MS = 5000;
/** toast 停留时长：比默认略长，给用户看清和反应的时间。 */
const TOAST_DURATION_MS = 10000;

/**
 * 「觉得有用就点个 ⭐」的一次性轻推：新用户第一次注册中转站成功（拿到核心价值）
 * 之后，用 sonner toast —— 不是模态弹窗 —— 提一次 GitHub 点星，之后永不再现
 *（`githubStarToastShown` settings 标志，与 `ccSwitchImportPrompted` 等同一个惯例）。
 *
 * 有意选 toast 而不是弹窗：模态求 star 是公认的打扰型做法，且本仓有意删过上游的
 * 首启欢迎弹窗（见 `App.tsx` 挂载处的注释），不该再往回走。常驻入口在顶栏的
 * `GitHubStarButton`，两者各管各的标志。
 *
 * 只挂在 `ONBOARDING_REGISTER_COMPLETED` 上：走「添加站点」手动路径的用户不推，
 * 他们仍有顶栏入口 —— 一条提示链路够了，不为覆盖全体用户再加第二个触发点。
 */
export function StarPromoToast() {
  const { t } = useTranslation();
  const settingsRef = useRef<Settings | null>(null);

  useEffect(() => {
    let cancelled = false;
    settingsApi
      .get()
      .then((s) => {
        if (!cancelled) settingsRef.current = s;
      })
      .catch(() => {
        // 读不到就不推：这是加分项，不为其报错。
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useTauriEvent<unknown>(ONBOARDING_REGISTER_COMPLETED, () => {
    const settings = settingsRef.current;
    // null = 设置还没加载完（注册窗至少要几秒，实际到不了这里）或加载失败 —— 都不推。
    if (!settings || settings.githubStarToastShown === true) return;

    // 先落标志再展示：存失败的话下次还会推，可接受；反过来（先展示后落）遇到崩溃
    // 会推第二次。同时更新本地 ref，同一次进程里重复事件也不会推两条。
    settingsRef.current = { ...settings, githubStarToastShown: true };
    const { webdavSync: _webdavSync, ...rest } = settings;
    settingsApi.save({ ...rest, githubStarToastShown: true }).catch(() => {});

    window.setTimeout(() => {
      toast(t("loongport.star.toastTitle"), {
        description: t("loongport.star.toastBody"),
        duration: TOAST_DURATION_MS,
        action: {
          label: t("loongport.star.toastAction"),
          onClick: () => void settingsApi.openExternal(GITHUB_REPO),
        },
      });
    }, SHOW_DELAY_MS);
  });

  return null;
}
