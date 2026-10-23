import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CodeAgentsPanel } from "@/components/agents/CodeAgentsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const call = vi.mocked(invoke);
beforeEach(() => {
  i18n.addResourceBundle(
    "zh",
    "translation",
    { codeAgents: zh.codeAgents },
    true,
    true,
  );
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
    if (command === "create_code_agents_task")
      throw new Error("private network context agents.access_denied");
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
  expect(await screen.findByRole("alert")).toHaveTextContent("读取已有任务");
  expect(screen.getByRole("alert")).not.toHaveTextContent("private network");
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

it.each([
  ["agents.code_route_unsupported", "API Key"],
  ["agents.access_denied", "重新授权"],
])(
  "shows actionable rejection for %s without claiming a task exists",
  async (code, advice) => {
    call.mockImplementation(async (command) => {
      if (command === "code_agents_access")
        return [{ id: "grant", allowedModels: [] }];
      if (command === "code_agents_models") return ["test-model"];
      if (command === "create_code_agents_task") throw new Error(code);
      return [];
    });
    const user = userEvent.setup();
    render(<CodeAgentsPanel />);
    await user.type(screen.getByLabelText("代码仓库"), "/tmp/example");
    await user.type(screen.getByLabelText("代码任务"), "Fix");
    await user.type(screen.getByLabelText("代码模型"), "test-model");
    await user.click(screen.getByRole("button", { name: "运行代码任务" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(advice);
    expect(alert).not.toHaveTextContent("读取已有任务");
    expect(alert).not.toHaveTextContent(code);
  },
);
it("maps known task reasons without showing backend codes or arbitrary error details", async () => {
  call.mockImplementation(async (command) => {
    if (command === "code_agents_access") return [];
    if (command === "code_agents_models") return [];
    return [
      {
        taskId: "unsupported",
        status: "failed",
        canCancel: false,
        processStopped: true,
        error: "agents.code_route_unsupported",
      },
      {
        taskId: "unrecognized",
        status: "failed",
        canCancel: false,
        processStopped: true,
        error: "private upstream detail agents.access_denied",
      },
    ];
  });
  render(<CodeAgentsPanel />);
  expect(await screen.findByText(/API Key/)).toBeInTheDocument();
  expect(
    screen.getByText("任务未完成，请检查工作区与执行状态。"),
  ).toBeInTheDocument();
  expect(screen.queryByText(/private upstream/)).not.toBeInTheDocument();
  expect(screen.queryByText(/agents\./)).not.toBeInTheDocument();
});
