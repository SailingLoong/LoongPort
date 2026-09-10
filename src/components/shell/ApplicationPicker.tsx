import { useTranslation } from "react-i18next";
import { APP_IDS, getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import type { VisibleApps } from "@/types";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export function ApplicationPicker({
  activeApp,
  onSwitch,
  visibleApps,
}: {
  activeApp: AppId;
  onSwitch: (app: AppId) => void;
  visibleApps: VisibleApps;
}) {
  const { t } = useTranslation();
  const apps = APP_IDS.filter((id) => id !== "codex-image");
  return (
    <Select value={activeApp} onValueChange={(app) => onSwitch(app as AppId)}>
      <SelectTrigger
        className="w-[180px]"
        aria-label={t("client.selectApplication")}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {[true, false].map((favorite) => (
          <SelectGroup key={String(favorite)}>
            <SelectLabel>
              {t(favorite ? "client.favoriteApps" : "client.moreApps")}
            </SelectLabel>
            {apps
              .filter((app) => Boolean(visibleApps[app]) === favorite)
              .map((app) => (
                <SelectItem key={app} value={app}>
                  {getAppDisplayName(app, t)}
                </SelectItem>
              ))}
          </SelectGroup>
        ))}
      </SelectContent>
    </Select>
  );
}
