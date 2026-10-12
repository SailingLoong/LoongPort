import { useState } from "react";
import userEvent from "@testing-library/user-event";
import { PreservedView } from "@/components/ui/PreservedView";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  Sheet,
  SheetTrigger,
  SheetClose,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";

function renderSheet(props: { dismissOnOutsideClick?: boolean } = {}) {
  const onOpenChange = vi.fn();
  render(
    <>
      <button type="button">outside</button>
      <Sheet open onOpenChange={onOpenChange}>
        <SheetContent closeLabel="close" {...props}>
          <SheetHeader>
            <SheetTitle>Drawer</SheetTitle>
            <SheetDescription>body</SheetDescription>
          </SheetHeader>
          <input aria-label="draft" />
        </SheetContent>
      </Sheet>
    </>,
  );
  return { onOpenChange };
}

/** Radix 在下一帧才开始听外部 pointerdown：先让它挂上，再点遮罩外面 */
async function clickOutside() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  await act(async () => {
    const target = screen.getByRole("button", {
      name: "outside",
      hidden: true,
    });
    fireEvent.pointerDown(target, { button: 0, pointerType: "mouse" });
    fireEvent.click(target);
  });
}

describe("SheetContent", () => {
  it("ignores clicks on the overlay by default so drafts aren't lost", async () => {
    const { onOpenChange } = renderSheet();
    await clickOutside();
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("closes on an outside click only when the drawer opts in", async () => {
    const { onOpenChange } = renderSheet({ dismissOnOutsideClick: true });
    await clickOutside();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

function DraftSheet() {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger>Open options</SheetTrigger>
      <SheetContent closeLabel="Close options">
        <SheetTitle>Options</SheetTitle>
        <SheetDescription>Local draft options</SheetDescription>
        <input
          aria-label="Option draft"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        <SheetClose>Cancel</SheetClose>
      </SheetContent>
    </Sheet>
  );
}

it.each(["Cancel", "Escape", "Close options"])(
  "closes with %s and restores focus to its trigger",
  async (method) => {
    render(<DraftSheet />);
    const user = userEvent.setup();
    const trigger = screen.getByRole("button", { name: "Open options" });
    await user.click(trigger);
    expect(screen.getByRole("dialog")).toHaveClass("z-[80]");
    expect(
      document.querySelector('[data-state="open"].bg-overlay'),
    ).toHaveClass("z-[80]");
    if (method === "Escape") await user.keyboard("{Escape}");
    else await user.click(screen.getByRole("button", { name: method }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  },
);

it("retains drafts but releases portal focus and pointer locks when its page is inactive", async () => {
  const view = (active: boolean) => (
    <>
      <button>Other page</button>
      <PreservedView active={active}>
        <DraftSheet />
      </PreservedView>
    </>
  );
  const { rerender } = render(view(true));
  await userEvent.click(screen.getByRole("button", { name: "Open options" }));
  await userEvent.type(
    screen.getByRole("textbox", { name: "Option draft" }),
    "unsaved option",
  );
  rerender(view(false));
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(document.body.style.pointerEvents).not.toBe("none");
  await userEvent.click(screen.getByRole("button", { name: "Other page" }));
  rerender(view(true));
  await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
  expect(screen.getByRole("textbox", { name: "Option draft" })).toHaveValue(
    "unsaved option",
  );
});

it("hides closing portal DOM when a preserved page changes before the exit animation finishes", async () => {
  const style = document.createElement("style");
  style.textContent =
    '[data-state="closed"] { animation-name: exit; animation-duration: 30s; }';
  document.head.append(style);
  function AnimatedSheet() {
    const [active, setActive] = useState(true);
    const [open, setOpen] = useState(true);
    return (
      <PreservedView active={active}>
        <Sheet open={open} onOpenChange={setOpen}>
          <SheetContent closeLabel="Close options">
            <SheetTitle>Options</SheetTitle>
            <SheetDescription>Local draft options</SheetDescription>
            <button
              onClick={() => {
                setOpen(false);
                setActive(false);
              }}
            >
              Continue
            </button>
          </SheetContent>
        </Sheet>
      </PreservedView>
    );
  }
  try {
    render(<AnimatedSheet />);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    const retainedNodes = document.querySelectorAll(
      '.fixed[data-state="closed"]',
    );
    expect(retainedNodes).toHaveLength(2);
    for (const node of retainedNodes) {
      expect(node).toHaveAttribute("hidden");
      expect(node).toHaveClass("!hidden");
      expect(node).not.toBeVisible();
    }
  } finally {
    style.remove();
  }
});
