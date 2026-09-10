import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronDown,
  History,
  Plus,
  Search,
  Settings2,
} from "lucide-react";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type { AccountRoute } from "@/components/shell/navigation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";
import { useApplicationOverview } from "./useApplicationOverview";

interface Props {
  appId: AppId;
  providers: Record<string, Provider>;
  onSwitchProvider: (provider: Provider) => void;
  onOpenAccount: (account: AccountRoute) => void;
  onAdd: () => void;
  children: ReactNode;
}
const panel =
  "rounded-2xl border border-border/70 bg-card p-5 shadow-sm sm:p-6";

export function ApplicationWorkspace({
  appId,
  providers,
  onSwitchProvider,
  onOpenAccount,
  onAdd,
  children,
}: Props) {
  const { t } = useTranslation();
  const model = useApplicationOverview(appId, providers, onSwitchProvider);
  const [selecting, setSelecting] = useState(false);
  const [managing, setManaging] = useState(false);
  const [search, setSearch] = useState("");
  const [source, setSource] = useState("all");
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});
  const configurations = model.data?.configurations ?? [];
  const current = configurations.filter((item) =>
    model.data?.isAdditive
      ? item.presentation.isInConfig
      : item.presentation.isCurrent,
  );
  const recent = (model.data?.recentProviderIds ?? []).flatMap((id) => {
    const item = configurations.find(
      (candidate) => candidate.providerId === id,
    );
    return item && !current.some((candidate) => candidate.providerId === id)
      ? [item]
      : [];
  });
  const matching = configurations.filter(
    (item) =>
      (source === "all" || item.source === source) &&
      [
        item.name,
        item.serviceName,
        item.accountLabel,
        item.configurationName,
        item.model,
      ].some((value) =>
        value?.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()),
      ),
  );
  const groups = new Map<string, ApplicationConfiguration[]>();
  for (const item of matching) {
    const key = item.account
      ? `${item.account.kind}:${item.account.id}`
      : item.source;
    groups.set(key, [...(groups.get(key) ?? []), item]);
  }
  const details = (item: ApplicationConfiguration) => (
    <div className="min-w-0 space-y-1">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="break-words font-medium">
          {item.configurationName ?? item.name}
        </h3>
        {item.presentation.isDefaultModel && (
          <span className="rounded-md bg-blue-500/10 px-2 py-0.5 text-xs text-blue-600 dark:text-blue-400">
            {t("applications.default")}
          </span>
        )}
      </div>
      {(item.serviceName || item.accountLabel) && (
        <p className="break-words text-sm text-muted-foreground">
          {[item.serviceName, item.accountLabel].filter(Boolean).join(" · ")}
        </p>
      )}
      {item.model && (
        <p className="break-words text-sm text-muted-foreground">
          {item.model}
        </p>
      )}
    </div>
  );
  return (
    <div className="space-y-5">
      {selecting ? (
        <section className={panel}>
          <Button
            variant="ghost"
            className="mb-4 -ml-2"
            onClick={() => setSelecting(false)}
          >
            <ArrowLeft className="h-4 w-4" />
            {t("common.back")}
          </Button>
          <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="text-lg font-semibold">
                {t("applications.switchService")}
              </h2>
              <p className="mt-1 text-sm text-muted-foreground">
                {t("applications.selectDescription")}
              </p>
            </div>
            <Button variant="outline" onClick={onAdd}>
              <Plus className="h-4 w-4" />
              {t("applications.addService")}
            </Button>
          </div>
          <div className="relative">
            <Search className="absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
            <Input
              autoFocus
              type="search"
              aria-label={t("applications.search")}
              placeholder={t("applications.search")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className="pl-9"
            />
          </div>
          <div
            className="my-4 flex flex-wrap gap-2"
            role="group"
            aria-label={t("applications.source")}
          >
            {["all", "official", "relay", "custom"].map((value) => (
              <Button
                key={value}
                variant="toggle"
                size="sm"
                aria-pressed={source === value}
                onClick={() => setSource(value)}
              >
                {t(`applications.sources.${value}`)}
              </Button>
            ))}
          </div>
          <div className="space-y-3">
            {[...groups].map(([key, items]) => (
              <Collapsible
                key={key}
                open={search.trim() !== "" || (openGroups[key] ?? false)}
                onOpenChange={(open) =>
                  setOpenGroups((previous) => ({ ...previous, [key]: open }))
                }
                className="rounded-xl bg-muted/35"
              >
                <CollapsibleTrigger className="flex w-full items-center gap-3 rounded-xl p-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  <ChevronDown className="h-4 w-4 shrink-0 transition-transform [[data-state=closed]>&]:-rotate-90" />
                  <span className="min-w-0 flex-1">
                    <span className="block break-words text-sm font-medium">
                      {items[0].serviceName ??
                        t(`applications.sources.${items[0].source}`)}
                    </span>
                    {items[0].accountLabel && (
                      <span className="block break-words text-xs text-muted-foreground">
                        {items[0].accountLabel}
                      </span>
                    )}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {items.length}
                  </span>
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <div className="space-y-2 px-3 pb-3">
                    {items.map((item) => (
                      <div
                        key={item.providerId}
                        className="flex flex-wrap items-center justify-between gap-3 rounded-lg bg-card p-4"
                      >
                        {details(item)}
                        <div className="flex shrink-0 items-center gap-2">
                          {(item.presentation.isCurrent ||
                            (model.data?.isAdditive &&
                              item.presentation.isInConfig)) && (
                            <Check
                              className="h-4 w-4 text-blue-600"
                              aria-label={t("applications.configured")}
                            />
                          )}
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={model.busy || !item.canSelect}
                            aria-label={`${t("applications.use")} ${item.configurationName ?? item.name}`}
                            onClick={() => void model.select(item)}
                          >
                            {t("applications.use")}
                          </Button>
                        </div>
                      </div>
                    ))}
                  </div>
                </CollapsibleContent>
              </Collapsible>
            ))}
            {matching.length === 0 && !model.isPending && (
              <p className="py-8 text-center text-sm text-muted-foreground">
                {t("applications.noMatches")}
              </p>
            )}
          </div>
        </section>
      ) : (
        <>
          <section className={panel}>
            <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
              <div>
                <h2 className="text-lg font-semibold">
                  {t(
                    model.data?.isAdditive
                      ? "applications.enabledConfigurations"
                      : "applications.currentConfiguration",
                  )}
                </h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  {t("applications.currentDescription")}
                </p>
              </div>
              <Button onClick={() => setSelecting(true)}>
                {t("applications.switchService")}
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
            <div className="space-y-3">
              {current.map((item) => (
                <div
                  key={item.providerId}
                  className="flex flex-wrap items-center justify-between gap-4 rounded-xl bg-muted/35 p-4"
                >
                  {details(item)}
                  {item.account && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        item.account && onOpenAccount(item.account)
                      }
                    >
                      {t("applications.manageAccount")}
                      <ArrowRight className="h-4 w-4" />
                    </Button>
                  )}
                </div>
              ))}
            </div>
            {current.length === 0 && !model.isPending && !model.error && (
              <div className="rounded-xl bg-muted/35 px-5 py-8 text-center">
                <p className="mb-4 text-sm text-muted-foreground">
                  {t("applications.empty")}
                </p>
                <Button variant="outline" onClick={onAdd}>
                  <Plus className="h-4 w-4" />
                  {t("applications.addService")}
                </Button>
              </div>
            )}
          </section>
          {recent.length > 0 && (
            <section className={panel}>
              <h2 className="mb-4 flex items-center gap-2 text-sm font-semibold">
                <History className="h-4 w-4 text-muted-foreground" />
                {t("applications.recent")}
              </h2>
              <div className="grid gap-3 lg:grid-cols-2">
                {recent.map((item) => (
                  <div
                    key={item.providerId}
                    className="flex flex-wrap items-center justify-between gap-3 rounded-xl bg-muted/35 p-4"
                  >
                    {details(item)}
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={model.busy || !item.canSelect}
                      onClick={() => void model.select(item)}
                    >
                      {t("applications.useAgain")}
                    </Button>
                  </div>
                ))}
              </div>
            </section>
          )}
        </>
      )}
      {model.isPending && (
        <p role="status" className="p-4 text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {model.error && (
        <div
          role="alert"
          className="flex items-center justify-between gap-4 rounded-xl bg-muted p-4 text-sm"
        >
          <span>{t("applications.loadFailed")}</span>
          <Button variant="outline" onClick={() => void model.refetch()}>
            {t("common.retry")}
          </Button>
        </div>
      )}
      <Collapsible open={managing} onOpenChange={setManaging} className={panel}>
        <CollapsibleTrigger asChild>
          <Button variant="ghost" className="-ml-2">
            <Settings2 className="h-4 w-4" />
            {t("applications.manageConfigurations")}
            <ChevronDown className="h-4 w-4" />
          </Button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <p className="mb-5 mt-2 text-sm text-muted-foreground">
            {t("applications.manageDescription")}
          </p>
          {children}
        </CollapsibleContent>
      </Collapsible>
      <SwitchTierConfirmDialog
        targetName={model.confirmation}
        onCancel={model.cancel}
        onSwitch={model.confirm}
      />
    </div>
  );
}
