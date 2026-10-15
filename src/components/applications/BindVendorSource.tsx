import { useRef, useState, useSyncExternalStore } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import {
  serviceSourcesApi,
  type ServiceSource,
} from "@/lib/api/serviceSources";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

type Operation = {
  phase: "idle" | "pending" | "unknown" | "confirmed";
  source: ServiceSource | null;
};
const operations = new WeakMap<
  object,
  Map<string, ReturnType<typeof makeOperation>>
>();
function makeOperation() {
  let value: Operation = { phase: "idle", source: null };
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set: (next: Operation) => {
      value = next;
      listeners.forEach((listener) => listener());
    },
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
function operationFor(client: object, appId: string) {
  let apps = operations.get(client);
  if (!apps) {
    apps = new Map();
    operations.set(client, apps);
  }
  let operation = apps.get(appId);
  if (!operation) {
    operation = makeOperation();
    apps.set(appId, operation);
  }
  return operation;
}

export function BindVendorSource({
  appId,
  disabled,
}: {
  appId: AppId;
  disabled?: boolean;
}) {
  return appId === "claude" || appId === "codex" ? (
    <BindingForm key={appId} appId={appId} disabled={disabled} />
  ) : null;
}
function BindingForm({
  appId,
  disabled,
}: {
  appId: "claude" | "codex";
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const lock = useRef(false);
  const operation = operationFor(client, appId);
  const outcome = useSyncExternalStore(operation.subscribe, operation.get);
  const [open, setOpen] = useState(outcome.phase !== "idle");
  const [sources, setSources] = useState<ServiceSource[]>([]);
  const [selected, setSelected] = useState("");
  const [loading, setBusy] = useState(false);
  const busy = loading || outcome.phase === "pending";
  const status = outcome.phase;
  const [loadFailed, setLoadFailed] = useState(false);
  const choices = sources.filter(
    (s) =>
      s.account.kind === "vendor" && !s.bindings.some((b) => b.appId === appId),
  );
  async function load() {
    if (
      lock.current ||
      disabled ||
      operation.get().phase === "pending" ||
      operation.get().phase === "unknown"
    )
      return;
    if (operation.get().phase === "confirmed") {
      operation.set({ phase: "idle", source: null });
      setSelected("");
    }
    lock.current = true;
    setBusy(true);
    setOpen(true);
    setLoadFailed(false);
    setSources([]);
    try {
      setSources(await serviceSourcesApi.list());
    } catch {
      setLoadFailed(true);
    } finally {
      lock.current = false;
      setBusy(false);
    }
  }
  async function readback(source: ServiceSource) {
    try {
      const fresh = await serviceSourcesApi.list();
      setSources(fresh);
      const found = fresh.find(
        (s) =>
          s.sourceId === source.sourceId &&
          s.account.kind === "vendor" &&
          s.account.id === source.account.id &&
          s.bindings.some((b) => b.appId === appId),
      );
      if (found) {
        client.setQueryData(["serviceSources"], fresh);
        operation.set({ phase: "confirmed", source });
        for (const key of [
          "applicationOverview",
          "providers",
          "applicationRouting",
        ])
          void client.invalidateQueries({ queryKey: [key, appId] });
      } else {
        operation.set({ phase: "unknown", source });
      }
    } catch {
      operation.set({ phase: "unknown", source });
    }
  }
  async function bind() {
    const source = choices.find((s) => s.sourceId === selected);
    if (!source || disabled || lock.current || operation.get().phase !== "idle")
      return;
    lock.current = true;
    setBusy(true);
    operation.set({ phase: "pending", source });
    try {
      await serviceSourcesApi.bindVendor(source.sourceId, appId);
    } catch {
      // A transport error does not establish whether the command committed.
    }
    await readback(source);
    lock.current = false;
    setBusy(false);
  }
  async function check() {
    const current = operation.get();
    if (!current.source || current.phase !== "unknown" || lock.current) return;
    lock.current = true;
    setBusy(true);
    operation.set({ ...current, phase: "pending" });
    await readback(current.source);
    lock.current = false;
    setBusy(false);
  }
  return (
    <div className="space-y-2">
      <Button
        variant="outline"
        disabled={disabled || busy || status === "unknown"}
        onClick={() => {
          void load();
        }}
      >
        {t("applications.bindSource.open")}
      </Button>
      {open && (
        <div className="space-y-2 rounded-md border p-3">
          <p className="text-sm text-muted-foreground">
            {t("applications.bindSource.hint")}
          </p>
          <Select
            value={selected}
            onValueChange={setSelected}
            disabled={
              disabled || busy || status === "unknown" || status === "confirmed"
            }
          >
            <SelectTrigger aria-label={t("applications.bindSource.source")}>
              <SelectValue placeholder={t("applications.bindSource.source")} />
            </SelectTrigger>
            <SelectContent>
              {choices.map((s) => (
                <SelectItem key={s.sourceId} value={s.sourceId}>
                  {s.name} · {s.serviceName} · {s.accountLabel}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            disabled={
              disabled ||
              busy ||
              !choices.some((s) => s.sourceId === selected) ||
              status === "unknown" ||
              status === "confirmed"
            }
            onClick={() => {
              void bind();
            }}
          >
            {t("applications.bindSource.add")}
          </Button>
          {busy && <p role="status">{t("applications.bindSource.busy")}</p>}
          {!busy &&
            !loadFailed &&
            choices.length === 0 &&
            status === "idle" && <p>{t("applications.bindSource.empty")}</p>}
          {(loadFailed || status === "unknown") && (
            <p role="alert">
              {t(
                `applications.bindSource.${status === "unknown" ? "unknown" : "failed"}`,
              )}
            </p>
          )}
          {status === "confirmed" && (
            <p role="status">{t("applications.bindSource.confirmed")}</p>
          )}
          {status === "unknown" && (
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => {
                void check();
              }}
            >
              {t("applications.bindSource.check")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
