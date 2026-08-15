import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { GithubIcon } from "@/components/icons/GithubIcon";

/**
 * 顶栏的 GitHub 入口：常驻按钮 + 活动期小红点。
 *
 * 状态全部由 `App` 持有（star-reward 的 claimed / configured 与弹窗都在那），
 * 本组件只呈现：`showDot` = 「点 Star 领注册礼」活动在且还没领过 —— 领过即熄，
 * 与「点没点过这个按钮」无关。点击行为也由 App 决定（活动在 ⇒ 问后端要邀请
 * payload 弹窗；不在 ⇒ 直接开仓库）。
 */
export function GitHubStarButton({
  showDot,
  busy,
  onClick,
}: {
  /** star 领礼活动在 && 未领取。 */
  showDot: boolean;
  /** 正在向后端要邀请 payload（那次基线取数要过网络）。 */
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
