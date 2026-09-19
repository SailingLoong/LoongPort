import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { UsageDashboard } from "./UsageDashboard";

/**
 * 「用量」独立视图（左边栏一级入口）。
 *
 * 用量仪表盘原本寄生在设置页的 tab 里 —— 但看用量是高频动作、设置是低频造访，
 * 记录/资源页想跳用量都得「借道设置页」（App.tsx 曾经的 setSettingsDefaultTab("usage")）。
 * 这里只做接线（刷新间隔读写 settings），仪表盘本体仍是 UsageDashboard，一处实现两个宿主。
 */
export function UsagePage() {
  const { t } = useTranslation();
  const { settings, autoSaveSettings } = useSettings();

  return (
    <div
      className="page-content flex-1 overflow-y-auto pr-2"
      aria-label={t("usage.title")}
    >
      <UsageDashboard
        refreshIntervalMs={settings?.usageDashboardRefreshIntervalMs}
        onRefreshIntervalChange={async (next) => {
          // autoSaveSettings 失败时返回 null（错误已 toast）—— 传回 false 让
          // 仪表盘把刷新间隔的选择回滚，避免 UI 与持久化值分叉。
          const result = await autoSaveSettings({
            usageDashboardRefreshIntervalMs: next,
          });
          return result !== null;
        }}
      />
    </div>
  );
}
