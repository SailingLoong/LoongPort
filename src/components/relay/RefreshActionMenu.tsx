import { Loader2, MoreHorizontal, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

interface RefreshActionMenuProps {
  actionLabel: string;
  loading: boolean;
  onAction: () => void;
}

/**
 * 两类账号行共用的次要刷新动作入口。
 *
 * 圆形刷新图标无法说明它到底会重拉分组，还是只会把本地密钥重新写入配置；
 * 统一收进「更多」菜单后，动作名称可以完整展示，也不会与真正的全局刷新混淆。
 */
export function RefreshActionMenu({
  actionLabel,
  loading,
  onAction,
}: RefreshActionMenuProps) {
  const { t } = useTranslation();
  const menuLabel = t("loongport.row.more");

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0 text-muted-foreground hover:text-foreground"
          disabled={loading}
          aria-label={menuLabel}
          title={menuLabel}
        >
          {loading ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <MoreHorizontal className="h-3.5 w-3.5" />
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem disabled={loading} onSelect={() => onAction()}>
          <RefreshCw className="h-3.5 w-3.5" />
          {actionLabel}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
