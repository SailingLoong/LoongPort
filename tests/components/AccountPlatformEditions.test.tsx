import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { QueryClientProvider } from "@tanstack/react-query";
import { createTestQueryClient } from "../utils/testQueryClient";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
const list = vi.hoisted(() =>
  vi.fn(async () => [
    {
      id: "cn",
      label: "CN account",
      region: "cn",
      status: "needsLogin",
      authorized: false,
      canClaim: false,
      canRefresh: false,
      canRetire: false,
    },
    {
      id: "global",
      label: "Global account",
      region: "global",
      status: "needsLogin",
      authorized: false,
      canClaim: false,
      canRefresh: false,
      canRetire: false,
    },
  ]),
);
vi.mock("@/lib/api/minimaxCode", () => ({ minimaxCodeApi: { list } }));
const trae = vi.hoisted(() => ({
  list: vi.fn(async () => [
    { id: "CN-only-account", realm: "cn", status: "stored" },
    { id: "SG-only-account", realm: "sg", status: "stored" },
    { id: "US-only-account", realm: "us", status: "stored" },
  ]),
  begin: vi.fn(),
  confirm: vi.fn(),
  cancel: vi.fn(),
  retire: vi.fn(),
}));
vi.mock("@/lib/api/traeAccounts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/traeAccounts")>()),
  traeAccountsApi: trae,
}));
it("keeps MiniMax domestic and global identities separate", async () => {
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AuthCenterPanel platform="minimax_code_global" editionScoped />
    </QueryClientProvider>,
  );
  expect(
    await screen.findByRole("button", { name: "查看 Global account" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("CN account")).not.toBeInTheDocument();
});
it("mounts only CN Code source, accounts and authorization host for the China edition", async () => {
  render(<AuthCenterPanel platform="trae_cn" editionScoped />);
  expect(
    await screen.findByRole("heading", { name: "CN-only-account" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("SG-only-account")).not.toBeInTheDocument();
  expect(screen.queryByText("US-only-account")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "检查并尝试签到" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "开启自动签到" }),
  ).toBeInTheDocument();
  expect(screen.queryByText(/签到尚未接入/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /添加.*TRAE/ }));
  expect(screen.getByText(/官方稳定版 Trae CN Code/)).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "CN" })).toBeInTheDocument();
  expect(screen.queryByRole("radio", { name: "SG" })).not.toBeInTheDocument();
  expect(screen.queryByRole("radio", { name: "US" })).not.toBeInTheDocument();
  expect(screen.getByText(/https:\/\/api\.trae\.cn/)).toBeInTheDocument();
  expect(
    screen.queryByText(/https:\/\/growsg-normal\.trae\.ai/),
  ).not.toBeInTheDocument();
  expect(trae.begin).not.toHaveBeenCalled();
  expect(trae.confirm).not.toHaveBeenCalled();
});
it("shows MiniMax authorization separately from claim state and inference acceptance", async () => {
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AuthCenterPanel platform="minimax_code_global" editionScoped />
    </QueryClientProvider>,
  );
  expect(
    await screen.findByRole("region", { name: "授权状态" }),
  ).toHaveTextContent("需要重新授权");
  expect(
    screen.getByRole("region", { name: "权益与签到" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "模型与推理" })).toHaveTextContent(
    "尚未验收",
  );
});

it("keeps a claimed badge out of MiniMax authorization status", async () => {
  list.mockResolvedValueOnce([
    {
      id: "cn",
      label: "Claimed account",
      region: "cn",
      status: "claimed",
      authorized: true,
      canClaim: false,
      canRefresh: true,
      canRetire: true,
    },
  ]);
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AuthCenterPanel platform="minimax_code" editionScoped />
    </QueryClientProvider>,
  );
  const auth = await screen.findByRole("region", { name: "授权状态" });
  expect(auth).toHaveTextContent("账号授权已保存");
  expect(
    within(auth).queryByText("minimaxCode.status.claimed"),
  ).not.toBeInTheDocument();
  expect(
    within(screen.getByRole("region", { name: "权益与签到" })).getByText(
      "minimaxCode.status.claimed",
    ),
  ).toBeInTheDocument();
});

it("does not expose CN check-in controls in the global TRAE edition", async () => {
  render(<AuthCenterPanel platform="trae" editionScoped />);
  expect(
    await screen.findByRole("heading", { name: "SG-only-account" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("CN-only-account")).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "检查并尝试签到" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: /开启自动签到|关闭自动签到/ }),
  ).not.toBeInTheDocument();
});
