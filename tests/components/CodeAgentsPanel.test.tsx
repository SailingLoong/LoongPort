import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CodeAgentsPanel } from "@/components/agents/CodeAgentsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const call = vi.mocked(invoke);
beforeEach(() => {
  call.mockReset();
  call.mockImplementation(async (command, args) => {
    if (
      command === "code_agents_access" &&
      (args as Record<string, unknown> | undefined)?.action === "authorize"
    )
      return { id: "grant", allowedModels: [] };
    if (command === "code_agents_models") return ["test-model"];
    return [];
  });
});
it("reads grants without creating one, then submits an explicit authorized code task", async () => {
  const user = userEvent.setup();
  render(<CodeAgentsPanel />);
  await screen.findByRole("button", { name: "授权代码任务" });
  expect(
    call.mock.calls.some(
      ([, args]) =>
        (args as Record<string, unknown> | undefined)?.action === "authorize",
    ),
  ).toBe(false);
  await user.click(screen.getByRole("button", { name: "授权代码任务" }));
  await user.type(screen.getByLabelText("代码仓库"), "/tmp/example-repository");
  await user.type(screen.getByLabelText("代码任务"), "Fix the bug");
  await user.type(screen.getByLabelText("代码模型"), "test-model");
  await user.click(screen.getByRole("button", { name: "运行代码任务" }));
  await waitFor(() =>
    expect(call).toHaveBeenCalledWith("create_code_agents_task", {
      request: expect.objectContaining({
        repository: "/tmp/example-repository",
        input: "Fix the bug",
        credentialId: "grant",
        model: "test-model",
        routeApp: "codex",
      }),
    }),
  );
});
it("retains the request id after uncertain submission and exposes result and cancellation", async () => {
  call.mockImplementation(async (command) => {
    if (command === "code_agents_access")
      return [{ id: "grant", allowedModels: [] }];
    if (command === "code_agents_models") return ["test-model"];
    if (command === "create_code_agents_task") throw new Error("uncertain");
    return [
      {
        taskId: "task",
        status: "accepted",
        canCancel: true,
        processStopped: false,
        workspace: "/tmp/task",
        changeSummary: "result.txt",
      },
    ];
  });
  const user = userEvent.setup();
  render(<CodeAgentsPanel />);
  await user.type(screen.getByLabelText("代码仓库"), "/tmp/example");
  await user.type(screen.getByLabelText("代码任务"), "Fix");
  await user.type(await screen.findByLabelText("代码模型"), "test-model");
  await user.click(screen.getByRole("button", { name: "运行代码任务" }));
  await screen.findByRole("alert");
  await user.click(screen.getByRole("button", { name: "运行代码任务" }));
  const submissions = call.mock.calls.filter(
    ([command]) => command === "create_code_agents_task",
  );
  expect(submissions).toHaveLength(2);
  expect(submissions[0][1]).toEqual(submissions[1][1]);
  await user.click(screen.getByRole("button", { name: "取消代码任务" }));
  expect(call).toHaveBeenCalledWith("list_code_agents_tasks", {
    cancelTaskId: "task",
  });
  expect(screen.getByText("result.txt")).toBeInTheDocument();
});
