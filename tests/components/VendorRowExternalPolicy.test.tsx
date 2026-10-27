import { render, screen, within, fireEvent } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { VendorRow } from "@/components/relay/VendorRow";
import type { VendorAccountRow, VendorPlanInfo } from "@/lib/api/vendor";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/components/relay/RowBalance", () => ({ RowBalance: () => null }));
function mount(current: boolean) {
  const free = {
    planId: "opencode-free",
    planName: "Saved Free",
    providerId: "synthetic-free",
    isCurrent: current,
    userEdited: true,
    canEditConfig: true,
    canSwitch: false,
    switchBlockedReason: "opencodeFreeExternalUnavailable",
  } as VendorPlanInfo;
  const paid = {
    planId: "opencode",
    planName: "Paid API",
    providerId: "synthetic-paid",
    isCurrent: !current,
    userEdited: false,
    canEditConfig: true,
    canSwitch: true,
  } as VendorPlanInfo;
  const account: VendorAccountRow = {
    id: 1,
    vendorId: "opencode",
    vendorName: "OpenCode",
    accountLabel: "Synthetic only",
    status: "ready",
    canQueryBalance: true,
    canRefresh: true,
    canDelete: true,
    plans: [free, paid],
  };
  const onUse = vi.fn();
  const onEdit = vi.fn();
  render(
    <VendorRow
      account={account}
      busy={new Set()}
      onLogin={vi.fn()}
      onProvision={vi.fn()}
      onUse={onUse}
      onDelete={vi.fn()}
      onEdit={onEdit}
      onReset={vi.fn()}
    />,
  );
  return { onUse, onEdit, free };
}
describe("OpenCode external-free saved configuration", () => {
  for (const current of [false, true])
    it(`shows explicit pause without erasing config; current=${current}`, () => {
      const { onUse, onEdit, free } = mount(current);
      const row = screen
        .getByText("Saved Free")
        .closest(".group\\/tier") as HTMLElement;
      expect(
        within(row).getByText(
          "loongport.vendor.opencodeFreeExternalUnavailable",
        ),
      ).toBeInTheDocument();
      expect(
        within(row).queryByRole("button", { name: "provider.enable" }),
      ).not.toBeInTheDocument();
      expect(
        within(row).queryByRole("button", { name: "provider.inUse" }),
      ).not.toBeInTheDocument();
      fireEvent.click(within(row).getByTitle("loongport.tier.edit"));
      expect(onEdit).toHaveBeenCalledWith(free);
      expect(onUse).not.toHaveBeenCalled();
      if (current) {
        const paid = screen
          .getByText("Paid API")
          .closest(".group\\/tier") as HTMLElement;
        fireEvent.click(
          within(paid).getByRole("button", { name: "provider.enable" }),
        );
        expect(onUse).toHaveBeenCalledTimes(1);
      }
    });
});
