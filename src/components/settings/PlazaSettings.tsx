import { useTranslation } from "react-i18next";
import { Store } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { ToggleRow } from "@/components/ui/toggle-row";
import { PLAZA_VISIBLE_DEFAULT, settingsApi } from "@/lib/api";
import { useSettingsQuery } from "@/lib/query";

/**
 * 「广场推荐列表」开关（设置 → 常规的最底部）。
 *
 * 只控制广场页内推荐列表的显隐（广场页与搜索框直连常驻，见
 * `RelayDirectoryPage` 的消费）。这是该开关的**唯一**用户入口：默认值由
 * 后端按首启归因播种（站长引流来的用户默认关），用户在这里翻转的结果永远
 * 优先。它**不走**表单的全量保存 —— `plazaVisible` 是后端专有字段（旧快照
 * 回写会抹掉刚播的种），改它走窄命令 `plaza_set_visible`，改完手动失效
 * settings 查询。
 */
export function PlazaSettings() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const checked = settings?.plazaVisible ?? PLAZA_VISIBLE_DEFAULT;

  const toggle = (visible: boolean) => {
    settingsApi
      .plazaSetVisible(visible)
      .then(() => queryClient.invalidateQueries({ queryKey: ["settings"] }))
      .catch((error) => {
        toast.error(String(error));
      });
  };

  return (
    <section className="space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40">
        <Store className="h-4 w-4 text-primary" />
        <h3 className="text-sm font-medium">{t("settings.plazaVisibility")}</h3>
      </div>

      <ToggleRow
        icon={<Store className="h-4 w-4 text-blue-500" />}
        title={t("settings.plazaVisibilityToggle")}
        description={t("settings.plazaVisibilityDescription")}
        checked={checked}
        onCheckedChange={(value) => toggle(value)}
      />
    </section>
  );
}
