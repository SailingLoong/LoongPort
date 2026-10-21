import { useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  accountPlatformLabel,
  accountPlatforms,
  isAccountPlatform,
  type AccountPlatform,
} from "./accountPlatforms";

const storageKey = "loongport.account-platform-tabs.v1";
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
function readVisible(): AccountPlatform[] {
  try {
    const stored = JSON.parse(localStorage.getItem(storageKey) ?? "null");
    if (Array.isArray(stored))
      return [...new Set(stored.filter(isAccountPlatform))];
  } catch {
    /* A corrupt display preference must never block accounts. */
  }
  return accountPlatforms.map((item) => item.id);
}
export function AccountPlatformTabs({
  active,
  onSelect,
  disabled = false,
}: {
  active?: AccountPlatform;
  onSelect: (value: AccountPlatform | undefined) => void;
  disabled?: boolean;
}) {
  const [visible, setVisible] = useState(readVisible);
  const displayed =
    active && !visible.includes(active) ? [...visible, active] : visible;
  const [open, setOpen] = useState(false);
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const add = useRef<HTMLButtonElement>(null);
  const save = (next: AccountPlatform[]) => {
    setVisible(next);
    try {
      localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      /* Navigation remains usable in read-only storage. */
    }
  };
  const choose = (id: AccountPlatform) => {
    onSelect(id);
    requestAnimationFrame(() => refs.current.get(id)?.focus());
  };
  const hide = (id: AccountPlatform) => {
    const index = displayed.indexOf(id);
    const next = visible.filter((item) => item !== id);
    save(next);
    const remaining = displayed.filter((item) => item !== id);
    const neighbor = remaining[Math.min(index, remaining.length - 1)];
    if (active === id) onSelect(neighbor);
    requestAnimationFrame(() =>
      neighbor ? refs.current.get(neighbor)?.focus() : add.current?.focus(),
    );
  };
  return (
    <div className="flex min-w-0 items-start gap-1 border-b border-border pb-2">
      <div
        role="tablist"
        aria-label="服务平台"
        className="flex min-w-0 flex-1 gap-1 overflow-x-auto"
      >
        {displayed.map((id) => {
          const item = accountPlatforms.find((item) => item.id === id)!;
          const label = accountPlatformLabel(item);
          return (
            <div key={id} className="group relative shrink-0">
              <button
                ref={(node) => {
                  if (node) refs.current.set(id, node);
                  else refs.current.delete(id);
                }}
                role="tab"
                aria-selected={active === id}
                aria-label={label}
                disabled={disabled}
                tabIndex={
                  active === id ||
                  (!displayed.includes(active!) && displayed[0] === id)
                    ? 0
                    : -1
                }
                onClick={() => choose(id)}
                onKeyDown={(event) => {
                  if (
                    ![
                      "ArrowLeft",
                      "ArrowRight",
                      "Home",
                      "End",
                      "Delete",
                    ].includes(event.key)
                  )
                    return;
                  event.preventDefault();
                  if (event.key === "Delete") {
                    hide(id);
                    return;
                  }
                  const index = displayed.indexOf(id);
                  const target =
                    event.key === "Home"
                      ? displayed[0]
                      : event.key === "End"
                        ? displayed.at(-1)!
                        : displayed[
                            (index +
                              (event.key === "ArrowRight" ? 1 : -1) +
                              displayed.length) %
                              displayed.length
                          ];
                  choose(target);
                }}
                className={cn(
                  "flex h-11 items-center gap-2 rounded-lg border px-3 pr-7 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  active === id
                    ? "border-primary/40 bg-primary/10 text-primary"
                    : "border-transparent hover:bg-muted",
                )}
              >
                <AccountPlatformLogo platform={item} />
                <span>{item.name}</span>
                {"edition" in item && (
                  <span className="rounded border px-1 text-[10px]">
                    {item.edition}
                  </span>
                )}
              </button>
              <button
                type="button"
                disabled={disabled}
                onClick={() => hide(id)}
                aria-label={`隐藏 ${label} 标签`}
                title="只隐藏标签，不删除账号或改变共同池"
                className="absolute right-1 top-1 rounded p-0.5 opacity-0 hover:bg-muted focus:opacity-100 focus-visible:ring-2 group-hover:opacity-100 group-focus-within:opacity-100"
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          );
        })}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            ref={add}
            variant="ghost"
            size="icon"
            disabled={disabled}
            aria-label="添加平台标签"
            className="shrink-0"
          >
            <Plus className="h-4 w-4" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-72 space-y-1">
          <p className="pb-2 text-xs text-muted-foreground">
            仅改变导航显示，账号和共同池保持不变。
          </p>
          {accountPlatforms.map((item) => (
            <Button
              key={item.id}
              variant="ghost"
              className="w-full justify-start gap-2"
              aria-label={`${visible.includes(item.id) ? "查看" : "添加"} ${accountPlatformLabel(item)}`}
              onClick={() => {
                if (!visible.includes(item.id)) save([...visible, item.id]);
                onSelect(item.id);
                setOpen(false);
                requestAnimationFrame(() => refs.current.get(item.id)?.focus());
              }}
            >
              <AccountPlatformLogo platform={item} />
              {accountPlatformLabel(item)}
            </Button>
          ))}
        </PopoverContent>
      </Popover>
    </div>
  );
}
