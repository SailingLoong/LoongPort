import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
const setDefault = vi.hoisted(() => vi.fn());
vi.mock("@/components/providers/forms/hooks/useCopilotAuth", () => ({
  useCopilotAuth: () => ({
    accounts: [
      {
        id: "copilot-a",
        login: "Synthetic GitHub",
        github_domain: "github.com",
      },
    ],
    defaultAccountId: null,
    isStatusSuccess: true,
    hasAnyAccount: true,
    pollingState: "idle",
    setDefaultAccount: setDefault,
  }),
}));
it("keeps original GitHub account actions and distinguishes missing entitlement data", async () => {
  render(<CopilotAuthSection unified />);
  const auth = screen.getByRole("region", { name: "授权状态" });
  await userEvent.click(within(auth).getByRole("button", { name: "设为默认" }));
  expect(setDefault).toHaveBeenCalledWith("copilot-a");
  expect(screen.getByRole("region", { name: "权益与签到" })).toHaveTextContent(
    "未提供",
  );
  expect(screen.getByRole("region", { name: "模型与推理" })).toHaveTextContent(
    "尚未验收",
  );
});
