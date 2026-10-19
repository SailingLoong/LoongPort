import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { providersApi } from "@/lib/api/providers";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { proxyApi, type InitialProxyRoute } from "@/lib/api/proxy";
import type { Provider } from "@/types";

type App = "claude" | "codex";
type Choice = Pick<Provider, "id" | "name"> & { snapshot: string };
type Attempt = InitialProxyRoute & { app: App; name: string };
const emptyApplication = (routing: ApplicationRouting) => {
  const mode = routing.modeState;
  return (
    mode?.status === "ready" &&
    mode.mode === "direct" &&
    mode.attached === false &&
    mode.currentProviderId === null &&
    mode.directProviderId === null &&
    mode.canWrite
  );
};
const accountChoice = (row: Provider): boolean => {
  const binding = row.meta?.authBinding;
  return (
    binding?.source === "managed_account" &&
    !!binding.accountId?.trim() &&
    ["minimax_code", "zcode_start_plan", "workbuddy", "trae"].includes(
      binding.authProvider ?? "",
    )
  );
};

// Keep only a fingerprint of the already-saved route in UI state. The full
// request-affecting projection is compared again after preparation; a different
// row cannot silently replace the account/model the user just chose.
async function routeSnapshot(row: Provider): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(
      JSON.stringify([row.id, row.name, row.settingsConfig, row.meta]),
    ),
  );
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

