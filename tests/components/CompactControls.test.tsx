import {
  Sheet,
  SheetTrigger,
  SheetContent,
  SheetTitle,
  SheetDescription,
} from "@/components/ui/sheet";
import {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SearchField } from "@/components/ui/search-field";

it("clears search with Escape before bubbling a second Escape and restores input focus after clear", async () => {
  const onEscape = vi.fn();
  function Example() {
    const [value, setValue] = useState("sample");
    return (
      <div
        onKeyDown={(event) => {
          if (event.key === "Escape") onEscape();
        }}
      >
        <SearchField
          aria-label="Search tiers"
          clearLabel="Clear search"
          value={value}
          onValueChange={setValue}
        />
      </div>
    );
  }
  render(<Example />);
  const input = screen.getByRole("textbox", { name: "Search tiers" });
  await userEvent.click(input);
  await userEvent.keyboard("{Escape}");
  expect(input).toHaveValue("");
  expect(onEscape).not.toHaveBeenCalled();
  await userEvent.keyboard("{Escape}");
  expect(onEscape).toHaveBeenCalledTimes(1);
  await userEvent.type(input, "again");
  await userEvent.click(screen.getByRole("button", { name: "Clear search" }));
  expect(input).toHaveValue("");
  expect(input).toHaveFocus();
});

it.each(["disabled", "readOnly"] as const)(
  "keeps a %s search immutable for clear controls and Escape",
  async (state) => {
    const change = vi.fn();
    render(
      <SearchField
        aria-label="Immutable search"
        clearLabel="Clear search"
        value="protected value"
        onValueChange={change}
        {...{ [state]: true }}
      />,
    );
    const input = screen.getByRole("textbox", { name: "Immutable search" });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(change).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "Clear search" }),
    ).not.toBeInTheDocument();
    expect(input).toHaveValue("protected value");
  },
);

it.each(["Sheet", "Dialog"])(
  "clears a populated search before dismissing its real %s on the next Escape",
  async (kind) => {
    const onKeyDown = vi.fn();
    function Example() {
      const [value, setValue] = useState("sample");
      const Root = kind === "Sheet" ? Sheet : Dialog;
      const Trigger = kind === "Sheet" ? SheetTrigger : DialogTrigger;
      const content = (
        <SearchField
          aria-label="Drawer search"
          clearLabel="Clear search"
          value={value}
          onValueChange={setValue}
          onKeyDown={onKeyDown}
        />
      );
      return (
        <Root>
          <Trigger>Open search</Trigger>
          {kind === "Sheet" ? (
            <SheetContent closeLabel="Close search">
              <SheetTitle>Search</SheetTitle>
              <SheetDescription>Synthetic search</SheetDescription>
              {content}
            </SheetContent>
          ) : (
            <DialogContent>
              <DialogTitle>Search</DialogTitle>
              <DialogDescription>Synthetic search</DialogDescription>
              {content}
            </DialogContent>
          )}
        </Root>
      );
    }
    render(<Example />);
    const trigger = screen.getByRole("button", { name: "Open search" });
    await userEvent.click(trigger);
    const input = screen.getByRole("textbox", { name: "Drawer search" });
    await userEvent.keyboard("a");
    expect(onKeyDown).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(input).toHaveValue("");
    expect(input).toHaveFocus();
    expect(onKeyDown).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  },
);
