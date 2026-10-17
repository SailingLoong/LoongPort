import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ModelDropdown } from "@/components/providers/forms/shared/ModelDropdown";

describe("ModelDropdown", () => {
  it("keeps an active free filter removable when a refreshed list has no pricing", () => {
    const onSelect = vi.fn();
    Element.prototype.scrollIntoView = vi.fn();
    const view = render(
      <ModelDropdown
        models={[{ id: "known-free", ownedBy: null, isFree: true }]}
        onSelect={onSelect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Select model" }));
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.freeOnly" }),
    );
    view.rerender(
      <ModelDropdown
        models={[{ id: "new-unknown", ownedBy: null }]}
        onSelect={onSelect}
      />,
    );
    const filter = screen.getByRole("button", {
      name: "providerForm.freeOnly",
    });
    expect(filter).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.queryByRole("option", { name: "new-unknown" }),
    ).not.toBeInTheDocument();
    fireEvent.click(filter);
    expect(screen.getByRole("option", { name: "new-unknown" })).toBeVisible();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("filters only backend-confirmed free models and preserves their exact ids", async () => {
    const onSelect = vi.fn();
    Element.prototype.scrollIntoView = vi.fn();
    render(
      <ModelDropdown
        models={[
          { id: "openrouter/free", ownedBy: "openrouter", isFree: true },
          { id: "vendor/model:free", ownedBy: "vendor", isFree: true },
          { id: "paid:free", ownedBy: "vendor", isFree: false },
          { id: "unknown", ownedBy: "vendor" },
        ]}
        onSelect={onSelect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Select model" }));
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.freeOnly" }),
    );
    expect(
      screen.getByRole("button", { name: "providerForm.freeOnly" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.queryByRole("option", { name: "paid:free" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "unknown" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /vendor\/model:free/ }));
    expect(onSelect).toHaveBeenCalledWith("vendor/model:free");
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
  it("exposes a labelled search input and filters by vendor", async () => {
    const onSelect = vi.fn();
    Element.prototype.scrollIntoView = vi.fn();

    render(
      <ModelDropdown
        models={[
          { id: "gpt-5", ownedBy: "openai" },
          { id: "claude-sonnet", ownedBy: "anthropic" },
        ]}
        onSelect={onSelect}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Select model" }));

    const searchInput = screen.getByRole("combobox", {
      name: "Search models...",
    });
    expect(searchInput).toBeInTheDocument();

    fireEvent.change(searchInput, { target: { value: "openai" } });

    await waitFor(() => {
      expect(screen.getByRole("option", { name: "gpt-5" })).toBeVisible();
      expect(
        screen.queryByRole("option", { name: "claude-sonnet" }),
      ).not.toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("option", { name: "gpt-5" }));
    expect(onSelect).toHaveBeenCalledWith("gpt-5");
  });
});
