import * as React from "react";
import { Search, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { HoverTip } from "@/components/ui/hover-tip";
import { cn } from "@/lib/utils";

export interface SearchFieldProps extends Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange" | "type"
> {
  value: string;
  onValueChange: (value: string) => void;
  /** 清除按钮的名字（悬停提示 + aria-label） */
  clearLabel: string;
  /** 外层容器（定宽、外边距）；输入框本身的样式跟 Input 走 */
  containerClassName?: string;
}

/**
 * 搜索框（docs/design-system.html「输入」）：高 32、圆角 6、左侧 14px 放大镜，
 * 有字时右侧出现清除按钮；Esc 先清空，空了才交给外层（关抽屉 / 对话框）。
 */
export const SearchField = React.forwardRef<HTMLInputElement, SearchFieldProps>(
  (
    {
      value,
      onValueChange,
      clearLabel,
      containerClassName,
      className,
      onKeyDown,
      disabled,
      readOnly,
      ...props
    },
    ref,
  ) => {
    const inputRef = React.useRef<HTMLInputElement>(null);
    React.useImperativeHandle(ref, () => inputRef.current as HTMLInputElement);
    const editable = !disabled && !readOnly;
    React.useEffect(() => {
      if (!editable || !value) return;
      // Radix dismisses at document capture. Clear the focused search first,
      // then let an empty search's next Escape reach the owning overlay.
      const clearOnEscape = (event: KeyboardEvent) => {
        if (
          event.key !== "Escape" ||
          event.target !== inputRef.current ||
          event.isComposing ||
          event.defaultPrevented
        )
          return;
        event.preventDefault();
        event.stopPropagation();
        onValueChange("");
      };
      window.addEventListener("keydown", clearOnEscape, true);
      return () => window.removeEventListener("keydown", clearOnEscape, true);
    }, [editable, value, onValueChange]);
    return (
      <div role="search" className={cn("relative min-w-0", containerClassName)}>
        <Search
          aria-hidden="true"
          strokeWidth={1.5}
          className="pointer-events-none absolute start-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-fg-3"
        />
        <Input
          ref={inputRef}
          type="text"
          value={value}
          disabled={disabled}
          readOnly={readOnly}
          onChange={(event) => {
            if (editable) onValueChange(event.target.value);
          }}
          onKeyDown={onKeyDown}
          spellCheck={false}
          className={cn("pe-8 ps-8", className)}
          {...props}
        />
        {value && editable && (
          <HoverTip content={clearLabel}>
            <button
              type="button"
              onClick={() => {
                onValueChange("");
                inputRef.current?.focus();
              }}
              aria-label={clearLabel}
              className="absolute end-1 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-control text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
            >
              <X aria-hidden="true" className="h-3.5 w-3.5" strokeWidth={1.5} />
            </button>
          </HoverTip>
        )}
      </div>
    );
  },
);
SearchField.displayName = "SearchField";
