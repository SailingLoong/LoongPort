import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { exit } from "@tauri-apps/plugin-process";
import { SecretUnlockForm } from "@/components/SecretUnlock";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  startupUpgradeApi,
  type StartupUpgradeReview,
  type UpgradeApp,
  type UpgradeAppReview,
} from "@/lib/api/startupUpgrade";

const APPS: { id: UpgradeApp; name: string }[] = [
  { id: "claude", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "gemini", name: "Gemini CLI" },
  { id: "grokbuild", name: "Grok Build" },
];

/** Queries and controlled recovery use the original owner; drafts never grant admission. */
function UpgradeAppCard({
  app,
  token,
}: {
  app: (typeof APPS)[number];
  token: string;
}) {
  const { t } = useTranslation();
  const [view, setView] = useState<UpgradeAppReview | null>(null);
  const [fresh, setFresh] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const busyRef = useRef(false);
  const request = useRef(0);

  async function query() {
    const sequence = ++request.current;
    setFresh(false);
    try {
      const next = await startupUpgradeApi.queryApp(token, app.id);
      if (sequence !== request.current) return;
      if (next.appType !== app.id) throw new Error("upgrade.source_changed");
      setView(next);
      setFresh(true);
    } catch {
      if (sequence === request.current) setError(true);
    }
  }

  useEffect(() => {
    void query();
    return () => {
      ++request.current;
    };
  }, [token, app.id]);

  async function act(recover: boolean) {
    if (busyRef.current) return;
    if (recover && (!fresh || !view?.canRecoverOperation || !view.revision))
      return;
    busyRef.current = true;
    setBusy(true);
    setError(false);
    setFresh(false);
    const sequence = ++request.current;
    try {
      if (recover && view) {
        const next = await startupUpgradeApi.recoverApp(
          token,
          app.id,
          view.revision,
        );
        if (sequence !== request.current) return;
        if (next.appType !== app.id) throw new Error("upgrade.source_changed");
        setView(next);
        setFresh(true);
      } else {
        await query();
      }
    } catch {
      if (sequence !== request.current) return;
      setError(true);
      // A rejected/lost response is queried, never automatically replayed.
      await query();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  const fact = (value: boolean | null | undefined) =>
    t(
      value === true
        ? "startupUpgrade.yes"
        : value === false
          ? "startupUpgrade.no"
          : "startupUpgrade.unknown",
    );
  return (
    <section
      aria-label={app.name}
      className="space-y-3 rounded-lg border p-4"
      aria-busy={busy}
    >
      <h2 className="font-semibold">{app.name}</h2>
      <dl className="grid grid-cols-2 gap-x-3 gap-y-2 text-sm">
        <dt>{t("startupUpgrade.modeLabel")}</dt>
        <dd>
          {t(
            view?.savedMode === "direct"
              ? "startupUpgrade.mode.direct"
              : view?.savedMode === "proxy"
                ? "startupUpgrade.mode.proxy"
                : "startupUpgrade.unknown",
          )}
        </dd>
        <dt>{t("startupUpgrade.journalLabel")}</dt>
        <dd>{fact(view?.hasPendingOperation)}</dd>
        <dt>{t("startupUpgrade.pointerLabel")}</dt>
        <dd>{fact(view?.pointerConsistent)}</dd>
        <dt>{t("startupUpgrade.fieldsLabel")}</dt>
        <dd>{fact(view?.storedFieldsMatch)}</dd>
      </dl>
      <p className="text-sm text-muted-foreground">
        {t("startupUpgrade.keepFiles")}
      </p>
      <p className="text-sm text-muted-foreground">
        {t("startupUpgrade.noTakeover")}
      </p>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {t("startupUpgrade.appQueryFailed")}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void act(false)}
        >
          {t("startupUpgrade.recheck")}
        </Button>
        <Button
          disabled={busy || !fresh || view?.canRecoverOperation !== true}
          onClick={() => void act(true)}
        >
          {t("startupUpgrade.recover")}
        </Button>
      </div>
    </section>
  );
}

export function StartupUpgrade() {
  const { t } = useTranslation();
  const [view, setView] = useState<StartupUpgradeReview | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const busyRef = useRef(false);
  const request = useRef(0);

  async function query() {
    const sequence = ++request.current;
    // Never leave an old token actionable while a fresh query is uncertain.
    setView(null);
    try {
      const next = await startupUpgradeApi.query();
      if (sequence === request.current) setView(next);
    } catch {
      if (sequence === request.current) setError(true);
    }
  }
  useEffect(() => {
    void query();
    return () => {
      ++request.current;
    };
  }, []);

  async function act(
    action:
      "query" | "authenticate" | "prepare" | "publish" | "cancel" | "exit",
  ) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(false);
    const sequence = ++request.current;
    try {
      let next: StartupUpgradeReview | undefined;
      if (action === "query") {
        await query();
        return;
      }
      if (action === "exit") {
        await exit(0);
        return;
      }
      if (action === "authenticate" && view?.canAuthenticate === true) {
        next = await startupUpgradeApi.authenticate(password || null);
      } else if (
        action === "prepare" &&
        view?.canCheckAndBackup === true &&
        view.reviewToken
      ) {
        next = await startupUpgradeApi.prepare(view.reviewToken);
      } else if (
        action === "publish" &&
        canPublish &&
        view?.reviewToken &&
        view.checkpointId
      ) {
        next = await startupUpgradeApi.publish(
          view.reviewToken,
          view.checkpointId,
        );
      } else if (
        action === "cancel" &&
        canCancel &&
        view?.reviewToken &&
        view.checkpointId
      ) {
        next = await startupUpgradeApi.cancel(
          view.reviewToken,
          view.checkpointId,
        );
      }
      if (next && sequence === request.current) setView(next);
    } catch {
      if (sequence !== request.current) return;
      setError(true);
      if (action !== "exit") await query();
    } finally {
      setPassword("");
      busyRef.current = false;
      setBusy(false);
    }
  }
  const published = view?.status === "database_verified";
  const canPublish =
    view?.status === "checkpoint_ready" &&
    view.canStartUpgrade === true &&
    view.checkpointPresent &&
    !!view.reviewToken &&
    !!view.checkpointId;
  const canCancel =
    !!view?.reviewToken &&
    !!view.checkpointId &&
    (view.status === "checkpoint_ready" ||
      view.status === "cancellation_requires_verification");
  const statusKey =
    view?.status === "cancelled"
      ? "startupUpgrade.cancelled"
      : published
        ? "startupUpgrade.databaseOnly"
        : "startupUpgrade.verificationRequired";
  if (view?.status === "recovery_required") {
    return <SecretUnlockForm initialError="secret.recovery_required" />;
  }
  return (
    <main className="min-h-screen bg-background p-6 text-foreground">
      <div className="mx-auto max-w-3xl space-y-5 rounded-xl border bg-card p-6 shadow-sm">
        <h1 className="text-xl font-semibold">{t("startupUpgrade.title")}</h1>
        <p role="status" className="text-sm text-muted-foreground">
          {t(statusKey)}
        </p>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {t("startupUpgrade.queryFailed")}
          </p>
        )}
        {view?.canAuthenticate === true && (
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              void act("authenticate");
            }}
          >
            <Label htmlFor="upgrade-password">{t("secrets.password")}</Label>
            <Input
              id="upgrade-password"
              type="password"
              autoComplete="current-password"
              value={password}
              disabled={busy}
              onChange={(event) => setPassword(event.target.value)}
            />
            <Button type="submit" disabled={busy}>
              {t("startupUpgrade.authenticate")}
            </Button>
          </form>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => void act("query")}
          >
            {t("startupUpgrade.recheck")}
          </Button>
          {view?.canCheckAndBackup === true && (
            <Button
              disabled={busy || !view.reviewToken}
              onClick={() => void act("prepare")}
            >
              {t("startupUpgrade.checkBackup")}
            </Button>
          )}
          {canCancel && (
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => void act("cancel")}
            >
              {t("startupUpgrade.cancel")}
            </Button>
          )}
          {canPublish && (
            <Button disabled={busy} onClick={() => void act("publish")}>
              {t("startupUpgrade.start")}
            </Button>
          )}
        </div>
        {published && view?.reviewToken && (
          <div className="grid gap-3 md:grid-cols-2">
            {APPS.map((app) => (
              <UpgradeAppCard
                key={`${view.reviewToken}:${app.id}`}
                app={app}
                token={view.reviewToken!}
              />
            ))}
          </div>
        )}
        <p className="text-sm text-muted-foreground">
          {t("startupUpgrade.notComplete")}
        </p>
        <div className="flex flex-wrap justify-between gap-2 border-t pt-4">
          {/* Completion stays unavailable until the original owner exposes verified handoff. */}
          <Button disabled>{t("startupUpgrade.complete")}</Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => void act("exit")}
          >
            {t(published ? "startupUpgrade.later" : "startupUpgrade.exit")}
          </Button>
        </div>
        {published && (
          <p className="text-sm text-muted-foreground">
            {t("startupUpgrade.publishedExit")}
          </p>
        )}
      </div>
    </main>
  );
}
