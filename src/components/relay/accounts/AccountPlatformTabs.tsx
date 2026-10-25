import { useTranslation } from "react-i18next";
import { useRef } from "react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  accountPlatformLabel,
  accountPlatforms,
  type AccountPlatform,
} from "./accountPlatforms";

const logos = import.meta.glob<string>(
  "../../../assets/account-platforms/*.{svg,png}",
  { eager: true, query: "?url", import: "default" },
);
export function AccountPlatformLogo({
  platform,
}: {
  platform: (typeof accountPlatforms)[number];
}) {
  return (
    <img
      src={logos[`../../../assets/account-platforms/${platform.logo}`]}
      alt={`${platform.name} Logo`}
      className="h-5 w-5 shrink-0 object-contain"
    />
  );
}
/** Single account filter. Platform presence is projected from native account owners. */
export function AccountPlatformTabs({
  active,
  platforms,
  onSelect,
  disabled = false,
  editions = {},
}: {
  active?: AccountPlatform;
  platforms: readonly AccountPlatform[];
  onSelect: (value: AccountPlatform | undefined) => void;
  disabled?: boolean;
  editions?: Partial<Record<AccountPlatform, "CN" | "INTL">>;
}) {
  const { t } = useTranslation();
  const allLabel = t("loongport.accounts.allPlatforms", {
    defaultValue: "全部账号",
  });
  const displayed = [...new Set(platforms)];
  const choices: (AccountPlatform | undefined)[] = [undefined, ...displayed];
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const choose = (id: AccountPlatform | undefined) => {
    onSelect(id);
    requestAnimationFrame(() => {
      const target = refs.current.get(id ?? "all");
      target?.focus({ preventScroll: true });
      target?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    });
  };
  return (
    <TooltipProvider delayDuration={350}>
      <div
        role="tablist"
        aria-label={t("loongport.accounts.platformFilter", {
          defaultValue: "平台账号筛选",
        })}
        className="flex min-w-0 gap-2 scroll-px-1 overflow-x-auto border-b border-border p-1 pb-2"
      >
        {choices.map((id) => {
          const item = accountPlatforms.find((item) => item.id === id);
          const label = item ? accountPlatformLabel(item) : allLabel;
          const selected = active === id;
          const edition =
            item &&
            ("edition" in item
              ? item.edition === "国际版"
                ? "INTL"
                : "CN"
              : editions[item.id]);
          return (
            <Tooltip key={id ?? "all"}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  role="tab"
                  aria-label={label}
                  aria-selected={selected}
                  disabled={disabled}
                  tabIndex={
                    selected || (!displayed.includes(active!) && !id) ? 0 : -1
                  }
                  ref={(node) => {
                    if (node) refs.current.set(id ?? "all", node);
                    else refs.current.delete(id ?? "all");
                  }}
                  onClick={() => choose(id)}
                  onKeyDown={(event) => {
                    if (
                      !["ArrowLeft", "ArrowRight", "Home", "End"].includes(
                        event.key,
                      )
                    )
                      return;
                    event.preventDefault();
                    const index = choices.indexOf(id);
                    choose(
                      event.key === "Home"
                        ? choices[0]
                        : event.key === "End"
                          ? choices.at(-1)
                          : choices[
                              (index +
                                (event.key === "ArrowRight" ? 1 : -1) +
                                choices.length) %
                                choices.length
                            ],
                    );
                  }}
                  className={cn(
                    "relative flex h-10 shrink-0 items-center justify-center rounded-lg border text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    item ? "w-10" : "min-w-[84px] px-3",
                    edition && "pb-2",
                    selected
                      ? "border-primary/40 bg-primary/10 text-primary"
                      : "border-border hover:bg-muted",
                  )}
                >
                  {item ? <AccountPlatformLogo platform={item} /> : allLabel}
                  {edition && (
                    <span
                      aria-hidden="true"
                      className="absolute bottom-0.5 left-1/2 -translate-x-1/2 rounded bg-background/90 px-0.5 text-[8px] font-semibold leading-[10px]"
                    >
                      {edition}
                    </span>
                  )}
                </button>
              </TooltipTrigger>
              <TooltipContent side="bottom">{label}</TooltipContent>
            </Tooltip>
          );
        })}
      </div>
    </TooltipProvider>
  );
}
