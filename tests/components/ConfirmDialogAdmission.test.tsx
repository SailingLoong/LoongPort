import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { ConfirmDialog } from "@/components/ConfirmDialog";
it("disables only submission when an open confirmation loses write admission", async () => {
  const onConfirm = vi.fn(),
    onCancel = vi.fn();
  const props = {
    isOpen: true,
    title: "Delete",
    message: "Confirm",
    onConfirm,
    onCancel,
  };
  const { rerender } = render(<ConfirmDialog {...props} />);
  rerender(<ConfirmDialog {...props} confirmDisabled />);
  expect(screen.getByRole("button", { name: "common.confirm" })).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(onCancel).toHaveBeenCalledOnce();
  expect(onConfirm).not.toHaveBeenCalled();
});
