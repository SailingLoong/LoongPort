import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HelpTip, DisabledReason } from "@/components/ui/help-tip";

const explanation = "Changes remain local until applied.";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("offers a persistent accessible explanation and opens on keyboard focus without stealing it", async () => {
  render(<HelpTip title="Draft changes">{explanation}</HelpTip>);
  const trigger = screen.getByRole("button", { name: "Draft changes" });
  expect(trigger).toHaveAccessibleDescription(explanation);
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  await userEvent.tab();
  expect(trigger).toHaveFocus();
  expect(screen.getByRole("tooltip")).toHaveTextContent(explanation);
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  expect(trigger).toHaveFocus();
});

it("opens after pointer dwell, stays open over the card, and dismisses after leaving", () => {
  vi.useFakeTimers();
  render(<HelpTip title="Draft changes">{explanation}</HelpTip>);
  const trigger = screen.getByRole("button", { name: "Draft changes" });
  fireEvent.pointerEnter(trigger);
  act(() => vi.advanceTimersByTime(299));
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  act(() => vi.advanceTimersByTime(1));
  fireEvent.pointerLeave(trigger);
  fireEvent.pointerEnter(screen.getByRole("tooltip"));
  act(() => vi.advanceTimersByTime(300));
  expect(screen.getByRole("tooltip")).toBeInTheDocument();
  fireEvent.pointerLeave(screen.getByRole("tooltip"));
  act(() => vi.advanceTimersByTime(200));
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
});

it("pins a clicked explanation until Escape", async () => {
  render(<HelpTip title="Draft changes">{explanation}</HelpTip>);
  const trigger = screen.getByRole("button", { name: "Draft changes" });
  await userEvent.click(trigger);
  fireEvent.pointerLeave(trigger);
  expect(screen.getByRole("tooltip")).toBeInTheDocument();
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
});

it("explains a disabled button on focus and suppresses pointer, Enter and Space actions", async () => {
  const onAction = vi.fn();
  render(
    <DisabledReason reason="This action is blocked.">
      <button onClick={onAction}>Apply</button>
    </DisabledReason>,
  );
  const trigger = screen.getByRole("button", { name: "Apply" });
  // :focus-visible belongs to the focused button, never its non-focusable wrapper.
  const matches = Element.prototype.matches;
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (
    this: Element,
    selector: string,
  ) {
    return selector === ":focus-visible"
      ? this === trigger
      : matches.call(this, selector);
  });
  await userEvent.tab();
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-disabled", "true");
  expect(trigger).toHaveAccessibleDescription("This action is blocked.");
  expect(screen.getByRole("tooltip")).toHaveTextContent(
    "This action is blocked.",
  );
  await userEvent.keyboard("{Enter} ");
  await userEvent.click(trigger);
  expect(onAction).not.toHaveBeenCalled();
});

it("leaves an enabled button unchanged when there is no reason", async () => {
  const onAction = vi.fn();
  render(
    <DisabledReason>
      <button onClick={onAction}>Apply</button>
    </DisabledReason>,
  );
  await userEvent.click(screen.getByRole("button", { name: "Apply" }));
  expect(onAction).toHaveBeenCalledTimes(1);
});
