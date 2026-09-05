/**
 * 省心视图的模型选择器：可输入过滤的 Combobox（形状抄 ProfileSwitcher
 * 的 Command-in-Popover）。每行除了模型名，还带「N 档 · 最低 $X.XX/M」
 * ——覆盖度与决策成本两个数，都由看板后端算好（唯源）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronsUpDown } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { fmtUsd } from "@/components/usage/format";
import type { TierBoardModelOption } from "@/lib/api/autoMode";

/** Select 不能用空串当 value 的哨兵（与 EasyBoard 同一个约定）。 */
export const MODEL_ANY = "__any__";

export function ModelPicker({
  model,
  modelOptions,
  disabled,
  onSelect,
}: {
  model: string | null;
  modelOptions: TierBoardModelOption[];
  disabled?: boolean;
  onSelect: (model: string | null) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const label = model ?? t("autoMode.modelAny", { defaultValue: "不限模型" });
  const meta = (option: TierBoardModelOption) => {
    const tiers = t("autoMode.board.tiersShort", { defaultValue: "档" });
    const count = `${option.tierCount} ${tiers}`;
    return option.cheapestPricePerMillion != null
      ? `${count} · ${fmtUsd(option.cheapestPricePerMillion, 2)}/M`
      : count;
  };

  const handleSelect = (value: string) => {
    setOpen(false);
    onSelect(value === MODEL_ANY ? null : value);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          disabled={disabled}
          className="w-64 justify-between font-normal"
        >
          <span className="truncate">{label}</span>
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-72 p-0" align="start">
        <Command label={label}>
          <CommandInput
            placeholder={t("autoMode.board.modelFilterPlaceholder", {
              defaultValue: "输入模型名筛选…",
            })}
          />
          <CommandList>
            <CommandEmpty>
              {t("autoMode.board.modelFilterEmpty", {
                defaultValue: "没有匹配的模型",
              })}
            </CommandEmpty>
            <CommandGroup>
              <CommandItem value={MODEL_ANY} onSelect={handleSelect}>
                <Check
                  className={cn(
                    "mr-2 h-4 w-4 shrink-0",
                    model == null ? "opacity-100" : "opacity-0",
                  )}
                />
                {t("autoMode.modelAny", { defaultValue: "不限模型" })}
              </CommandItem>
              {modelOptions.map((option) => (
                <CommandItem
                  key={option.model}
                  value={option.model}
                  onSelect={handleSelect}
                >
                  <Check
                    className={cn(
                      "mr-2 h-4 w-4 shrink-0",
                      model === option.model ? "opacity-100" : "opacity-0",
                    )}
                  />
                  <span className="min-w-0 flex-1 truncate">
                    {option.model}
                  </span>
                  <span className="ml-2 shrink-0 text-xs text-muted-foreground tabular-nums">
                    {meta(option)}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
