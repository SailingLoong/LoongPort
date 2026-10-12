import { act, render, screen } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";

import JsonEditor from "@/components/JsonEditor";

describe("JsonEditor", () => {
  it("can disable JSON formatting while retaining exact editable text", () => {
    const onChange = vi.fn();
    const value = '{"large":9007199254740993}';
    const { container } = render(
      <JsonEditor
        value={value}
        onChange={onChange}
        language="json"
        showFormat={false}
        showValidation={false}
      />,
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    const view = EditorView.findFromDOM(
      container.querySelector(".cm-content") as HTMLElement,
    );
    expect(view?.state.doc.toString()).toBe(value);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("preserves the existing JSON formatter by default", () => {
    render(<JsonEditor value="{}" onChange={vi.fn()} />);
    expect(screen.getByRole("button")).toBeVisible();
  });

  it("updates height and callbacks without recreating the editor view", () => {
    const firstOnChange = vi.fn();
    const secondOnChange = vi.fn();
    const { container, rerender } = render(
      <JsonEditor
        id="configuration-json"
        ariaLabel="Configuration JSON"
        value="{}"
        onChange={firstOnChange}
        height={60}
      />,
    );

    const content = container.querySelector(".cm-content");
    expect(content).not.toBeNull();
    const originalView = EditorView.findFromDOM(content as HTMLElement);
    expect(originalView).toBeDefined();
    expect(content).toHaveAttribute("aria-label", "Configuration JSON");

    rerender(
      <JsonEditor
        id="configuration-json"
        ariaLabel="Configuration JSON"
        value="{}"
        onChange={secondOnChange}
        height={120}
      />,
    );

    const currentContent = container.querySelector(".cm-content");
    const currentView = EditorView.findFromDOM(currentContent as HTMLElement);
    expect(currentView).toBe(originalView);
    expect(container.querySelector("#configuration-json")).toHaveStyle({
      height: "120px",
    });

    act(() => {
      currentView?.dispatch({
        changes: {
          from: 0,
          to: currentView.state.doc.length,
          insert: '{"changed":true}',
        },
      });
    });

    expect(firstOnChange).not.toHaveBeenCalled();
    expect(secondOnChange).toHaveBeenLastCalledWith('{"changed":true}');
  });

  it("keeps the cursor near the edited region when external normalization adds text", () => {
    const original = '{\n  "a": 1,\n  "b": 2\n}';
    const normalized = '{\n  "a": 1,\n  "added": true,\n  "b": 2\n}';
    const { container, rerender } = render(
      <JsonEditor value={original} onChange={vi.fn()} />,
    );
    const content = container.querySelector(".cm-content");
    const view = EditorView.findFromDOM(content as HTMLElement);
    const originalCursor = original.indexOf('"b"') + 1;

    act(() => {
      view?.dispatch({ selection: { anchor: originalCursor } });
    });
    rerender(<JsonEditor value={normalized} onChange={vi.fn()} />);

    expect(view?.state.selection.main.head).toBe(normalized.indexOf('"b"') + 1);
  });

  it("keeps the cursor on unchanged context between separate external edits", () => {
    const original = '{"a":1,"b":2,"c":3}';
    const normalized = '{"a":100,"b":2,"c":300}';
    const { container, rerender } = render(
      <JsonEditor value={original} onChange={vi.fn()} />,
    );
    const content = container.querySelector(".cm-content");
    const view = EditorView.findFromDOM(content as HTMLElement);
    const originalCursor = original.indexOf('"b"') + 1;

    act(() => {
      view?.dispatch({ selection: { anchor: originalCursor } });
    });
    rerender(<JsonEditor value={normalized} onChange={vi.fn()} />);

    expect(view?.state.selection.main.head).toBe(normalized.indexOf('"b"') + 1);
  });
});
