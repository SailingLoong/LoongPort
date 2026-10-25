import { extractErrorMessage } from "@/utils/errorUtils";
import { useTranslation } from "react-i18next";
import { useEffect, useRef, useState } from "react";
import {
  codeAgentsApi,
  type CodeCredential,
  type CodeTask,
} from "@/lib/api/agents";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

function codeTaskErrorKey(error: unknown): string | undefined {
  switch (extractErrorMessage(error)) {
    case "agents.code_route_unsupported":
      return "codeAgents.routeUnsupported";
    case "agents.access_denied":
      return "codeAgents.accessDenied";
    case "agents.runtime_requires_interaction":
      return "codeAgents.interaction";
    default:
      return undefined;
  }
}

export function CodeAgentsPanel() {
  const { t } = useTranslation();
  const statuses: Record<string, string> = {
    accepted: t("codeAgents.running"),
    completed: t("codeAgents.completed"),
    cancelled: t("codeAgents.cancelled"),
    failed: t("codeAgents.failed"),
    unknown: t("codeAgents.unknown"),
  };
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
        setError(t("codeAgents.readFailed"));
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
        if (active.current) {
          // Optional model suggestions must not hide an operation's outcome.
          setError((current) => current || t("codeAgents.modelsFailed"));
        }
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
    } catch (cause) {
      if (active.current) {
        const knownReason = codeTaskErrorKey(cause);
        setError(
          t(
            knownReason ??
              (action === "create"
                ? "codeAgents.createUncertain"
                : "codeAgents.operationFailed"),
          ),
        );
      }
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
      <h3 className="font-semibold">{t("codeAgents.title")}</h3>
      <p className="text-sm text-muted-foreground">
        {t("codeAgents.description")}
      </p>
      <div className="flex gap-2">
        <Button disabled={busy} onClick={() => void mutate("authorize")}>
          {t("codeAgents.authorize")}
        </Button>
        <Button
          variant="outline"
          disabled={busy || !credentialId}
          onClick={() => void mutate("revoke")}
        >
          {t("codeAgents.revoke")}
        </Button>
      </div>
      <label className="block">
        {t("codeAgents.grant")}
        <select
          aria-label={t("codeAgents.grant")}
          value={credentialId}
          onChange={(event) => setCredentialId(event.target.value)}
          className="block w-full rounded border bg-background p-2"
        >
          <option value="">{t("codeAgents.chooseGrant")}</option>
          {credentials.map((credential) => (
            <option key={credential.id} value={credential.id}>
              Codex · {credential.id.slice(0, 8)}
            </option>
          ))}
        </select>
      </label>
      <label className="block">
        {t("codeAgents.repository")}
        <Input
          aria-label={t("codeAgents.repository")}
          value={repository}
          onChange={(event) => setRepository(event.target.value)}
        />
      </label>
      <label className="block">
        {t("codeAgents.base")}
        <Input
          aria-label={t("codeAgents.base")}
          value={baseRef}
          onChange={(event) => setBaseRef(event.target.value)}
        />
      </label>
      <label className="block">
        {t("codeAgents.model")}
        <Input
          aria-label={t("codeAgents.model")}
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
          {t("codeAgents.modelHint")}
        </span>
      </label>
      <label className="block">
        {t("codeAgents.title")}
        <Textarea
          aria-label={t("codeAgents.title")}
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
          {t("codeAgents.run")}
        </Button>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void refresh()}
        >
          {t("codeAgents.refresh")}
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
            {t("codeAgents.title")} {task.taskId.slice(0, 8)} ·{" "}
            {statuses[task.status] ?? t("codeAgents.unknownStatus")}
          </p>
          {task.workspace && (
            <p className="break-all">
              {t("codeAgents.workspace")}
              {task.workspace}
            </p>
          )}
          {task.baseCommit && (
            <p>
              {t("codeAgents.baseCommit")}
              {task.baseCommit}
            </p>
          )}
          {task.headCommit && (
            <p>
              {t("codeAgents.headCommit")}
              {task.headCommit}
            </p>
          )}
          {task.changeSummary && (
            <pre className="whitespace-pre-wrap break-words">
              {task.changeSummary}
            </pre>
          )}
          {task.output && (
            <p className="whitespace-pre-wrap break-words">{task.output}</p>
          )}
          {task.outputUnavailable && <p>{t("codeAgents.lostOutput")}</p>}
          {task.error && (
            <p>{t(codeTaskErrorKey(task.error) ?? "codeAgents.unfinished")}</p>
          )}
          {!task.canCancel && !task.processStopped && (
            <p>{t("codeAgents.processUnknown")}</p>
          )}
          <Button
            variant="outline"
            disabled={busy || !task.canCancel}
            onClick={() => void mutate("cancel", task.taskId)}
          >
            {t("codeAgents.cancel")}
          </Button>
        </article>
      ))}
    </section>
  );
}
