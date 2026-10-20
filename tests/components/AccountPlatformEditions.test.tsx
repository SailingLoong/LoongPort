import { render, screen, within } from "@testing-library/react";
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
it("does not mount international TRAE account access for the unconnected China edition", () => {
  render(<AuthCenterPanel platform="trae_cn" editionScoped />);
  expect(screen.getByText(/中国版尚未接入/)).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: /导入/ }),
  ).not.toBeInTheDocument();
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
