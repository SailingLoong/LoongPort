import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

type AgentRun = {
  taskId: string;
  agentId: string;
  result: {
    status: string;
    output?: string | null;
    reason?: string | null;
    outputUnavailable?: boolean;
  };
};
interface AgentsPanelProps {
  onOpenChange?: (open: boolean) => void;
}
const statuses: Record<string, string> = {
  accepted: "执行中",
  completed: "完成",
  failed: "失败",
  unknown: "结果未确认",
  not_started: "未启动",
};
export function AgentsPanel({}: AgentsPanelProps) {
  const [input, setInput] = useState("");
  const [model, setModel] = useState("");
  const [runs, setRuns] = useState<AgentRun[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const mounted = useRef(false);
  const operation = useRef(false);
  const revision = useRef(0);
  const request = useRef<{ input: string; model: string; id: string } | null>(
    null,
  );
  async function refresh() {
    if (operation.current) return;
    const current = ++revision.current;
    try {
      const data = await invoke<AgentRun[]>("list_agents_tasks");
      if (mounted.current && revision.current === current) setRuns(data);
    } catch {
      if (mounted.current && revision.current === current)
        setError("无法读取任务，请确认应用已解锁且当前平台受支持。");
    }
  }
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    return () => {
      mounted.current = false;
      revision.current++;
      clearInterval(timer);
    };
  }, []);
  async function mutate(kind: "create" | "cancel", taskId?: string) {
    if (operation.current) return;
    operation.current = true;
    revision.current++;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      let data: AgentRun[];
      if (kind === "create") {
        // A failed/uncertain response keeps the same id for an unchanged draft.
        // Retrying must query the durable claim, not start the same work twice.
        if (
          !request.current ||
          request.current.input !== input ||
          request.current.model !== model
        ) {
          request.current = { input, model, id: crypto.randomUUID() };
        }
        data = await invoke<AgentRun[]>("create_agents_task", {
          requestId: request.current.id,
          input,
          model,
        });
      } else {
        data = await invoke<AgentRun[]>("cancel_agents_task", { taskId });
      }
      if (mounted.current) {
        setRuns(data);
        setNotice(
          kind === "cancel"
            ? "已请求取消；不代表远端已停止，后续状态以结果为准。"
            : "任务状态已更新；未启动项不会自动补跑。",
        );
      }
    } catch {
      if (mounted.current)
        setError(
          kind === "create"
            ? "创建失败或结果未确认。再次提交同一内容不会自动重放已受理任务。"
            : "取消结果未确认，请刷新状态。",
        );
    } finally {
      operation.current = false;
      if (mounted.current) {
        setBusy(false);
        void refresh();
      }
    }
  }
  const taskIds = [...new Set(runs.map((run) => run.taskId))];
  return (
    <div className="page-content flex flex-col flex-1 min-h-0 gap-4 p-6">
      <h2 className="text-xl font-semibold">多 Agents</h2>
      <p className="text-sm text-muted-foreground">
        仅支持 Codex 当前已应用账号链。独立分析与复核共用 LoongPort
        入口，不需要安装 Paperclip。当前先验证两个独立
        run，尚非完整协作流程；仅处理有限文本，不运行工具。
      </p>
      <label className="space-y-1">
        任务
        <Textarea
          aria-label="任务"
          value={input}
          maxLength={8000}
          onChange={(e) => setInput(e.target.value)}
        />
      </label>
      <label className="space-y-1">
        模型
        <Input
          aria-label="模型"
          value={model}
          maxLength={128}
          onChange={(e) => setModel(e.target.value)}
        />
      </label>
      <div className="flex gap-2">
        <Button
          disabled={busy || !input.trim() || !model.trim()}
          onClick={() => void mutate("create")}
        >
          创建任务
        </Button>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void refresh()}
        >
          刷新状态
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      {notice && <p role="status">{notice}</p>}
      {taskIds.map((id) => (
        <section key={id} className="glass-card rounded-xl p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="font-medium">任务 {id.slice(0, 8)}</h3>
            <Button
              variant="outline"
              disabled={
                busy ||
                !runs.some(
                  (r) =>
                    r.taskId === id &&
                    ["accepted", "not_started"].includes(r.result.status),
                )
              }
              onClick={() => void mutate("cancel", id)}
            >
              取消任务
            </Button>
          </div>
          {runs
            .filter((r) => r.taskId === id)
            .map((run) => (
              <div key={run.agentId} className="rounded-md border p-3">
                <p>
                  {run.agentId === "analysis" ? "分析 Agent" : "复核 Agent"} ·{" "}
                  {statuses[run.result.status] ?? "未知状态"}
                </p>
                {run.result.output && (
                  <p className="whitespace-pre-wrap break-words">
                    {run.result.output}
                  </p>
                )}
                {run.result.outputUnavailable && (
                  <p>应用重启后正文不可恢复，保留执行状态；不会自动重跑。</p>
                )}
                {run.result.reason && (
                  <p className="text-sm text-muted-foreground">
                    {run.result.reason}
                  </p>
                )}
              </div>
            ))}
        </section>
      ))}
    </div>
  );
}
