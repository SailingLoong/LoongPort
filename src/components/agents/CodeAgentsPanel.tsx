import { useEffect, useRef, useState } from "react";
import {
  codeAgentsApi,
  type CodeCredential,
  type CodeTask,
} from "@/lib/api/agents";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

const statuses: Record<string, string> = {
  accepted: "执行中",
  completed: "完成",
  cancelled: "已取消",
  failed: "失败",
  unknown: "结果未确认",
};
export function CodeAgentsPanel() {
  const [credentials, setCredentials] = useState<CodeCredential[]>([]);
  const [credentialId, setCredentialId] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [model, setModel] = useState("");
  const [repository, setRepository] = useState("");
  const [baseRef, setBaseRef] = useState("HEAD");
  const [input, setInput] = useState("");
  const [tasks, setTasks] = useState<CodeTask[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const active = useRef(true);
  const operation = useRef(false);
  const revision = useRef(0);
  const draft = useRef<{ fingerprint: string; id: string } | null>(null);
  async function refresh() {
    if (operation.current) return;
    const current = ++revision.current;
    try {
      const [grants, records] = await Promise.all([
        codeAgentsApi.access(),
        codeAgentsApi.list(),
      ]);
      if (!active.current || current !== revision.current) return;
      setCredentials(grants);
      setCredentialId((value) =>
        grants.some((grant) => grant.id === value)
          ? value
          : (grants[0]?.id ?? ""),
      );
      setTasks(records);
    } catch {
      if (active.current && current === revision.current)
        setError("无法读取代码任务。");
    }
  }
  useEffect(() => {
    active.current = true;
    void refresh();
    void codeAgentsApi
      .models()
      .then((values) => {
        if (active.current) setModels(values);
      })
      .catch(() => {
        if (active.current) setError("Codex 暂不可用，请检查本机安装。");
      });
    const timer = setInterval(() => void refresh(), 3000);
    return () => {
      active.current = false;
      revision.current++;
      clearInterval(timer);
    };
  }, []);
  async function mutate(
    action: "authorize" | "revoke" | "create" | "cancel",
    taskId?: string,
  ) {
    if (operation.current) return;
    operation.current = true;
    revision.current++;
    setBusy(true);
    setError("");
    try {
      if (action === "authorize") {
        const grant = await codeAgentsApi.authorize();
        if (active.current) {
          setCredentials((values) => [
            ...values.filter((value) => value.id !== grant.id),
            grant,
          ]);
          setCredentialId(grant.id);
        }
      } else if (action === "revoke") {
        await codeAgentsApi.revoke(credentialId);
        if (active.current) {
          setCredentials((values) =>
            values.filter((value) => value.id !== credentialId),
          );
          setCredentialId("");
        }
      } else if (action === "cancel") {
        const records = await codeAgentsApi.cancel(taskId!);
        if (active.current) setTasks(records);
      } else {
        const request = {
          repository,
          baseRef,
          input,
          model,
          credentialId,
          routeApp: "codex",
        };
        const fingerprint = JSON.stringify(request);
        if (draft.current?.fingerprint !== fingerprint)
          draft.current = { fingerprint, id: crypto.randomUUID() };
        await codeAgentsApi.create({ ...request, requestId: draft.current.id });
      }
    } catch {
      if (active.current)
        setError(
          action === "create"
            ? "启动结果未确认；再次提交相同内容会读取已有任务。"
            : "操作失败，请刷新状态后重试。",
        );
    } finally {
      operation.current = false;
      if (active.current) {
        setBusy(false);
        if (action !== "authorize") void refresh();
      }
    }
  }
  const selected = credentials.find(
    (credential) => credential.id === credentialId,
  );
  const availableModels = models.filter(
    (value) =>
      !selected?.allowedModels.length || selected.allowedModels.includes(value),
  );
  return (
    <section className="glass-card rounded-xl p-4 space-y-3">
      <h3 className="font-semibold">代码任务</h3>
      <p className="text-sm text-muted-foreground">
        在独立工作区运行 Codex。修改保留供你检查，不会自动合并。仅使用你授权的
        LoongPort 账号链。
      </p>
      <div className="flex gap-2">
        <Button disabled={busy} onClick={() => void mutate("authorize")}>
          授权代码任务
        </Button>
        <Button
          variant="outline"
          disabled={busy || !credentialId}
          onClick={() => void mutate("revoke")}
        >
          撤销代码授权
        </Button>
      </div>
      <label className="block">
        代码授权
        <select
          aria-label="代码授权"
          value={credentialId}
          onChange={(event) => setCredentialId(event.target.value)}
          className="block w-full rounded border bg-background p-2"
        >
          <option value="">请选择授权</option>
          {credentials.map((credential) => (
            <option key={credential.id} value={credential.id}>
              Codex · {credential.id.slice(0, 8)}
            </option>
          ))}
        </select>
      </label>
      <label className="block">
        代码仓库
        <Input
          aria-label="代码仓库"
          value={repository}
          onChange={(event) => setRepository(event.target.value)}
        />
      </label>
      <label className="block">
        基准版本
        <Input
          aria-label="基准版本"
          value={baseRef}
          onChange={(event) => setBaseRef(event.target.value)}
        />
      </label>
      <label className="block">
        代码模型
        <Input
          aria-label="代码模型"
          list="code-agent-models"
          value={model}
          onChange={(event) => setModel(event.target.value)}
          maxLength={128}
        />
        <datalist id="code-agent-models">
          {availableModels.map((value) => (
            <option key={value} value={value} />
          ))}
        </datalist>
        <span className="text-sm text-muted-foreground">
          可选择账号链已有模型，或填写模型 ID。实际可用性由运行结果确认。
        </span>
      </label>
      <label className="block">
        代码任务
        <Textarea
          aria-label="代码任务"
          maxLength={65536}
          value={input}
          onChange={(event) => setInput(event.target.value)}
        />
      </label>
      <div className="flex gap-2">
        <Button
          disabled={
            busy ||
            !credentialId ||
            !repository.trim() ||
            !baseRef.trim() ||
            !input.trim() ||
            !model.trim() ||
            (!!selected?.allowedModels.length &&
              !selected.allowedModels.includes(model))
          }
          onClick={() => void mutate("create")}
        >
          运行代码任务
        </Button>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void refresh()}
        >
          刷新代码状态
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      {tasks.map((task) => (
        <article key={task.taskId} className="rounded border p-3 space-y-2">
          <p>
            代码任务 {task.taskId.slice(0, 8)} ·{" "}
            {statuses[task.status] ?? "未知状态"}
          </p>
          {task.workspace && (
            <p className="break-all">工作区：{task.workspace}</p>
          )}
          {task.baseCommit && <p>基准：{task.baseCommit}</p>}
          {task.headCommit && <p>结果版本：{task.headCommit}</p>}
          {task.changeSummary && (
            <pre className="whitespace-pre-wrap break-words">
              {task.changeSummary}
            </pre>
          )}
          {task.output && (
            <p className="whitespace-pre-wrap break-words">{task.output}</p>
          )}
          {task.outputUnavailable && (
            <p>应用重启后正文不可恢复，工作区和执行状态仍保留。</p>
          )}
          {task.error && (
            <p>
              {task.error === "agents.runtime_requires_interaction"
                ? "Codex 需要额外确认，任务已停止。"
                : "任务未完成，请检查工作区与执行状态。"}
            </p>
          )}
          {!task.canCancel && !task.processStopped && (
            <p>进程停止状态未确认。</p>
          )}
          <Button
            variant="outline"
            disabled={busy || !task.canCancel}
            onClick={() => void mutate("cancel", task.taskId)}
          >
            取消代码任务
          </Button>
        </article>
      ))}
    </section>
  );
}
