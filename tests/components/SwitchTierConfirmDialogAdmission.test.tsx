import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";

it("locks an already open confirmation when admission changes but keeps cancel available", async () => {
  const onSwitch = vi.fn();
  const onCancel = vi.fn();
  const props = { targetName: "Target", onSwitch, onCancel };
  const { rerender } = render(<SwitchTierConfirmDialog {...props} />);
  rerender(<SwitchTierConfirmDialog {...props} disabled />);
  const buttons = screen.getAllByRole("button");
  const confirms = buttons.filter((button) => button.hasAttribute("disabled"));
  expect(confirms).toHaveLength(2);
  for (const button of confirms) await userEvent.click(button);
  expect(onSwitch).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(onCancel).toHaveBeenCalledOnce();
});
