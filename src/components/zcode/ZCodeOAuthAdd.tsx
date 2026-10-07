import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  accountErrorText,
  safeAccountError,
  type AccountError,
} from "@/lib/api/zcodeAccounts";
import {
  zcodeLoginApi,
  type LoginFamily,
  type LoginProgress,
} from "@/lib/api/zcodeLogin";
import { settingsApi } from "@/lib/api/settings";

type Write = "begin" | "key" | "decline" | "save";

export function ZCodeOAuthAdd({
  open,
  onClose,
  onSaved,
  libraryDataRoot,
}: {
  open: boolean;
  onClose: () => void;
  onSaved: () => Promise<void>;
  libraryDataRoot?: string;
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.login.${key}`, { defaultValue, ...values });
  const [family, setFamily] = useState<LoginFamily>("bigmodel");
  const [progress, setProgress] = useState<LoginProgress | null>(null);
  const [busy, setBusy] = useState<Write | null>(null);
  const [reading, setReading] = useState(false);
  const [paused, setPaused] = useState(false);
  const [error, setError] = useState<AccountError | null>(null);
  const [uncertain, setUncertain] = useState<Write | null>(null);
  const [updateDuplicate, setUpdateDuplicate] = useState(false);
  const [listRefreshFailed, setListRefreshFailed] = useState(false);
  const flow = useRef<string | null>(null);
  const generation = useRef(0);
  const readGeneration = useRef(0);
  const alive = useRef(false);
  const inFlight = useRef<Write | null>(null);
  const readInFlight = useRef(false);
  const notified = useRef(new Set<string>());
  const savedCallback = useRef(onSaved);
  savedCallback.current = onSaved;

  const abandon = useCallback(() => {
    generation.current += 1;
    readGeneration.current += 1;
    inFlight.current = null;
    readInFlight.current = false;
    const previous = flow.current;
    flow.current = null;
    if (previous) void zcodeLoginApi.cancel(previous).catch(() => {});
  }, []);

  const reset = useCallback(() => {
    setProgress(null);
    setBusy(null);
    setReading(false);
    setPaused(false);
    setError(null);
    setUncertain(null);
    setUpdateDuplicate(false);
    setListRefreshFailed(false);
  }, []);

  useEffect(() => {
    alive.current = open;
    if (open) {
      reset();
      setFamily("bigmodel");
    }
    return () => {
      alive.current = false;
      abandon();
    };
  }, [open, libraryDataRoot, abandon, reset]);

  const current = useCallback(
    (version: number, flowId?: string) =>
      alive.current &&
      generation.current === version &&
      (!flowId || flow.current === flowId),
    [],
  );

  const accept = useCallback(
    (next: LoginProgress, version: number) => {
      if (!current(version, next.flowId)) return;
      setProgress(next);
      setError(next.error ? safeAccountError(next.error) : null);
      setUncertain((previous) =>
        next.phase === "saved" ||
        next.phase === "cancelled" ||
        next.phase === "expired" ||
        next.phase === "failed" ||
        (previous === "save" && next.error?.remedy === "retrySave") ||
        ((previous === "key" || previous === "decline") &&
          next.error?.remedy === "retryKeyConsent") ||
        ((previous === "key" || previous === "decline") &&
          next.phase === "review")
          ? null
          : previous,
      );
      if (
        next.phase === "saved" &&
        next.saved &&
        !notified.current.has(next.flowId)
      ) {
        // Record before awaiting the list refresh. A saved receipt is never a Save retry.
        notified.current.add(next.flowId);
        void Promise.resolve()
          .then(() => savedCallback.current())
          .catch(() => {
            if (current(version, next.flowId)) setListRefreshFailed(true);
          });
      }
    },
    [current],
  );

  const read = useCallback(async () => {
    const flowId = flow.current;
    if (!alive.current || !flowId || inFlight.current || readInFlight.current)
      return;
    const version = generation.current;
    const readVersion = ++readGeneration.current;
    readInFlight.current = true;
    setReading(true);
    try {
      const next = await zcodeLoginApi.progress(flowId);
      if (
        current(version, flowId) &&
        readGeneration.current === readVersion &&
        next.flowId === flowId
      ) {
        accept(next, version);
      }
    } catch (cause) {
      if (current(version, flowId) && readGeneration.current === readVersion) {
        setError(safeAccountError(cause));
        setPaused(true);
      }
    } finally {
      if (current(version, flowId) && readGeneration.current === readVersion) {
        readInFlight.current = false;
        setReading(false);
      }
    }
  }, [accept, current]);

  useEffect(() => {
    if (
      !open ||
      paused ||
      reading ||
      busy ||
      !progress ||
      (progress.phase !== "waiting" && progress.phase !== "preparing")
    )
      return;
    const seconds = progress.authorization?.pollIntervalSec;
    // This polls a read-only snapshot; the backend owns official HTTP polling.
    const timer = setTimeout(
      () => void read(),
      Math.max(1, seconds ?? 2) * 1000,
    );
    return () => clearTimeout(timer);
  }, [open, progress, reading, busy, paused, read]);

  const openExternal = async (url: string) => {
    const version = generation.current;
    try {
      await settingsApi.openExternal(url);
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
    }
  };

  const begin = async () => {
    if (
      !alive.current ||
      inFlight.current ||
      readInFlight.current ||
      flow.current
    )
      return;
    const version = generation.current;
    inFlight.current = "begin";
    setBusy("begin");
    setError(null);
    try {
      const next = await (libraryDataRoot === undefined
        ? zcodeLoginApi.begin(family)
        : zcodeLoginApi.begin(family, libraryDataRoot));
      if (!current(version)) {
        void zcodeLoginApi.cancel(next.flowId).catch(() => {});
        return;
      }
      flow.current = next.flowId;
      accept(next, version);
      if (next.authorization && next.phase === "waiting")
        await openExternal(next.authorization.url);
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
    } finally {
      if (current(version)) {
        inFlight.current = null;
        setBusy(null);
      }
    }
  };

  const write = async (action: Exclude<Write, "begin">) => {
    const flowId = flow.current;
    if (
      !alive.current ||
      !flowId ||
      !progress ||
      inFlight.current ||
      readInFlight.current
    )
      return;
    if (
      (action === "key" || action === "decline") &&
      (progress.phase !== "keyRequired" || keyUncertain)
    )
      return;
    if (action === "key" && (!progress.account || !progress.project)) return;
    if (action === "save" && (progress.phase !== "review" || saveUncertain))
      return;
    const version = generation.current;
    inFlight.current = action;
    setBusy(action);
    setError(null);
    try {
      const next =
        action === "key"
          ? await zcodeLoginApi.confirmKey(
              flowId,
              progress.project!.organizationId,
              progress.project!.projectId,
            )
          : action === "decline"
            ? await zcodeLoginApi.declineKey(flowId)
            : await zcodeLoginApi.save(flowId, updateDuplicate);
      if (next.flowId === flowId) accept(next, version);
    } catch (cause) {
      if (current(version, flowId)) {
        setError(safeAccountError(cause));
        setUncertain(action);
      }
    } finally {
      if (current(version, flowId)) {
        inFlight.current = null;
        setBusy(null);
      }
    }
  };

  const close = () => {
    alive.current = false;
    abandon();
    onClose();
  };
  const startAnother = () => {
    abandon();
    reset();
  };
  const cancelRead = () => {
    readGeneration.current += 1;
    readInFlight.current = false;
    setReading(false);
    setPaused(true);
  };
  const phase = progress?.phase;
  const blocked = busy !== null || reading;
  const keyUncertain =
    progress?.error?.remedy !== "retryKeyConsent" &&
    (uncertain === "key" ||
      uncertain === "decline" ||
      !!progress?.keyMayExist ||
      !!progress?.error);
  const saveUncertain =
    progress?.error?.remedy !== "retrySave" &&
    (uncertain === "save" || !!progress?.error);
  const title = !phase
    ? copy("title", "Add a sign-in account")
    : phase === "waiting"
      ? copy("waiting", "Waiting for official authorization")
      : phase === "preparing"
        ? copy("preparing", "Identity verified; preparing connections")
        : phase === "keyRequired"
          ? copy("keyTitle", "Complete the Coding Plan connection")
          : phase === "review"
            ? copy("review", "Confirm this account before saving")
            : phase === "saved"
              ? copy("saved", "Account save result")
              : phase === "expired"
                ? copy("expired", "Official authorization expired")
                : phase === "cancelled"
                  ? copy("cancelled", "Official sign-in cancelled")
                  : copy("failed", "Official sign-in could not complete");
  const capability = (value: "ready" | "unknown" | "unavailable") =>
    value === "ready"
      ? copy("ready", "Ready")
      : value === "unknown"
        ? copy("unknown", "Unknown")
        : copy("unavailable", "Unavailable");
  const failureKey = error?.code.slice("zcode.account.".length);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <DialogContent
        className="max-w-xl"
        closeButtonLabel={copy("close", "Close sign-in")}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {copy(
              "noSwitch",
              "Adding an account keeps the current ZCode sign-in unchanged.",
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 overflow-y-auto px-6 py-5">
          {!progress && (
            <>
              <p className="text-sm">
                {copy(
                  "intro",
                  "Choose the official account platform. LoongPort will verify the returned identity and prepare its connections before you save.",
                )}
              </p>
              <Select
                value={family}
                onValueChange={(value) => setFamily(value as LoginFamily)}
                disabled={blocked}
              >
                <SelectTrigger
                  aria-label={copy("family", "Official account platform")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="bigmodel">BigModel</SelectItem>
                  <SelectItem value="zai">z.ai</SelectItem>
                </SelectContent>
              </Select>
              <p className="rounded-md border border-border-default bg-muted/30 p-3 text-sm">
                {copy(
                  "officialBrowser",
                  "The official authorization page opens in your system browser. Passwords are entered only on the official page, never in LoongPort.",
                )}
              </p>
            </>
          )}
          {progress && (
            <>
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
                <dt className="text-muted-foreground">
                  {copy("family", "Official account platform")}
                </dt>
                <dd>{progress.family === "bigmodel" ? "BigModel" : "z.ai"}</dd>
                {progress.account && (
                  <>
                    <dt className="text-muted-foreground">
                      {copy("account", "Account")}
                    </dt>
                    <dd className="break-all">{progress.account.label}</dd>
                    <dt className="text-muted-foreground">
                      {copy("identity", "Identity source")}
                    </dt>
                    <dd>
                      {copy("officialIdentity", "Verified official sign-in")}
                    </dd>
                  </>
                )}
                {progress.project && (
                  <>
                    <dt className="text-muted-foreground">
                      {copy("organization", "Personal organization")}
                    </dt>
                    <dd className="break-all">
                      {progress.project.organizationName ??
                        progress.project.organizationId}
                    </dd>
                    <dt className="text-muted-foreground">
                      {copy("project", "Personal project")}
                    </dt>
                    <dd className="break-all">
                      {progress.project.projectName ??
                        progress.project.projectId}
                    </dd>
                  </>
                )}
              </dl>
              {phase === "waiting" && (
                <>
                  <p
                    className="rounded-md bg-muted/30 p-3 text-sm"
                    role="status"
                  >
                    {copy(
                      "waitingHint",
                      "Complete sign-in on the official page, then return here. Review the actual returned account before saving if your browser reuses an existing sign-in.",
                    )}
                  </p>
                  {progress.authorization && (
                    <p className="text-xs text-muted-foreground">
                      {copy("expiresAt", "Authorization valid until {{time}}", {
                        time: new Date(
                          progress.authorization.expiresAt * 1000,
                        ).toLocaleString(),
                      })}
                    </p>
                  )}
                </>
              )}
              {phase === "preparing" && (
                <p role="status" className="text-sm">
                  {copy(
                    "preparingHint",
                    "Preparing the official Start and Coding connections for this account.",
                  )}
                </p>
              )}
              {progress.connections && (
                <div className="space-y-1 rounded-md border border-border-default p-3 text-sm">
                  <p>
                    {copy("startConnection", "Start Plan: {{status}}", {
                      status: capability(progress.connections.start),
                    })}
                  </p>
                  <p>
                    {copy("codingConnection", "Coding Plan: {{status}}", {
                      status: capability(progress.connections.coding),
                    })}
                  </p>
                  {progress.connections.needsKey && (
                    <p className="text-muted-foreground">
                      {copy(
                        "connectionPending",
                        "Coding connection is pending; the Start result is shown separately.",
                      )}
                    </p>
                  )}
                </div>
              )}
              {phase === "keyRequired" && (
                <>
                  <p className="text-sm">
                    {copy(
                      "keyIntro",
                      "This account needs a Coding Plan Key. Creating it grants persistent access for the account and personal project shown above.",
                    )}
                  </p>
                  <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
                    <dt className="text-muted-foreground">
                      {copy("keyName", "Key name")}
                    </dt>
                    <dd>zcode-api-key</dd>
                    <dt className="text-muted-foreground">
                      {copy("keyPurpose", "Purpose")}
                    </dt>
                    <dd>
                      {copy(
                        "keyPurposeDetail",
                        "Prepare this account's official Coding Plan model connection.",
                      )}
                    </dd>
                    <dt className="text-muted-foreground">
                      {copy("keyStorage", "Storage and revocation")}
                    </dt>
                    <dd>
                      {copy(
                        "keyStorageDetail",
                        "Encrypted on this computer in the LoongPort vault. Revoke it on the official platform's Key management page.",
                      )}
                    </dd>
                  </dl>
                  <p className="rounded-md border border-border-default bg-muted/30 p-3 text-sm">
                    {copy(
                      "keyConsent",
                      "Your explicit consent is required for this Key creation. An existing usable Key is reused instead of creating another.",
                    )}
                  </p>
                </>
              )}
              {(phase === "review" || phase === "saved") && (
                <>
                  {phase === "review" && (
                    <p className="text-sm">
                      {copy(
                        "saveScope",
                        "Save this account's login session, required official connection credentials and masked label in the local encrypted vault. Any existing Key supplied for this connection will be reused and encrypted with your confirmation.",
                      )}
                    </p>
                  )}
                  <p className="text-xs text-muted-foreground">
                    {copy(
                      "noModelCall",
                      "Connection status does not confirm a real model request. No model request is made or quota spent by this workflow.",
                    )}
                  </p>
                </>
              )}
              {phase === "review" && progress.account?.duplicate && (
                <div className="space-y-3 rounded-md border border-border-default p-3 text-sm">
                  <p>
                    {copy(
                      "duplicate",
                      "This account is already saved. Keep the existing account by default, or explicitly update it after the new candidate is complete.",
                    )}
                  </p>
                  <label className="flex items-start gap-2">
                    <Checkbox
                      checked={updateDuplicate}
                      disabled={blocked || saveUncertain}
                      onCheckedChange={(checked) =>
                        setUpdateDuplicate(checked === true)
                      }
                      aria-label={copy(
                        "updateDuplicate",
                        "Explicitly update this existing account",
                      )}
                    />
                    <span>
                      {copy(
                        "updateDuplicate",
                        "Explicitly update this existing account",
                      )}
                    </span>
                  </label>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={blocked || saveUncertain}
                    onClick={startAnother}
                  >
                    {copy(
                      "differentAccount",
                      "Sign in with a different account",
                    )}
                  </Button>
                </div>
              )}
              {progress.keyCreated && (
                <p className="text-sm">
                  {copy(
                    "keyCreated",
                    "The official Key was created. You can manage or revoke it on the official platform.",
                  )}
                </p>
              )}
              {progress.keyMayExist && (
                <p className="rounded-md border border-border-default p-3 text-sm">
                  {copy(
                    "keyMayExist",
                    "The official Key may already exist. Query the original result and inspect the original project before retrying. LoongPort will not create another Key or delete it automatically.",
                  )}
                </p>
              )}
              {(phase === "keyRequired" ||
                phase === "review" ||
                progress.keyCreated ||
                progress.keyMayExist) &&
                progress.keyManagementUrl && (
                  <Button
                    variant="link"
                    className="h-auto p-0 text-sm"
                    onClick={() => void openExternal(progress.keyManagementUrl)}
                  >
                    {copy("manageKeys", "Open official Key management")}
                  </Button>
                )}
              {phase === "saved" && progress.saved && (
                <p
                  role="status"
                  className="rounded-md border border-border-default bg-muted/30 p-3 text-sm"
                >
                  {progress.saved.outcome === "saved"
                    ? copy("savedNew", "Account saved in the encrypted vault.")
                    : progress.saved.outcome === "refreshed"
                      ? copy(
                          "savedUpdate",
                          "Existing account updated in the encrypted vault.",
                        )
                      : copy("savedKept", "Kept the existing saved account.")}
                </p>
              )}
              {listRefreshFailed && (
                <p role="alert" className="text-sm text-destructive">
                  {copy(
                    "refreshFailed",
                    "Account saved, but the account list could not refresh. Return to the list and refresh it; do not save again.",
                  )}
                </p>
              )}
              {phase === "saved" && (
                <p className="text-sm">
                  {copy(
                    "savedNext",
                    "You can add another account. To switch later, choose the account in the list and review the client impact.",
                  )}
                </p>
              )}
            </>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {failureKey === "operation_failed"
                ? progress
                  ? copy(
                      "operationFailed",
                      "The sign-in result could not be verified. Query the original result to check this account's progress.",
                    )
                  : copy(
                      "beginFailed",
                      "No official authorization result was received. Try starting official sign-in again.",
                    )
                : t(`zcode.accounts.errors.${failureKey}`, {
                    defaultValue:
                      accountErrorText[
                        failureKey as keyof typeof accountErrorText
                      ] ?? accountErrorText.operation_failed,
                  })}
            </p>
          )}
          {uncertain && (
            <p className="text-sm">
              {copy(
                "uncertain",
                "The response was not confirmed. Query the original result before another action; do not repeat the write.",
              )}
            </p>
          )}
          {progress && (
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={blocked}
                onClick={() => {
                  setPaused(false);
                  void read();
                }}
              >
                {copy("queryOriginal", "Query original result")}
              </Button>
              {reading && (
                <Button variant="outline" size="sm" onClick={cancelRead}>
                  {copy("cancelRead", "Cancel reading")}
                </Button>
              )}
              {phase === "waiting" && progress.authorization && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void openExternal(progress.authorization!.url)}
                >
                  {copy("reopenBrowser", "Reopen official authorization")}
                </Button>
              )}
            </div>
          )}
        </div>
        <DialogFooter className="flex-wrap">
          <Button variant="outline" onClick={close}>
            {phase === "saved"
              ? copy("backToList", "Return to accounts")
              : copy("cancel", "Cancel")}
          </Button>
          {!progress && (
            <Button disabled={blocked} onClick={() => void begin()}>
              {copy("continue", "Continue official sign-in")}
            </Button>
          )}
          {phase === "keyRequired" && (
            <>
              <Button
                variant="outline"
                disabled={blocked || !!keyUncertain}
                onClick={() => void write("decline")}
              >
                {copy("declineKey", "Not now; keep connection pending")}
              </Button>
              <Button
                disabled={
                  blocked ||
                  !!keyUncertain ||
                  !progress?.account ||
                  !progress.project
                }
                onClick={() => void write("key")}
              >
                {copy("confirmKey", "Authorize creating and saving this Key")}
              </Button>
            </>
          )}
          {phase === "review" && (
            <Button
              disabled={blocked || saveUncertain}
              onClick={() => void write("save")}
            >
              {copy("save", "Save to encrypted account vault")}
            </Button>
          )}
          {phase === "saved" && (
            <Button disabled={blocked} onClick={startAnother}>
              {copy("addAnother", "Add another account")}
            </Button>
          )}
          {(phase === "failed" ||
            phase === "expired" ||
            phase === "cancelled") && (
            <Button
              disabled={blocked || !!progress?.keyMayExist}
              onClick={startAnother}
            >
              {copy("restart", "Start a new sign-in")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
