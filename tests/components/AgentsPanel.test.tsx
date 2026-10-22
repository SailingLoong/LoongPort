import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AgentRun } from "@/lib/api/agents";
import { AgentsPanel } from "@/components/agents/AgentsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const call = vi.mocked(invoke);
function groups(runs: AgentRun[], canCancel = true) {
  return [...new Set(runs.map((run) => run.taskId))].map((taskId) => ({
    taskId,
    status: "accepted",
    canCancel,
    runs: runs.filter((run) => run.taskId === taskId),
  }));
}
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
  call.mockResolvedValue(groups(rows));
  const user = userEvent.setup();
  render(<AgentsPanel />);
  await user.click(await screen.findByRole("button", { name: "取消任务" }));
  await waitFor(() =>
    expect(call).toHaveBeenCalledWith("cancel_agents_task", {
      taskId: "task-1",
      grouped: true,
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

it("surfaces only completed review as the final result", async () => {
  call.mockResolvedValue(
    groups([
      {
        taskId: "chain-1",
        agentId: "analysis",
        result: { status: "completed", output: "Initial analysis" },
      },
      {
        taskId: "chain-1",
        agentId: "review",
        dependsOnAnalysis: true,
        result: { status: "completed", output: "Reviewed answer" },
      },
    ]),
  );
  render(<AgentsPanel />);
  expect(
    await screen.findByRole("heading", { name: "最终结果" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Reviewed answer")).toBeInTheDocument();
  expect(screen.getByText(/多个分析并行执行/)).toBeInTheDocument();
});
it("does not turn partial analysis success into a final answer", async () => {
  call.mockResolvedValue(
    groups([
      {
        taskId: "chain-2",
        agentId: "analysis",
        result: { status: "completed", output: "Draft only" },
      },
      {
        taskId: "chain-2",
        agentId: "review",
        dependsOnAnalysis: true,
        result: { status: "failed", reason: "analysis_output_too_large" },
      },
    ]),
  );
  render(<AgentsPanel />);
  expect(await screen.findByText("Draft only")).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "最终结果" }),
  ).not.toBeInTheDocument();
  expect(screen.getByText(/汇总未完成，当前没有最终结果/)).toBeInTheDocument();
});
it("keeps restart output loss explicit without automatically resubmitting", async () => {
  call.mockResolvedValue(
    groups([
      {
        taskId: "chain-3",
        agentId: "review",
        dependsOnAnalysis: true,
        result: { status: "completed", outputUnavailable: true },
      },
    ]),
  );
  render(<AgentsPanel />);
  expect(await screen.findByText(/正文不可恢复/)).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "最终结果" }),
  ).not.toBeInTheDocument();
  expect(
    call.mock.calls.filter(([name]) => name === "create_agents_task"),
  ).toHaveLength(0);
});
it("allows cancellation while review is waiting for analysis", async () => {
  call.mockResolvedValue(
    groups([
      {
        taskId: "chain-4",
        agentId: "analysis",
        result: { status: "completed", output: "Draft" },
      },
      {
        taskId: "chain-4",
        agentId: "review",
        dependsOnAnalysis: true,
        result: { status: "waiting", reason: "waiting_for_analysis" },
      },
    ]),
  );
  const user = userEvent.setup();
  render(<AgentsPanel />);
  const cancel = await screen.findByRole("button", { name: "取消任务" });
  expect(cancel).toBeEnabled();
  await user.click(cancel);
  expect(call).toHaveBeenCalledWith("cancel_agents_task", {
    taskId: "chain-4",
    grouped: true,
  });
});

it("does not relabel a legacy independent review as a chained final result", async () => {
  call.mockResolvedValue(
    groups([
      {
        taskId: "legacy",
        agentId: "review",
        result: { status: "completed", output: "Independent review" },
      },
    ]),
  );
  render(<AgentsPanel />);
  expect(await screen.findByText("Independent review")).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "最终结果" }),
  ).not.toBeInTheDocument();
});

it("uses group state owned by the backend and renders every analysis", async () => {
  call.mockResolvedValue([
    {
      taskId: "group",
      status: "failed",
      canCancel: false,
      runs: [
        {
          taskId: "group",
          agentId: "analysis",
          result: { status: "completed", output: "One" },
        },
        {
          taskId: "group",
          agentId: "analysis-2",
          result: { status: "accepted", output: "Two" },
        },
      ],
    },
  ]);
  render(<AgentsPanel />);
  expect(await screen.findByText("Two")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "取消任务" })).toBeDisabled();
});

it("removes an accidental empty analysis without discarding the draft", async () => {
  const user = userEvent.setup();
  render(<AgentsPanel />);
  await user.type(screen.getByLabelText("任务"), "Keep this objective");
  await user.type(screen.getByLabelText("模型"), "test-model");
  await user.click(screen.getByRole("button", { name: "添加分析" }));
  expect(screen.getByRole("button", { name: "创建任务" })).toBeDisabled();
  await user.click(screen.getByRole("button", { name: "移除分析 3" }));
  expect(screen.getByLabelText("任务")).toHaveValue("Keep this objective");
  expect(screen.getByRole("button", { name: "创建任务" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "移除分析 2" }));
  expect(
    screen.queryByRole("button", { name: "移除分析 1" }),
  ).not.toBeInTheDocument();
});
