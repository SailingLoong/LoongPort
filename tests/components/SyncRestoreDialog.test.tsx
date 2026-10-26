import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SyncRestoreDialog } from "@/components/settings/SyncRestoreDialog";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const paste = (field: HTMLElement, text: string) =>
  fireEvent.paste(field, { clipboardData: { getData: () => text } });

describe("SyncRestoreDialog literal source input", () => {
  it("preserves mixed line endings through paste and a later edit, consumes once and clears before the pending reply", async () => {
    let finish!: () => void;
    const onRestore = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    render(
      <SyncRestoreDialog
        deviceName="Synthetic source"
        onRestore={onRestore}
        onClose={vi.fn()}
      />,
    );
    const field = screen.getByLabelText(
      "syncRestore.password",
    ) as HTMLTextAreaElement;
    const raw = "\r\n中文🔑 aB\rline\n ";
    paste(field, raw);
    field.setSelectionRange(field.value.length, field.value.length);
    fireEvent.keyDown(field, { key: "!" });
    fireEvent.input(field, {
      target: { value: field.value + "!" },
      inputType: "insertText",
    });
    fireEvent.submit(field.closest("form")!);
    fireEvent.submit(field.closest("form")!);
    expect(onRestore).toHaveBeenCalledTimes(1);
    expect(onRestore).toHaveBeenCalledWith(raw + "!");
    expect(field).toHaveValue("");
    expect(
      screen.getByRole("button", { name: "common.cancel" }),
    ).toBeDisabled();
    finish();
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "syncRestore.confirm" }),
      ).toBeDisabled(),
    );
  });

  it.each([" ", "1", "中文🔑", "aA", "e\u0301"])(
    "accepts original nonempty text %j without strength policy",
    async (raw) => {
      const onRestore = vi.fn().mockResolvedValue(undefined);
      render(
        <SyncRestoreDialog
          deviceName="Source"
          onRestore={onRestore}
          onClose={vi.fn()}
        />,
      );
      paste(screen.getByLabelText("syncRestore.password"), raw);
      fireEvent.click(
        screen.getByRole("button", { name: "syncRestore.confirm" }),
      );
      await waitFor(() => expect(onRestore).toHaveBeenCalledWith(raw));
    },
  );

  it("never submits an empty or cleared input and clears on cancel and unmount", () => {
    const onRestore = vi.fn(),
      onClose = vi.fn();
    const view = render(
      <SyncRestoreDialog
        deviceName="Source"
        onRestore={onRestore}
        onClose={onClose}
      />,
    );
    const field = screen.getByLabelText("syncRestore.password");
    fireEvent.submit(field.closest("form")!);
    expect(onRestore).not.toHaveBeenCalled();
    paste(field, "synthetic source key");
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(field).toHaveValue("");
    paste(field, "second synthetic key");
    view.unmount();
    expect(field).toHaveValue("");
  });

  it("uses static failure text, clears immediately and does not retry a changed snapshot", async () => {
    const raw = "synthetic\r\nsource";
    const onRestore = vi
      .fn()
      .mockRejectedValue(new Error(`sync.conflict ${raw}`));
    render(
      <SyncRestoreDialog
        deviceName="Source"
        onRestore={onRestore}
        onClose={vi.fn()}
      />,
    );
    const field = screen.getByLabelText("syncRestore.password");
    paste(field, raw);
    fireEvent.click(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "syncRestore.changed",
    );
    expect(document.body.textContent).not.toContain(raw);
    expect(field).toHaveValue("");
    expect(onRestore).toHaveBeenCalledOnce();
  });
});
