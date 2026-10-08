import { beforeEach, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkBuddyAccounts } from "@/components/workbuddy/WorkBuddyAccounts";
const api = vi.hoisted(() => ({
  list: vi.fn(),
  refresh: vi.fn(),
  refreshAll: vi.fn(),
  claim: vi.fn(),
  beginLogin: vi.fn(),
  finishLogin: vi.fn(),
  openExternal: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: api.openExternal },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const row = {
  id: "cn-account-a",
  label: "Synthetic account",
  canRefresh: true,
  canClaim: true,
  claimState: "available",
  credited: null,
  credits: {
    totalRemaining: null,
    nearestExpiry: null,
    updatedAt: null,
    packages: [
      { id: "r1", name: "Synthetic package", remaining: 0, expireAt: null },
    ],
  },
};
beforeEach(() => {
  vi.clearAllMocks();
  api.list.mockResolvedValue([row]);
  api.refresh.mockResolvedValue(row);
  api.refreshAll.mockResolvedValue([row]);
  api.claim.mockResolvedValue({
    ...row,
    claimState: "unconfirmed",
    canClaim: false,
  });
});
it("mount and refresh never claim, and unknown balance is not zero", async () => {
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(screen.getByTestId("workbuddy-total")).toHaveTextContent("—");
  expect(api.refresh).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "common.refresh" }));
  await waitFor(() => expect(api.refresh).toHaveBeenCalledWith("cn-account-a"));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await waitFor(() => expect(api.refreshAll).toHaveBeenCalledOnce());
  expect(api.claim).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.packages" }),
  );
  expect(await screen.findByText("Synthetic package")).toBeVisible();
});
it("explicit claim is single flight and uncertain readback stays unconfirmed", async () => {
  let resolve!: (v: typeof row) => void;
  api.claim.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  const button = screen.getByRole("button", { name: "workbuddy.claim" });
  await userEvent.dblClick(button);
  expect(api.claim).toHaveBeenCalledTimes(1);
  resolve({ ...row, claimState: "unconfirmed", canClaim: false });
  expect(await screen.findByText("workbuddy.state.unconfirmed")).toBeVisible();
});
it("backend capabilities control the claim action", async () => {
  api.list.mockResolvedValue([
    { ...row, claimState: "needsVerification", canClaim: false },
  ]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(
    screen.getByRole("button", { name: "workbuddy.claim" }),
  ).toBeDisabled();
  expect(screen.getByText("workbuddy.state.needsVerification")).toBeVisible();
});
it("untrusted transport errors do not reach UI and failed refresh retains snapshot", async () => {
  api.refresh.mockRejectedValue(new Error("SYNTHETIC_SECRET_CANARY"));
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("button", { name: "common.refresh" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.failed",
  );
  expect(screen.queryByText(/SYNTHETIC_SECRET_CANARY/)).not.toBeInTheDocument();
  expect(screen.getByText("Synthetic account")).toBeVisible();
});
it("adding an account starts only on click and saves after explicit authorization completion", async () => {
  api.beginLogin.mockResolvedValue({
    flowId: "f1",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(api.beginLogin).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  const link = await screen.findByRole("link", { name: "workbuddy.authorize" });
  expect(link).toHaveAttribute(
    "href",
    "https://www.codebuddy.cn/login?state=synthetic",
  );
  expect(api.openExternal).not.toHaveBeenCalled();
  await userEvent.click(link);
  expect(api.openExternal).toHaveBeenCalledWith(
    "https://www.codebuddy.cn/login?state=synthetic",
  );
  expect(api.finishLogin).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.finishAuthorization" }),
  );
  await waitFor(() => expect(api.finishLogin).toHaveBeenCalledWith("f1"));
  expect(api.claim).not.toHaveBeenCalled();
});
