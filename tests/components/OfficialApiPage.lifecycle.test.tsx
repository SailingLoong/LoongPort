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
  await user.click(
    screen.getByRole("button", { name: /官网账户登录（不会自动获取密钥）/ }),
  );
  rerender(view(false));
  await act(async () => resolve({ rowId: 1 }));
  rerender(view(true));
  expect(connected).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: /官网账户登录（不会自动获取密钥）/ }),
  ).toBeEnabled();
});

it("does not restore stale OpenCode failure guidance after leaving the page", async () => {
  let reject!: (reason: Error) => void;
  vi.mocked(vendorApi.openLogin).mockImplementationOnce(
    () =>
      new Promise((_resolve, rejectLogin) => {
        reject = rejectLogin;
      }),
  );
  const connected = vi.fn();
  const back = vi.fn();
  const view = (active: boolean) => (
    <PreservedView active={active}>
      <OfficialApiPage
        sourceAppId="codex"
        vendorId="opencode"
        onBack={back}
        onConnected={connected}
      />
    </PreservedView>
  );
  const { rerender } = render(view(true));
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: /官网账户登录/ }));
  rerender(view(false));
  await act(async () => reject(new Error("登录未完成")));
  rerender(view(true));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();

  vi.mocked(vendorApi.openLogin).mockRejectedValueOnce(
    new Error("新版 Console 未取得可用密钥"),
  );
  await user.click(screen.getByRole("button", { name: /官网账户登录/ }));
  expect(
    await screen.findByRole("button", {
      name: "使用已有 Personal key 或本机导入",
    }),
  ).toBeEnabled();
  rerender(view(false));
  rerender(view(true));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(connected).not.toHaveBeenCalled();
  expect(back).not.toHaveBeenCalled();
});
