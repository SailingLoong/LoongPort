import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AgentsPanel } from "@/components/agents/AgentsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const call = vi.mocked(invoke);
beforeEach(() => {
  call.mockReset();
  call.mockResolvedValue([]);
});
describe("LoongPort Agents", () => {
  it("creates a bounded pair through the native owner without a budget", async () => {
    const user = userEvent.setup();
    render(<AgentsPanel />);
    await user.type(screen.getByLabelText("任务"), "Review this design");
    await user.type(screen.getByLabelText("模型"), "test-model");
    await user.click(screen.getByRole("button", { name: "创建任务" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(
        "create_agents_task",
        expect.objectContaining({
          input: "Review this design",
          model: "test-model",
        }),
      ),
    );
    const payload = call.mock.calls.find(
      ([name]) => name === "create_agents_task",
    )![1];
    expect(payload).not.toHaveProperty("budget");
    expect(screen.getByText(/仅支持 Codex/)).toBeInTheDocument();
  });
  it("shows failure and does not retry a create automatically", async () => {
    call.mockImplementation(async (name) => {
      if (name === "create_agents_task") throw new Error("denied");
      return [];
    });
    const user = userEvent.setup();
    render(<AgentsPanel />);
    await user.type(screen.getByLabelText("任务"), "Check");
    await user.type(screen.getByLabelText("模型"), "test-model");
    await user.click(screen.getByRole("button", { name: "创建任务" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("创建失败");
    expect(
      call.mock.calls.filter(([name]) => name === "create_agents_task"),
    ).toHaveLength(1);
  });
});

it("reuses an unchanged draft id after an uncertain create", async () => {
  call.mockImplementation(async (name) => {
    if (name === "create_agents_task") throw new Error("uncertain");
    return [];
  });
  const user = userEvent.setup();
  render(<AgentsPanel />);
  await user.type(screen.getByLabelText("任务"), "Review");
  await user.type(screen.getByLabelText("模型"), "test-model");
  await user.click(screen.getByRole("button", { name: "创建任务" }));
  await screen.findByRole("alert");
  await user.click(screen.getByRole("button", { name: "创建任务" }));
  await waitFor(() =>
    expect(
      call.mock.calls.filter(([name]) => name === "create_agents_task"),
    ).toHaveLength(2),
  );
  const submissions = call.mock.calls.filter(
    ([name]) => name === "create_agents_task",
  );
  expect(submissions[0][1]).toEqual(submissions[1][1]);
});
it("cancels through the owner without claiming the provider has stopped", async () => {
  const rows = [
    { taskId: "task-1", agentId: "analysis", result: { status: "accepted" } },
  ];
  call.mockResolvedValue(rows);
  const user = userEvent.setup();
  render(<AgentsPanel />);
  await user.click(await screen.findByRole("button", { name: "取消任务" }));
  await waitFor(() =>
    expect(call).toHaveBeenCalledWith("cancel_agents_task", {
      taskId: "task-1",
    }),
  );
  expect(await screen.findByRole("status")).toHaveTextContent(
    "不代表远端已停止",
  );
});
it("blocks rapid duplicate create while the first submission is pending", async () => {
  let finish!: (v: unknown) => void;
  call.mockImplementation(async (name) =>
    name === "create_agents_task"
      ? await new Promise((resolve) => {
          finish = resolve;
        })
      : [],
  );
  const user = userEvent.setup();
  const view = render(<AgentsPanel />);
  await user.type(screen.getByLabelText("任务"), "Review");
  await user.type(screen.getByLabelText("模型"), "test-model");
  await user.dblClick(screen.getByRole("button", { name: "创建任务" }));
  expect(
    call.mock.calls.filter(([name]) => name === "create_agents_task"),
  ).toHaveLength(1);
  view.unmount();
  finish([]);
});
