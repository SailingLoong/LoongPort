import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import { Textarea, type TextareaProps } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

export interface LiteralKeyInputHandle {
  value: string;
}
type Props = Omit<TextareaProps, "value" | "defaultValue" | "onChange"> & {
  onValueChange: () => void;
};
type Selection = {
  start: number;
  end: number;
  display: string;
  inputType: string;
};
type Draft = { text: string; start: number; end: number };

// This is only the native textarea's display projection, never the submitted key.
const displayText = (raw: string) => raw.replace(/\r\n?/g, "\n");
function rawOffset(raw: string, displayOffset: number) {
  let rawIndex = 0;
  for (
    let displayed = 0;
    displayed < displayOffset && rawIndex < raw.length;
    displayed++
  ) {
    rawIndex += raw[rawIndex] === "\r" && raw[rawIndex + 1] === "\n" ? 2 : 1;
  }
  return rawIndex;
}
function editOriginal(
  raw: string,
  next: string,
  selection: Selection | null,
  inputType: string,
) {
  const previous = displayText(raw);
  if (selection?.display === previous) {
    let { start, end } = selection;
    const removed = previous.length - next.length;
    if (start === end && removed > 0) {
      if (inputType.endsWith("Backward")) start = Math.max(0, start - removed);
      if (inputType.endsWith("Forward"))
        end = Math.min(previous.length, end + removed);
    }
    const prefix = previous.slice(0, start);
    const suffix = previous.slice(end);
    if (
      next.startsWith(prefix) &&
      next.endsWith(suffix) &&
      next.length >= prefix.length + suffix.length
    ) {
      return (
        raw.slice(0, rawOffset(raw, start)) +
        next.slice(prefix.length, next.length - suffix.length) +
        raw.slice(rawOffset(raw, end))
      );
    }
  }
  // Non-keyboard edits (e.g. input methods) still retain untouched original spans.
  let start = 0;
  while (
    start < previous.length &&
    start < next.length &&
    previous[start] === next[start]
  )
    start++;
  let tail = 0;
  while (
    tail < previous.length - start &&
    tail < next.length - start &&
    previous[previous.length - 1 - tail] === next[next.length - 1 - tail]
  )
    tail++;
  return (
    raw.slice(0, rawOffset(raw, start)) +
    next.slice(start, next.length - tail) +
    raw.slice(rawOffset(raw, previous.length - tail))
  );
}

/** A masked native textarea with a short-lived original-text owner for CR/LF fidelity. */
export const LiteralKeyInput = forwardRef<LiteralKeyInputHandle, Props>(
  ({ onValueChange, className, ...props }, ref) => {
    const element = useRef<HTMLTextAreaElement>(null);
    const original = useRef("");
    const selection = useRef<Selection | null>(null);
    const composing = useRef(false);
    // Undo belongs to this transient input lifetime only, never to application caches.
    const undo = useRef<Draft[]>([]);
    const redo = useRef<Draft[]>([]);
    const capture = (inputType = "") => {
      const field = element.current;
      if (!field) return;
      selection.current = {
        start: field.selectionStart,
        end: field.selectionEnd,
        display: displayText(original.current),
        inputType,
      };
    };
    const currentDraft = (): Draft => ({
      text: original.current,
      start: selection.current?.start ?? element.current?.selectionStart ?? 0,
      end: selection.current?.end ?? element.current?.selectionEnd ?? 0,
    });
    const show = (text: string, start?: number, end = start) => {
      original.current = text;
      const field = element.current;
      if (field) {
        const display = displayText(text);
        if (field.value !== display) field.value = display;
        if (start !== undefined) field.setSelectionRange(start, end ?? start);
      }
      selection.current = null;
    };
    const commit = (text: string, start?: number, end = start) => {
      if (text === original.current) {
        show(text, start, end);
        return false;
      }
      undo.current.push(currentDraft());
      redo.current = [];
      show(text, start, end);
      if (!composing.current) onValueChange();
      return true;
    };
    const restore = (backward: boolean) => {
      const from = backward ? undo.current : redo.current;
      const to = backward ? redo.current : undo.current;
      const draft = from.pop();
      if (!draft) return;
      selection.current = null;
      to.push(currentDraft());
      show(draft.text, draft.start, draft.end);
      onValueChange();
    };
    useImperativeHandle(
      ref,
      () => ({
        get value() {
          return original.current;
        },
        set value(text: string) {
          original.current = text;
          if (element.current) element.current.value = displayText(text);
          undo.current = [];
          redo.current = [];
          selection.current = null;
        },
      }),
      [],
    );
    useEffect(() => {
      const field = element.current;
      const beforeInput = (event: InputEvent) => {
        if (
          event.inputType === "historyUndo" ||
          event.inputType === "historyRedo"
        ) {
          event.preventDefault();
          restore(event.inputType === "historyUndo");
        } else capture(event.inputType);
      };
      field?.addEventListener("beforeinput", beforeInput);
      return () => field?.removeEventListener("beforeinput", beforeInput);
    });
    useEffect(() => {
      const field = element.current;
      return () => {
        original.current = "";
        undo.current = [];
        redo.current = [];
        selection.current = null;
        if (field) field.value = "";
      };
    }, []);
    return (
      <Textarea
        {...props}
        ref={element}
        rows={2}
        className={cn(
          "min-h-[64px] text-transparent selection:text-transparent caret-foreground [-webkit-text-security:disc] supports-[-webkit-text-security:disc]:text-foreground supports-[-webkit-text-security:disc]:selection:text-foreground",
          className,
        )}
        autoComplete="off"
        onKeyDown={(event) => {
          const shortcut = (event.ctrlKey || event.metaKey) && !event.altKey;
          if (shortcut && ["z", "y"].includes(event.key.toLowerCase())) {
            event.preventDefault();
            restore(event.key.toLowerCase() === "z" && !event.shiftKey);
            return;
          }
          capture(
            event.key === "Backspace"
              ? "deleteContentBackward"
              : event.key === "Delete"
                ? "deleteContentForward"
                : "",
          );
          props.onKeyDown?.(event);
        }}
        onCut={(event) => {
          capture("deleteByCut");
          props.onCut?.(event);
        }}
        // Native drop has no cross-WebView raw-text insertion contract.
        // Ignore it rather than silently normalizing the supplied key.
        onDrop={(event) => event.preventDefault()}
        onDragOver={(event) => event.preventDefault()}
        onPaste={(event) => {
          event.preventDefault();
          const text = event.clipboardData.getData("text/plain");
          const field = event.currentTarget;
          capture("insertFromPaste");
          const { selectionStart: start, selectionEnd: end } = field;
          const raw = original.current;
          commit(
            raw.slice(0, rawOffset(raw, start)) +
              text +
              raw.slice(rawOffset(raw, end)),
            start + displayText(text).length,
          );
        }}
        onCompositionStart={() => {
          capture("insertCompositionText");
          composing.current = true;
        }}
        onCompositionEnd={(event) => {
          const next = editOriginal(
            original.current,
            event.currentTarget.value,
            selection.current,
            "insertCompositionText",
          );
          commit(next);
          composing.current = false;
          onValueChange();
        }}
        onChange={(event) => {
          const inputType =
            (event.nativeEvent as InputEvent).inputType ||
            selection.current?.inputType ||
            "";
          const next = editOriginal(
            original.current,
            event.currentTarget.value,
            selection.current,
            inputType,
          );
          commit(
            next,
            event.currentTarget.selectionStart,
            event.currentTarget.selectionEnd,
          );
        }}
      />
    );
  },
);
LiteralKeyInput.displayName = "LiteralKeyInput";
