import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { PreservedView } from "@/components/ui/PreservedView";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
function DraftForm() {
  const [value, setValue] = useState("");
  const [open, setOpen] = useState(false);
  return (
    <>
      <input
        aria-label="draft"
        value={value}
        onChange={(e) => setValue(e.target.value)}
      />
      <button onClick={() => setOpen(true)}>Open dialog</button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogTitle>Draft options</DialogTitle>
          <input aria-label="dialog draft" />
        </DialogContent>
      </Dialog>
    </>
  );
}
describe("PreservedView", () => {
  it("preserves form drafts while releasing Radix portals and focus locks on another page", async () => {
    const user = userEvent.setup();
    const view = (active: boolean) => (
      <>
        <button>Other page</button>
        <PreservedView active={active}>
          <DraftForm />
        </PreservedView>
      </>
    );
    const { rerender } = render(view(true));
    await user.type(
      screen.getByRole("textbox", { name: /^draft$/ }),
      "unsaved key",
    );
    await user.click(screen.getByRole("button", { name: "Open dialog" }));
    await user.type(
      screen.getByRole("textbox", { name: "dialog draft" }),
      "unsaved option",
    );
    rerender(view(false));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(document.body.style.pointerEvents).not.toBe("none");
    expect(
      screen.getByRole("button", { name: "Other page" }),
    ).not.toHaveAttribute("aria-hidden", "true");
    rerender(view(true));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "dialog draft" })).toHaveValue(
      "unsaved option",
    );
    await user.keyboard("{Escape}");
    expect(screen.getByRole("textbox", { name: /^draft$/ })).toHaveValue(
      "unsaved key",
    );
  });
  it("hides closing portal DOM even when its exit animation has not completed", async () => {
    const user = userEvent.setup();
    const style = document.createElement("style");
    style.textContent =
      '[data-state="closed"] { animation-name: exit; animation-duration: 30s; }';
    document.head.append(style);
    function AnimatedFlow() {
      const [active, setActive] = useState(true);
      const [open, setOpen] = useState(true);
      return (
        <PreservedView active={active}>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogContent>
              <DialogTitle>Animated options</DialogTitle>
              <button
                onClick={() => {
                  setOpen(false);
                  setActive(false);
                }}
              >
                Continue
              </button>
            </DialogContent>
          </Dialog>
        </PreservedView>
      );
    }
    try {
      render(<AnimatedFlow />);
      await user.click(screen.getByRole("button", { name: "Continue" }));
      for (const node of document.querySelectorAll('[data-state="closed"]')) {
        if (node instanceof HTMLElement && node.classList.contains("fixed")) {
          expect(node).toHaveAttribute("hidden");
          expect(node).toHaveClass("!hidden");
          expect(node).not.toBeVisible();
        }
      }
    } finally {
      style.remove();
    }
  });
});