/** Select a saved account route before the first original ENTER transaction. */
export function ManagedAccountBootstrapSection({
  appId,
  onInteractionChange,
}: {
  appId?: App;
  onInteractionChange?: (dirty: boolean, submitting: boolean) => void;
} = {}) {
  const { t } = useTranslation();
  const copy = (key: string, defaultValue: string) =>
    t(`managedAccount.bootstrap.${key}`, { defaultValue });
  const queryClient = useQueryClient();
  const [app, setApp] = useState<App | "">("");
  const [choices, setChoices] = useState<Choice[]>([]);
  const [selected, setSelected] = useState("");
  const [eligible, setEligible] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState<Attempt | null>(null);
  const [canRestart, setCanRestart] = useState(false);
  const [message, setMessage] = useState("");
  useEffect(() => {
    onInteractionChange?.(!!selected || !!pending, busy);
  }, [selected, pending, busy, onInteractionChange]);
  const alive = useRef(false);
  const generation = useRef(0);
  const inFlight = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      ++generation.current;
    };
  }, []);

  const load = async (target: App | "") => {
    const epoch = ++generation.current;
    setApp(target);
    setSelected("");
    setChoices([]);
    setEligible(false);
    setMessage("");
    setLoading(!!target);
    if (!target) return;
    try {
      const [rows, routing] = await Promise.all([
        providersApi.getAll(target),
        applicationRoutingApi.get(target),
      ]);
      const choices = await Promise.all(
        Object.values(rows)
          .filter(accountChoice)
          .map(async (row) => ({
            id: row.id,
            name: row.name,
            snapshot: await routeSnapshot(row),
          })),
      );
      if (!alive.current || epoch !== generation.current) return;
      setChoices(choices);
      setEligible(emptyApplication(routing));
      if (!emptyApplication(routing))
        setMessage(
          copy(
            "existing",
            "此应用已有配置或状态待核对，请在原应用路由页选择档位或完成恢复。",
          ),
        );
    } catch {
      if (alive.current && epoch === generation.current)
        setMessage(copy("loadFailed", "无法核对应用状态，请刷新后重试。"));
    } finally {
      if (alive.current && epoch === generation.current) setLoading(false);
    }
  };

  useEffect(() => {
    if (appId) void load(appId);
    // The containing view is keyed by app after its existing leave-draft guard.
    // Loading is the original read-only owner path; this never enables a route.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appId]);

  const recheck = async (attempt: Attempt) => {
    const [result, routing] = await Promise.all([
      applicationRoutingApi.queryRequest(attempt.app, attempt.request),
      applicationRoutingApi.get(attempt.app),
    ]);
    if (!alive.current) return;
    const mode = routing.modeState;
    if (
      result.status === "completed" &&
      mode?.status === "ready" &&
      mode.mode === "proxy" &&
      mode.attached === true &&
      mode.currentProviderId === attempt.providerId &&
      routing.routingActive
    ) {
      setPending(null);
      setEligible(false);
      setCanRestart(false);
      setMessage(
        `${attempt.app}: ${attempt.name} · ${copy("enabled", "已启用本地路由")}`,
      );
      void queryClient.invalidateQueries({ queryKey: ["applicationRouting"] });
      void queryClient.invalidateQueries({ queryKey: ["proxy"] });
    } else {
      // A different or missing receipt never authorizes replay. The user may
      // explicitly prepare a fresh confirmation only while the original owner
      // reports an empty writable app; the backend rechecks under its app lock.
      setCanRestart(
        emptyApplication(routing) &&
          ["notRecorded", "unknown", "discarded", "abandoned"].includes(
            result.status,
          ),
      );
      setMessage(
        copy(
          "unconfirmed",
          "尚未确认启用。请核对原应用状态；不会自动重复执行。",
        ),
      );
    }
  };
  const inspect = async () => {
    if (!pending || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      await recheck(pending);
    } catch {
      if (alive.current)
        setMessage(copy("unknown", "结果暂时无法查询，请稍后再核对。"));
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const enable = async () => {
    const choice = choices.find((row) => row.id === selected);
    if (!app || !choice || !eligible || pending || inFlight.current) return;
    const epoch = generation.current;
    const target = app;
    inFlight.current = true;
    setBusy(true);
    setMessage("");
    let attempt: Attempt | undefined;
    try {
      const request = await proxyApi.prepareTakeoverRequest(
        target,
        choice.id,
        crypto.randomUUID(),
      );
      if (!alive.current || epoch !== generation.current) return;
      const actual = (await providersApi.getAll(target))[choice.id];
      if (
        !actual ||
        !accountChoice(actual) ||
        (await routeSnapshot(actual)) !== choice.snapshot
      ) {
        throw new Error("managedAccount.routeChanged");
      }
      if (!alive.current || epoch !== generation.current) return;
      attempt = {
        app: target,
        name: choice.name,
        providerId: choice.id,
        request,
      };
      setPending(attempt);
      setCanRestart(false);
      await proxyApi.setProxyTakeoverForApp(target, true, {
        providerId: choice.id,
        request,
      });
      await recheck(attempt);
    } catch {
      if (!alive.current) return;
      if (attempt) {
        setMessage(copy("uncertain", "启用未确认，请查询结果后再决定下一步。"));
        try {
          await recheck(attempt);
        } catch {
          /* Keep the original pending request. */
        }
      } else
        setMessage(
          copy("prepareFailed", "准备失败，应用可能已变化。请刷新并重新选择。"),
        );
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(false);
    }
  };

  return (
    <section
      aria-label={copy("title", "首次启用账号路由")}
      className="space-y-3 rounded-xl border border-border/60 bg-card/60 p-6"
    >
      <h4 className="font-medium">{copy("title", "首次启用账号路由")}</h4>
      <p className="text-sm text-muted-foreground">
        {copy(
          "description",
          "先将账号加入应用路由链，再明确选择应用和已保存的账号档位。确认后会启动本地服务，并把该应用的原生配置写为本地路由；不会将账号凭据写入原生配置。",
        )}
      </p>
      <label className="block">
        {copy("app", "首次启用的应用")}
        <select
          aria-label={copy("app", "首次启用的应用")}
          value={app}
          disabled={busy || !!pending || !!appId}
          onChange={(event) => void load(event.target.value as App | "")}
        >
          <option value="">{copy("selectApp", "请选择应用")}</option>
          <option value="claude">Claude Code</option>
          <option value="codex">Codex</option>
        </select>
      </label>
      <label className="block">
        {copy("route", "已保存的账号档位")}
        <select
          aria-label={copy("route", "已保存的账号档位")}
          value={selected}
          disabled={busy || loading || !!pending || !eligible}
          onChange={(event) => {
            ++generation.current;
            setSelected(event.target.value);
          }}
        >
          <option value="">{copy("selectRoute", "请选择账号档位")}</option>
          {choices.map((choice) => (
            <option key={choice.id} value={choice.id}>
              {choice.name}
            </option>
          ))}
        </select>
      </label>
      <button
        type="button"
        disabled={!app || busy || !!pending}
        onClick={() => void load(app)}
      >
        {copy("refresh", "刷新应用状态")}
      </button>
      <button
        type="button"
        disabled={!eligible || !selected || busy || loading || !!pending}
        onClick={() => void enable()}
      >
        {copy("enable", "启用本地路由并使用此账号")}
      </button>
      {pending && (
        <button type="button" disabled={busy} onClick={() => void inspect()}>
          {copy("query", "查询启用结果")}
        </button>
      )}
      {pending && canRestart && (
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            setPending(null);
            setCanRestart(false);
            void load(pending.app);
          }}
        >
          {copy("restart", "重新核对并选择")}
        </button>
      )}
      {message && <p role="status">{message}</p>}
    </section>
  );
}
