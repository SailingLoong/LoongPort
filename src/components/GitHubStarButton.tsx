import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { GithubIcon } from "@/components/icons/GithubIcon";
import { GITHUB_REPO } from "@/config/constants";
import { settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

/**
 * 顶栏的 GitHub 入口：常驻按钮 + 首次小红点。
 *
 * 红点在用户点过一次这个按钮后永久消失（`githubStarBadgeClicked` settings 标志，
 * 与 `ccSwitchImportPrompted` 等一次性标志同一个惯例）。点掉 ≠ 点了星 ——
 * GitHub 的 starred API 需要登录态授权，桌面客户端不该为验证一个 UI 提示去要它，
 * 所以判据就是「点过」，不验证结果。
 *
 * 另一条「首次注册成功后的一次性 toast」在 `StarPromoToast`，与本按钮互不依赖
 * （toast 的 action 跳同一个仓库，但用户从那边过去不会点这里的按钮 —— 两个标志各管各的）。
 */
export function GitHubStarButton() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    let cancelled = false;
    settingsApi
      .get()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {
        // 读不到设置就当已点过处理（不显示红点）：宁可少一个提示，不在顶栏挂一个
        // 永远点不掉的假红点。
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 注意是「加载完且未点过」才显示 —— 反过来（默认显示）会让已点过的用户每次
  // 启动都看到红点闪一下再消失。
  const showDot = settings !== null && settings.githubStarBadgeClicked !== true;

  const handleClick = async () => {
    // 乐观更新：打开浏览器的那一刻红点就该消失；持久化失败最坏结果是下次启动
    // 红点再现，无害。
    if (settings) setSettings({ ...settings, githubStarBadgeClicked: true });
    await settingsApi.openExternal(GITHUB_REPO);
    if (!settings) return;
    try {
      // `webdavSync` 是只读的聚合字段，save 时要摘掉（与仓里其它调用点一致）。
      const { webdavSync: _webdavSync, ...rest } = settings;
      await settingsApi.save({
        ...rest,
        githubStarBadgeClicked: true,
      });
    } catch {
      // 见上：持久化失败可接受。
    }
  };

  const title = t("loongport.star.buttonTitle");

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={title}
      aria-label={title}
      onClick={() => void handleClick()}
      className="relative hover:bg-black/5 dark:hover:bg-white/5"
    >
      <GithubIcon className="h-4 w-4" />
      {showDot && (
        <span
          className="absolute top-1 right-1 h-2 w-2 rounded-full bg-red-500"
          aria-hidden="true"
        />
      )}
    </Button>
  );
}
