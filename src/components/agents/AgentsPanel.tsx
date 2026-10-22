import { CodeAgentsPanel } from "./CodeAgentsPanel";
import { useEffect, useRef, useState } from "react";
import {
  agentsApi,
  MAX_PARALLEL_ANALYSES,
  type AgentTaskGroup,
} from "@/lib/api/agents";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

interface AgentsPanelProps {
  onOpenChange?: (open: boolean) => void;
}
const statuses: Record<string, string> = {
  accepted: "执行中",
  waiting: "等待分析结果",
  completed: "完成",
  failed: "失败",
  unknown: "结果未确认",
  not_started: "未启动",
};
const reasons: Record<string, string> = {
  waiting_for_analysis: "等待分析完成，再提交汇总。",
  analysis_not_completed: "分析未成功完成，没有启动汇总。",
  analysis_output_too_large: "分析与原任务超过单次文本上限，没有启动汇总。",
  analysis_output_unavailable: "分析正文不可用，没有启动汇总。",
  cancelled_before_review: "已取消后续汇总，未提交新的汇总请求。",
  review_binding_failed: "账号链已变化或暂不可用，汇总未启动。",
  review_start_failed: "汇总启动失败，不会自动重试。",
  review_not_started_no_replay: "汇总未启动，不会因刷新或重启自动补跑。",
};
export function AgentsPanel({}: AgentsPanelProps) {
  const [input, setInput] = useState("");
  const [model, setModel] = useState("");
  const [groups, setGroups] = useState<AgentTaskGroup[]>([]);
  const [analyses, setAnalyses] = useState(["分析解决方案", "检查风险与遗漏"]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const mounted = useRef(false);
  const operation = useRef(false);
  const revision = useRef(0);
  const request = useRef<{
    input: string;
    model: string;
    analyses: string;
    id: string;
  } | null>(null);
  async function refresh() {
    if (operation.current) return;
    const current = ++revision.current;
    try {
      const data = await agentsApi.list();
      if (mounted.current && revision.current === current) setGroups(data);
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
      let data: AgentTaskGroup[];
      if (kind === "create") {
        // A failed/uncertain response keeps the same id for an unchanged draft.
        // Retrying must query the durable claim, not start the same work twice.
        if (
          !request.current ||
          request.current.input !== input ||
          request.current.model !== model ||
          request.current.analyses !== JSON.stringify(analyses)
        ) {
          request.current = {
            input,
            model,
            analyses: JSON.stringify(analyses),
            id: crypto.randomUUID(),
          };
        }
        data = await agentsApi.create(
          request.current.id,
          input,
          model,
          analyses.map((instruction) => ({ input: instruction, model })),
        );
      } else {
        data = await agentsApi.cancel(taskId!);
      }
      if (mounted.current) {
        setGroups(data);
        setNotice(
          kind === "cancel"
            ? "已请求取消；不代表远端已停止，后续状态以结果为准。"
            : "任务状态已更新；全部分析成功后把结果交给汇总，失败或取消不会自动重跑。",
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
  return (
    <div className="page-content flex flex-col flex-1 min-h-0 gap-4 p-6">
      <h2 className="text-xl font-semibold">多 Agents</h2>
      <CodeAgentsPanel />
      <p className="text-sm text-muted-foreground">
        仅支持 Codex
        当前已应用账号链。多个分析并行执行，全部完成后汇总为最终答复。仅处理文本，不运行工具。
        应用重启后不会自动续跑或重放任务。
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
      {analyses.map((instruction, index) => (
        <label key={index} className="space-y-1">
          分析 {index + 1}
          <Input
            aria-label={`分析 ${index + 1}`}
            value={instruction}
            onChange={(event) =>
              setAnalyses(
                analyses.map((value, i) =>
                  i === index ? event.target.value : value,
                ),
              )
            }
          />
          {analyses.length > 1 && (
            <Button
              variant="ghost"
              disabled={busy}
              aria-label={`移除分析 ${index + 1}`}
              onClick={() =>
                setAnalyses(analyses.filter((_, i) => i !== index))
              }
            >
              移除
            </Button>
          )}
        </label>
      ))}
      <Button
        variant="outline"
        disabled={busy || analyses.length >= MAX_PARALLEL_ANALYSES}
        onClick={() => setAnalyses([...analyses, ""])}
      >
        添加分析
      </Button>
      <div className="flex gap-2">
        <Button
          disabled={
            busy ||
            !input.trim() ||
            !model.trim() ||
            analyses.some((value) => !value.trim())
          }
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
      {groups.map((group) => (
        <section
          key={group.taskId}
          className="glass-card rounded-xl p-4 space-y-3"
        >
          <div className="flex items-center justify-between">
            <h3 className="font-medium">
              任务 {group.taskId.slice(0, 8)} ·{" "}
              {statuses[group.status] ?? "未知状态"}
            </h3>
            <Button
              variant="outline"
              disabled={busy || !group.canCancel}
              onClick={() => void mutate("cancel", group.taskId)}
            >
              取消任务
            </Button>
          </div>
          {group.runs.map((run, index) => (
            <div key={run.agentId} className="rounded-md border p-3">
              <p>
                {run.agentId === "review" ? "汇总" : `分析 ${index + 1}`} ·{" "}
                {statuses[run.result.status] ?? "未知状态"}
              </p>
              {run.agentId === "review" &&
                run.dependsOnAnalysis &&
                run.result.status === "completed" &&
                run.result.output && <h4 className="font-medium">最终结果</h4>}
              {run.agentId === "review" &&
                run.result.status !== "completed" && (
                  <p className="text-sm text-muted-foreground">
                    汇总未完成，当前没有最终结果。
                  </p>
                )}
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
                  {reasons[run.result.reason] ?? "任务未完成，请查看状态。"}
                </p>
              )}
            </div>
          ))}
        </section>
      ))}
    </div>
  );
}
