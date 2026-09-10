import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { FirstVisitDomainDialog } from "@/components/relay/directory/FirstVisitDomainDialog";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
describe("FirstVisitDomainDialog", () => {
  it("requires actual input and preserves it after closing and reopening", async () => {
    const user = userEvent.setup();
    const submit = vi.fn();
    const dismiss = vi.fn();
    const props = { open: true, onSubmit: submit, onDismiss: dismiss };
    const { rerender } = render(<FirstVisitDomainDialog {...props} />);
    expect(screen.getByTestId("first-site-confirm")).toBeDisabled();
    await user.type(screen.getByRole("textbox"), "panel.example");
    await user.keyboard("{Escape}");
    expect(submit).not.toHaveBeenCalled();
    rerender(<FirstVisitDomainDialog {...props} open={false} />);
    rerender(<FirstVisitDomainDialog {...props} />);
    expect(screen.getByRole("textbox")).toHaveValue("panel.example");
    await user.click(screen.getByTestId("first-site-confirm"));
    expect(submit).toHaveBeenCalledWith("panel.example");
  });
});
