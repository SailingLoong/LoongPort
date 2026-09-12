import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { AutoFailoverConfigPanel } from "@/components/proxy/AutoFailoverConfigPanel";
import { LocalRoutingServicePanel } from "@/components/settings/LocalRoutingServicePanel";
import type { SettingsFormState } from "@/hooks/useSettings";
import { getAppLabel, PROXY_APP_IDS } from "@/config/appConfig";

interface ConnectionSettingsTabContentProps {
  settings: SettingsFormState;
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>;
}

export function ConnectionSettingsTabContent({
  settings,
  onAutoSave,
}: ConnectionSettingsTabContentProps) {
  const { t } = useTranslation();

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      <p className="text-sm text-muted-foreground">
        {t("settings.connection.applicationsHint", {
          defaultValue: "档位选择、排序和自动故障转移在应用页设置。",
        })}
      </p>
      <Accordion
        type="multiple"
        defaultValue={["localRouting"]}
        className="space-y-4"
      >
        <LocalRoutingServicePanel settings={settings} onAutoSave={onAutoSave} />
        {PROXY_APP_IDS.map((appType) => (
          <AccordionItem
            key={appType}
            value={appType}
            className="rounded-xl glass-card overflow-hidden"
          >
            <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
              {getAppLabel(appType)}
            </AccordionTrigger>
            <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
              <AutoFailoverConfigPanel appType={appType} />
            </AccordionContent>
          </AccordionItem>
        ))}
      </Accordion>
    </motion.div>
  );
}
