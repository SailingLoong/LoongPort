import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { UniversalProviderFormModal } from "../UniversalProviderFormModal";
import { universalProviderPresets } from "@/config/universalProviderPresets";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
it("keeps an edited open draft across Activity hide and reveal", () => {
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <UniversalProviderFormModal isOpen onClose={vi.fn()} onSave={vi.fn()} />
    </PreservedView>
  );
  const view = render(tree(true));
  fireEvent.change(screen.getByLabelText("universalProvider.name"), {
    target: { value: "My draft" },
  });
  fireEvent.change(screen.getByLabelText("universalProvider.apiKey"), {
    target: { value: "ephemeral-key" },
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  expect(screen.getByLabelText("universalProvider.name")).toHaveValue(
    "My draft",
  );
  expect(screen.getByLabelText("universalProvider.apiKey")).toHaveValue(
    "ephemeral-key",
  );
});
it("resets fields when a modal actually closes and reopens", () => {
  const tree = (open: boolean) => (
    <UniversalProviderFormModal
      isOpen={open}
      onClose={vi.fn()}
      onSave={vi.fn()}
    />
  );
  const view = render(tree(true));
  fireEvent.change(screen.getByLabelText("universalProvider.name"), {
    target: { value: "Old draft" },
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  expect(screen.getByLabelText("universalProvider.name")).toHaveValue(
    universalProviderPresets[0].name,
  );
});
