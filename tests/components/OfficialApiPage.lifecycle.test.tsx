import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { OfficialApiPage } from "@/components/relay/OfficialApiPage";
import { PreservedView } from "@/components/ui/PreservedView";
import { vendorApi } from "@/lib/api/vendor";
vi.mock("@/lib/api/vendor", async (original) => ({
  ...(await original<typeof import("@/lib/api/vendor")>()),
  vendorApi: { openLogin: vi.fn() },
}));
it("retires a hidden login result and unlocks the preserved destination on return", async () => {
  let resolve!: (value: any) => void;
  vi.mocked(vendorApi.openLogin).mockImplementationOnce(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const connected = vi.fn();
  const view = (active: boolean) => (
    <PreservedView active={active}>
      <OfficialApiPage
        sourceAppId="codex"
        vendorId="opencode"
        onBack={vi.fn()}
        onConnected={connected}
      />
    </PreservedView>
  );
  const { rerender } = render(view(true));
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: /opencode.*connect/i }));
  rerender(view(false));
  await act(async () => resolve({ rowId: 1 }));
  rerender(view(true));
  expect(connected).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: /opencode.*connect/i }),
  ).toBeEnabled();
});
