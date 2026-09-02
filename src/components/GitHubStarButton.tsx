import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { GithubIcon } from "@/components/icons/GithubIcon";

/**
 * 顶栏的 GitHub 入口：常驻按钮 + 活动期小红点。
 *
 * 状态全部由 `App` 持有（star-reward 的 claimed 与弹窗都在那），本组件只
 * 呈现：`showDot` = 礼还没领（durable 的 claimed 判据，2026-08-17 起不再叠
 * 加「活动在不在」的瞬时缓存读 —— 活动下线时点击回落直接开仓库）。领过即
 * 熄，与「点没点过这个按钮」无关。
 */
export function GitHubStarButton({
  showDot,
  busy,
  onClick,
}: {
  /** 礼还没领（claimed 判据；活动是否在线由点击时的 offer 兜底）。 */
  showDot: boolean;
  /** 正在向后端要邀请 payload（纯本地读远端配置缓存，无网络等待）。 */
  busy: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const title = showDot
    ? t("loongport.star.dotTitle")
    : t("loongport.star.buttonTitle");

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={title}
      aria-label={title}
      onClick={onClick}
      disabled={busy}
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
