import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Notice, NoticeSlot } from "@/components/ui/notice";

it("keeps the status slot mounted while an explicit action dismisses a notice", async () => {
  const onApply = vi.fn();
  function Example() {
    const [visible, setVisible] = useState(true);
    return (
      <NoticeSlot>
        {visible && (
          <Notice
            title="Draft changes"
            tone="warning"
            actions={<button onClick={onApply}>Apply</button>}
            onDismiss={() => setVisible(false)}
            dismissLabel="Dismiss draft notice"
          >
            Changes have not been applied.
          </Notice>
        )}
      </NoticeSlot>
    );
  }
  render(<Example />);
  const status = screen.getByRole("status");
  expect(status).toHaveTextContent("Changes have not been applied.");
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Apply" }));
  expect(onApply).toHaveBeenCalledTimes(1);
  expect(status).toHaveTextContent("Draft changes");
  await userEvent.click(
    screen.getByRole("button", { name: "Dismiss draft notice" }),
  );
  expect(screen.getByRole("status")).toBe(status);
  expect(status).toBeEmptyDOMElement();
  expect(onApply).toHaveBeenCalledTimes(1);
});

it("does not render dismissal or action buttons unless requested", () => {
  render(<Notice title="Read-only information" />);
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
});
