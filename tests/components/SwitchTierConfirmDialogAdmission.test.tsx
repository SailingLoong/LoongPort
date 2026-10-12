import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";

it("locks an already open confirmation when admission changes but keeps cancel available", async () => {
  const onSwitch = vi.fn();
  const onCancel = vi.fn();
  const props = { targetName: "Target", onSwitch, onCancel };
  const { rerender } = render(<SwitchTierConfirmDialog {...props} />);
  rerender(<SwitchTierConfirmDialog {...props} disabled />);
  const buttons = screen.getAllByRole("button");
  const confirms = buttons.filter((button) => button.hasAttribute("disabled"));
  expect(confirms).toHaveLength(2);
  for (const button of confirms) await userEvent.click(button);
  expect(onSwitch).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(onCancel).toHaveBeenCalledOnce();
});

const catalog = {
  ownership: "unclaimedExternal" as const,
  choice: "keep" as const,
  canManage: true,
  revision: "catalog-v1",
  beforePointer: "/external/models.json",
  afterPointer: "/external/models.json",
};

it("keeps an external catalog by default and separates catalog approval from quitting", async () => {
  const onSwitch = vi.fn();
  render(
    <SwitchTierConfirmDialog
      targetName="Target"
      onSwitch={onSwitch}
      onCancel={vi.fn()}
      catalog={catalog}
      quitRequired={false}
      onPreviewCatalog={vi.fn()}
    />,
  );
  expect(
    screen.getByRole("radio", { name: "provider.preview.catalog.keep" }),
  ).toBeChecked();
  expect(screen.getByText("provider.preview.catalog.keepHint")).toBeVisible();
  expect(screen.getAllByText("/external/models.json")).toHaveLength(2);
  expect(
    screen.queryByText("loongport.quitConfirm.body"),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", {
      name: "loongport.quitConfirm.quitAndSwitch",
    }),
  ).not.toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.apply" }),
  );
  expect(onSwitch).toHaveBeenCalledWith(false, {
    choice: "keep",
    revision: "catalog-v1",
  });
});

it("requires a fresh matching preview after changing catalog choice before final apply", async () => {
  const onSwitch = vi.fn();
  const onPreviewCatalog = vi.fn();
  const props = {
    targetName: "Target",
    onSwitch,
    onPreviewCatalog,
    onCancel: vi.fn(),
    quitRequired: false,
  };
  const { rerender } = render(
    <SwitchTierConfirmDialog {...props} catalog={catalog} />,
  );
  await userEvent.click(
    screen.getByRole("radio", { name: "provider.preview.catalog.manage" }),
  );
  expect(
    screen.getByRole("button", { name: "applications.apply" }),
  ).toBeDisabled();
  await userEvent.click(
    screen.getByRole("button", {
      name: "provider.preview.catalog.confirmChoice",
    }),
  );
  expect(onPreviewCatalog).toHaveBeenCalledWith({
    choice: "manage",
    revision: "catalog-v1",
  });
  expect(onSwitch).not.toHaveBeenCalled();
  rerender(
    <SwitchTierConfirmDialog
      {...props}
      catalog={{
        ...catalog,
        choice: "manage",
        afterPointer: "/managed/models.json",
      }}
    />,
  );
  expect(screen.getByText("/managed/models.json")).toBeVisible();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.apply" }),
  );
  expect(onSwitch).toHaveBeenCalledWith(false, {
    choice: "manage",
    revision: "catalog-v1",
  });
});
