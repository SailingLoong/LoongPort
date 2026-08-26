import { useTranslation } from "react-i18next";
import type { SettingsFormState } from "@/hooks/useSettings";
import {
  AppWindow,
  BarChart3,
  Gauge,
  MonitorUp,
  Power,
  EyeOff,
} from "lucide-react";
import { ToggleRow } from "@/components/ui/toggle-row";
import { AnimatePresence, motion } from "framer-motion";
import { isLinux } from "@/lib/platform";

interface WindowSettingsProps {
  settings: SettingsFormState;
  onChange: (updates: Partial<SettingsFormState>) => void;
}

export function WindowSettings({ settings, onChange }: WindowSettingsProps) {
  const { t } = useTranslation();

  return (
    <section className="space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40">
        <AppWindow className="h-4 w-4 text-primary" />
        <h3 className="text-sm font-medium">{t("settings.windowBehavior")}</h3>
      </div>

      <div className="space-y-3">
        {/* 匿名使用统计。放在这里（而不是单独开一个「隐私」区）是因为它是**唯一**
            一条隐私相关的开关 —— 为一条开关立一个区是过度设计。
            默认开，首启弹过 `StatsNoticeDialog` 告知；这里是用户回来关掉它的地方。 */}
        <ToggleRow
          icon={<BarChart3 className="h-4 w-4 text-blue-500" />}
          title={t("settings.anonymousStats")}
          description={t("settings.anonymousStatsDescription")}
          checked={settings.enableAnonymousStats ?? true}
          onCheckedChange={(value) =>
            // ⚠️ **关掉时把安装标识一起清掉**（review 抓出的不一致）。
            //
            // `StatsNoticeDialog` 自己论证过「不该预生成 id，否则用户选了不参与、
            // 机器上却躺着一个为统计准备的 id」—— 那条论证对**后来关掉**同样成立，
            // 而原来这里只写 false、把 id 留着。
            //
            // 后果不只是「留了个文件」：用户关掉半年后再打开，服务端会把新旧上报
            // **重新关联到同一个安装** —— 而他合理预期「关了再开」是全新开始。
            onChange(
              value
                ? { enableAnonymousStats: true }
                : { enableAnonymousStats: false, statsInstallId: undefined },
            )
          }
        />

        {/* 站点实测共建（第二个隐私开关，紧挨着第一个）。与匿名统计的区别：
            默认关 + 对等条款（参与才解锁广场实测数据）。在这里打开也算一次
            明确表态 —— 直接置 confirmed，不再追着弹告知。 */}
        <ToggleRow
          icon={<Gauge className="h-4 w-4 text-teal-500" />}
          title={t("settings.crowdMetrics")}
          description={t("settings.crowdMetricsDescription")}
          checked={!!settings.crowdMetricsEnabled}
          onCheckedChange={(value) =>
            onChange(
              value
                ? {
                    crowdMetricsEnabled: true,
                    crowdMetricsNoticeConfirmed: true,
                  }
                : { crowdMetricsEnabled: false },
            )
          }
        />

        <ToggleRow
          icon={<Power className="h-4 w-4 text-orange-500" />}
          title={t("settings.launchOnStartup")}
          description={t("settings.launchOnStartupDescription")}
          checked={!!settings.launchOnStartup}
          onCheckedChange={(value) => onChange({ launchOnStartup: value })}
        />

        <AnimatePresence initial={false}>
          {settings.launchOnStartup && (
            <motion.div
              key="silent-startup"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 10 }}
              transition={{ duration: 0.3 }}
            >
              <ToggleRow
                icon={<EyeOff className="h-4 w-4 text-green-500" />}
                title={t("settings.silentStartup")}
                description={t("settings.silentStartupDescription")}
                checked={!!settings.silentStartup}
                onCheckedChange={(value) => onChange({ silentStartup: value })}
              />
            </motion.div>
          )}
        </AnimatePresence>

        <ToggleRow
          icon={<MonitorUp className="h-4 w-4 text-purple-500" />}
          title={t("settings.enableClaudePluginIntegration")}
          description={t("settings.enableClaudePluginIntegrationDescription")}
          checked={!!settings.enableClaudePluginIntegration}
          onCheckedChange={(value) =>
            onChange({ enableClaudePluginIntegration: value })
          }
        />

        <ToggleRow
          icon={<MonitorUp className="h-4 w-4 text-cyan-500" />}
          title={t("settings.skipClaudeOnboarding")}
          description={t("settings.skipClaudeOnboardingDescription")}
          checked={!!settings.skipClaudeOnboarding}
          onCheckedChange={(value) => onChange({ skipClaudeOnboarding: value })}
        />

        <ToggleRow
          icon={<AppWindow className="h-4 w-4 text-blue-500" />}
          title={t("settings.minimizeToTray")}
          description={t("settings.minimizeToTrayDescription")}
          checked={settings.minimizeToTrayOnClose}
          onCheckedChange={(value) =>
            onChange({ minimizeToTrayOnClose: value })
          }
        />

        {isLinux() && (
          <ToggleRow
            icon={<AppWindow className="h-4 w-4 text-amber-500" />}
            title={t("settings.useAppWindowControls")}
            description={t("settings.useAppWindowControlsDescription")}
            checked={!!settings.useAppWindowControls}
            onCheckedChange={(value) =>
              onChange({ useAppWindowControls: value })
            }
          />
        )}
      </div>
    </section>
  );
}
