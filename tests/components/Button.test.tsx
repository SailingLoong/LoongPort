import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

it("uses the shared compact action and input styles while retaining toggle compatibility", () => {
  render(
    <>
      <Button size="compact">Apply</Button>
      <Button variant="toggle" aria-pressed>
        View
      </Button>
      <Input aria-label="Name" />
    </>,
  );
  expect(screen.getByRole("button", { name: "Apply" })).toHaveClass(
    "h-7",
    "bg-action",
    "text-action-fg",
  );
  expect(screen.getByRole("button", { name: "View" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  expect(screen.getByRole("textbox", { name: "Name" })).toHaveClass(
    "h-8",
    "border-border-strong",
    "focus:border-ring",
  );
});

it("does not invoke a disabled or loading action", async () => {
  const onClick = vi.fn();
  render(
    <Button disabled aria-busy onClick={onClick}>
      Applying
    </Button>,
  );
  await userEvent.click(screen.getByRole("button", { name: "Applying" }));
  expect(onClick).not.toHaveBeenCalled();
  expect(screen.getByRole("button")).toHaveAttribute("aria-busy", "true");
});
