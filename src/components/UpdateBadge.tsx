import { useUpdate } from "@/contexts/UpdateContext";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ArrowUpCircle } from "lucide-react";

interface UpdateBadgeProps {
  className?: string;
  onClick?: () => void;
}

export function UpdateBadge({ className = "", onClick }: UpdateBadgeProps) {
  // 已跳过的版本不再亮徽章（跳过入口在设置→关于，「跳过本版本」）。
  const { hasUpdate, updateInfo, isDismissed } = useUpdate();
  const { t } = useTranslation();
  const isActive = hasUpdate && updateInfo && !isDismissed;
  const title = isActive
    ? t("settings.updateAvailable", {
        version: updateInfo?.availableVersion ?? "",
      })
    : t("settings.checkForUpdates");

  if (!isActive) {
    return null;
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={`
        relative h-8 w-8 rounded-full
        ${isActive ? "text-green-600 dark:text-green-400 hover:bg-green-50 dark:hover:bg-green-500/10" : "text-muted-foreground hover:bg-muted/60"}
        ${className}
      `}
    >
      <ArrowUpCircle className="h-5 w-5" />
    </Button>
  );
}
