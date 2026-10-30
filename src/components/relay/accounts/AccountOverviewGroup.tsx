import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { AccountPlatformLogo } from "./AccountPlatformTabs";
import type { AccountPlatform } from "./accountPlatforms";
import {
  AccountOrderMenu,
  AccountDragHandle,
  SortableAccountGroup,
  type AccountOrderItem,
} from "./AccountOrderSurface";
import { AccountOverviewRow } from "./AccountOverviewRow";
import type { AccountOverviewGroupVM } from "./accountOverviewGroups";
import type { AccountOverviewSort } from "./accountOverviewPreferences";
export function AccountOverviewGroup({
  group,
  orderItem,
  disabled,
  onToggle,
  onSort,
  onOpenDetails,
  onReorder,
}: {
  group: AccountOverviewGroupVM;
  orderItem: AccountOrderItem;
  disabled?: boolean;
  onToggle: () => void;
  onSort: (sort: AccountOverviewSort) => void;
  onOpenDetails: (key: string) => void;
  onReorder: (id: AccountPlatform, over: AccountPlatform) => void;
}) {
  const { t } = useTranslation();
  return (
    <SortableAccountGroup id={group.id} disabled={disabled} handleOnly>
      <section
        hidden={!group.visible}
        aria-label={`${group.label} · ${t("loongport.accounts.overview", { defaultValue: "账号总览" })}`}
        className="min-w-0 rounded-lg border border-border bg-card"
      >
        <Collapsible open={!group.collapsed} onOpenChange={onToggle}>
          <header className="flex flex-wrap items-center gap-2 px-3 py-2">
            <AccountDragHandle
              disabled={disabled}
              label={t("accountsOverview.drag", { name: group.label })}
            />
            <CollapsibleTrigger asChild>
              <Button
                size="sm"
                variant="ghost"
                disabled={disabled}
                aria-label={t(
                  group.collapsed
                    ? "accountsOverview.expand"
                    : "accountsOverview.collapse",
                  { name: group.label },
                )}
              >
                <AccountPlatformLogo platform={group.definition} />
                <span>{group.name}</span>
                {group.edition && (
                  <span className="text-xs text-muted-foreground">
                    {group.edition}
                  </span>
                )}
                <span className="text-xs text-muted-foreground">
                  {group.totalCount}
                </span>
              </Button>
            </CollapsibleTrigger>
            <div className="ml-auto flex flex-wrap items-center gap-2">
              <Select
                value={group.sort}
                disabled={disabled}
                onValueChange={(value) => onSort(value as AccountOverviewSort)}
              >
                <SelectTrigger
                  aria-label={`${group.label} ${t("accountsOverview.accountSort")}`}
                  className="h-8 w-auto gap-2 text-xs"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(["expiryAsc", "balanceDesc", "nameAsc"] as const).map(
                    (value) => (
                      <SelectItem key={value} value={value}>
                        {t(`accountsOverview.${value}`)}
                      </SelectItem>
                    ),
                  )}
                </SelectContent>
              </Select>
              <AccountOrderMenu
                items={[orderItem]}
                label={`${group.label} ${t("accountsOverview.sort")}`}
                vertical
                disabled={disabled}
                onReorder={onReorder}
              />
            </div>
          </header>
          <CollapsibleContent>
            {group.freshness !== "fresh" && (
              <p
                role="status"
                className="px-3 py-2 text-xs text-muted-foreground"
              >
                {t("loongport.accounts.staleSummary", {
                  defaultValue:
                    "读取失败，显示上次读取的账号摘要；当前状态未知。",
                })}
              </p>
            )}
            {group.rows.map((row) => (
              <AccountOverviewRow
                key={row.rowKey}
                row={row}
                disabled={disabled}
                onOpenDetails={onOpenDetails}
              />
            ))}
            {group.hiddenExhaustedCount > 0 && (
              <p className="px-3 py-2 text-xs text-muted-foreground">
                {t("accountsOverview.hiddenCount", {
                  count: group.hiddenExhaustedCount,
                })}
              </p>
            )}
          </CollapsibleContent>
        </Collapsible>
      </section>
    </SortableAccountGroup>
  );
}
