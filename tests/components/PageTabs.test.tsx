import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PageTabs } from "@/components/ui/page-tabs";
import { SegmentedControl } from "@/components/ui/segmented-control";

it("navigates tabs with arrows, Home and End, skipping disabled tabs and linking the panel", async () => {
  function Example() {
    const [value, setValue] = useState("connection");
    return (
      <>
        <PageTabs
          aria-label="Preview sections"
          idPrefix="preview"
          controls="preview-panel"
          value={value}
          onValueChange={setValue}
          items={[
            { value: "connection", label: "Connection" },
            { value: "unavailable", label: "Unavailable", disabled: true },
            { value: "files", label: "Files" },
          ]}
        />
        <div
          id="preview-panel"
          role="tabpanel"
          aria-labelledby={`preview-${value}`}
        >
          {value}
        </div>
      </>
    );
  }
  render(<Example />);
  const user = userEvent.setup();
  await user.tab();
  const connection = screen.getByRole("tab", { name: "Connection" });
  const files = screen.getByRole("tab", { name: "Files" });
  expect(connection).toHaveFocus();
  expect(connection).toHaveAttribute("aria-controls", "preview-panel");
  await user.keyboard("{ArrowRight}");
  expect(files).toHaveFocus();
  expect(files).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("tabpanel", { name: "Files" })).toHaveTextContent(
    "files",
  );
  await user.keyboard("{ArrowRight}");
  expect(connection).toHaveFocus();
  await user.keyboard("{ArrowLeft}");
  expect(files).toHaveFocus();
  await user.keyboard("{Home}");
  expect(connection).toHaveFocus();
  await user.keyboard("{End}");
  expect(files).toHaveFocus();
  expect(screen.getByRole("tab", { name: "Unavailable" })).toBeDisabled();
});

it("keeps segmented filters as pressed buttons rather than page tabs", async () => {
  const change = vi.fn();
  render(
    <SegmentedControl
      aria-label="Filter"
      value="all"
      onValueChange={change}
      items={[
        { value: "all", label: "All" },
        { value: "enabled", label: "Enabled" },
        { value: "locked", label: "Locked", disabled: true },
      ]}
    />,
  );
  expect(screen.getByRole("group", { name: "Filter" })).toBeInTheDocument();
  expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "All" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await userEvent.click(screen.getByRole("button", { name: "Enabled" }));
  await userEvent.click(screen.getByRole("button", { name: "Locked" }));
  expect(change).toHaveBeenCalledExactlyOnceWith("enabled");
});
