import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { VendorConnectionReviewDialog } from "@/components/relay/VendorConnectionReviewDialog";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { name?: string }) =>
      key + (options?.name ? ` ${options.name}` : ""),
  }),
}));
describe("explicit vendor recovery review", () => {
  it("shows the new target and uncertainty separately, with no action on open", () => {
    const confirm = vi.fn(),
      cancel = vi.fn();
    render(
      <VendorConnectionReviewDialog
        targetName="Account B"
        onConfirm={confirm}
        onCancel={cancel}
      />,
    );
    expect(screen.getByRole("dialog")).toHaveTextContent(
      "applications.vendorRecovery.description Account B",
    );
    expect(confirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(confirm).not.toHaveBeenCalled();
  });
  it("submits only the explicit confirm action and disables it while working", () => {
    const confirm = vi.fn();
    const { rerender } = render(
      <VendorConnectionReviewDialog
        targetName="Account B"
        onConfirm={confirm}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "applications.vendorRecovery.confirm",
      }),
    );
    expect(confirm).toHaveBeenCalledTimes(1);
    rerender(
      <VendorConnectionReviewDialog
        targetName="Account B"
        disabled
        onConfirm={confirm}
        onCancel={() => {}}
      />,
    );
    expect(
      screen.getByRole("button", {
        name: "applications.vendorRecovery.confirm",
      }),
    ).toBeDisabled();
  });
  it("has no visible review when there is no verified target", () => {
    render(
      <VendorConnectionReviewDialog
        targetName={null}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
