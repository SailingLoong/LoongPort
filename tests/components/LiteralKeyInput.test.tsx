import { createRef } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import {
  LiteralKeyInput,
  type LiteralKeyInputHandle,
} from "@/components/relay/LiteralKeyInput";

function mount() {
  const ref = createRef<LiteralKeyInputHandle>();
  const changed = vi.fn();
  const view = render(
    <LiteralKeyInput ref={ref} aria-label="key" onValueChange={changed} />,
  );
  return {
    ref,
    changed,
    input: screen.getByRole("textbox", { name: "key" }) as HTMLTextAreaElement,
    ...view,
  };
}
function paste(input: HTMLElement, text: string) {
  fireEvent.paste(input, {
    clipboardData: {
      getData: (format: string) => (format === "text/plain" ? text : ""),
    },
  });
}
it.each([
  "synthetic\nkey",
  "synthetic\r\nkey",
  "\r\nkey\r\n",
  "\rkey\r",
  "\r\n",
  " 🔑\t中文\r\n ",
])(
  "retains exact clipboard text despite textarea line-ending display rules (%j)",
  (text) => {
    const { ref, input } = mount();
    paste(input, text);
    expect(ref.current?.value).toBe(text);
    expect(input.value).toBe(text.replace(/\r\n?/g, "\n"));
    expect(input.outerHTML).not.toContain(text);
    expect(input).toHaveClass(
      "[-webkit-text-security:disc]",
      "text-transparent",
      "selection:text-transparent",
      "supports-[-webkit-text-security:disc]:text-foreground",
    );
  },
);
it("preserves prior CRLF while a user types after pasted text", async () => {
  const user = userEvent.setup();
  const { ref, input } = mount();
  input.focus();
  paste(input, "first\r\nsecond");
  await user.keyboard("X");
  expect(ref.current?.value).toBe("first\r\nsecondX");
});
it.each([
  ["\r\n\n", "\n"],
  ["\n\r\n", "\r\n"],
])(
  "deletes the intended repeated newline without rewriting the surviving one (%j)",
  async (text, expected) => {
    const user = userEvent.setup();
    const { ref, input } = mount();
    input.focus();
    paste(input, text);
    input.setSelectionRange(1, 1);
    await user.keyboard("{Backspace}");
    expect(ref.current?.value).toBe(expected);
  },
);
it("preserves untouched line endings during selection replacement and keyboard Enter", async () => {
  const user = userEvent.setup();
  const { ref, input } = mount();
  input.focus();
  paste(input, "ab\r\ncd\r\nef");
  input.setSelectionRange(3, 5);
  await user.keyboard("中{Enter}");
  expect(ref.current?.value).toBe("ab\r\n中\n\r\nef");
});
it("clears both the short-lived original and the rendered field on clear and unmount", () => {
  const { ref, input, unmount } = mount();
  paste(input, "secret\r\n🔑");
  const handle = ref.current!;
  handle.value = "";
  expect(handle.value).toBe("");
  expect(input.value).toBe("");
  paste(input, "other\r\nsecret");
  unmount();
  expect(handle.value).toBe("");
  expect(input.value).toBe("");
});

it.each([
  ["\r\n\n", "\n"],
  ["\n\r\n", "\r\n"],
])(
  "Delete removes the intended newline while preserving the next one (%j)",
  async (text, expected) => {
    const user = userEvent.setup();
    const { ref, input } = mount();
    input.focus();
    paste(input, text);
    input.setSelectionRange(0, 0);
    await user.keyboard("{Delete}");
    expect(ref.current?.value).toBe(expected);
  },
);
it("cuts only the selected text and leaves original CRLF outside it intact", async () => {
  const user = userEvent.setup();
  const { ref, input } = mount();
  input.focus();
  paste(input, "first\r\n中🔑\r\nlast");
  input.setSelectionRange(6, 9);
  await user.cut();
  expect(ref.current?.value).toBe("first\r\n\r\nlast");
});
it("uses the native Unicode text and replaces a whole emoji without introducing mask characters", async () => {
  const user = userEvent.setup();
  const { ref, input } = mount();
  input.focus();
  paste(input, "🔑\r\ne\u0301");
  input.setSelectionRange(0, 2);
  await user.keyboard("中");
  expect(ref.current?.value).toBe("中\r\ne\u0301");
  expect(ref.current?.value).not.toContain("•");
});
it("preserves exact line endings through explicit undo and redo, then clears that transient history", async () => {
  const user = userEvent.setup();
  const { ref, input } = mount();
  input.focus();
  paste(input, "head\r\ntail");
  await user.keyboard("X");
  await user.keyboard("{Control>}z{/Control}");
  expect(ref.current?.value).toBe("head\r\ntail");
  await user.keyboard("{Control>}y{/Control}");
  expect(ref.current?.value).toBe("head\r\ntailX");
  ref.current!.value = "";
  await user.keyboard("{Control>}z{/Control}");
  expect(ref.current?.value).toBe("");
  expect(input.value).toBe("");
});
it.each(["input-first", "composition-end-first"])(
  "does not duplicate or drop composition text with %s final events",
  (order) => {
    const { ref, input, changed } = mount();
    input.focus();
    paste(input, "left\r\nright");
    input.setSelectionRange(5, 5);
    changed.mockClear();
    fireEvent.compositionStart(input);
    fireEvent.input(input, {
      target: { value: "left\n中right", selectionStart: 6, selectionEnd: 6 },
      inputType: "insertCompositionText",
      isComposing: true,
    });
    expect(changed).not.toHaveBeenCalled();
    const finalInput = () =>
      fireEvent.input(input, {
        target: {
          value: "left\n中文right",
          selectionStart: 7,
          selectionEnd: 7,
        },
        inputType: "insertCompositionText",
        isComposing: false,
      });
    const end = () =>
      fireEvent.compositionEnd(input, {
        target: { value: "left\n中文right" },
        data: "中文",
      });
    if (order === "input-first") {
      finalInput();
      end();
    } else {
      end();
      finalInput();
    }
    expect(ref.current?.value).toBe("left\r\n中文right");
    expect(changed).toHaveBeenCalledTimes(1);
  },
);
it("collapses the selection after replacing text with an identical paste", () => {
  const { ref, input } = mount();
  paste(input, "same\r\ntext");
  input.select();
  paste(input, "same\r\ntext");
  expect(ref.current?.value).toBe("same\r\ntext");
  expect(input.selectionStart).toBe(input.value.length);
  expect(input.selectionEnd).toBe(input.value.length);
});
it("handles native history input events without normalizing the restored original", () => {
  const { ref, input } = mount();
  paste(input, "one\r\ntwo");
  input.select();
  paste(input, "replacement");
  fireEvent(
    input,
    new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "historyUndo",
    }),
  );
  expect(ref.current?.value).toBe("one\r\ntwo");
  fireEvent(
    input,
    new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "historyRedo",
    }),
  );
  expect(ref.current?.value).toBe("replacement");
});

it("blocks drag/drop rather than allowing the browser to rewrite original line endings", () => {
  const { ref, input } = mount();
  paste(input, "keep\r\noriginal");
  input.select();
  const allowed = fireEvent.drop(input, {
    dataTransfer: { getData: () => "drop\r\ntext" },
  });
  expect(allowed).toBe(false);
  expect(ref.current?.value).toBe("keep\r\noriginal");
  expect(input.value).toBe("keep\noriginal");
});
