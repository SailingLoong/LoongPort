import { useTranslation } from "react-i18next";
import { Store } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { ToggleRow } from "@/components/ui/toggle-row";
import { settingsApi } from "@/lib/api";
import { useSettingsQuery } from "@/lib/query";

/**
 * 「中转站广场」开关（设置 → 常规的最底部）。
 *
 * 这是广场可见性的**唯一**用户入口：默认值由后端按首启归因播种（站长引流
 * 来的用户默认关），用户在这里翻转的结果永远优先。它**不走**表单的全量
 * 保存 —— `plazaVisible` 是后端专有字段（旧快照回写会抹掉刚播的种），改它
 * 走窄命令 `plaza_set_visible`，改完手动失效 settings 查询。
 */
export function PlazaSettings() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const checked = settings?.plazaVisible ?? true;

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
