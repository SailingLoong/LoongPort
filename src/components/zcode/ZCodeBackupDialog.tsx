import { useId, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  accountErrorText,
  safeAccountError,
  type AccountError,
} from "@/lib/api/zcodeAccounts";
import { zcodeBackupApi, type BundleExportResult } from "@/lib/api/zcodeBackup";

type BackupDialogProps = {
  open: boolean;
  onClose: () => void;
  profiles: { id: string; family: "bigmodel" | "zai"; label: string | null }[];
  catalogRevision: string;
  libraryDataRoot?: string;
};

export function ZCodeBackupDialog(props: BackupDialogProps) {
  // A new source/catalog gets a new local session, including before effects run.
  return props.open ? (
    <BackupSession
      key={JSON.stringify([
        props.libraryDataRoot ?? null,
        props.catalogRevision,
      ])}
      {...props}
    />
  ) : null;
}

function BackupSession({
  onClose,
  profiles,
  catalogRevision,
  libraryDataRoot,
}: BackupDialogProps) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.backup.${key}`, { defaultValue, ...values });
  const inputId = useId();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [choosing, setChoosing] = useState(false);
  const [reading, setReading] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [requestId, setRequestId] = useState<string | null>(null);
  const [result, setResult] = useState<BundleExportResult | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<AccountError | null>(null);
  const alive = useRef(false);
  const generation = useRef(0);
  const inFlight = useRef(false);
  const readInFlight = useRef(false);
  const originalRequest = useRef<string | null>(null);
  const secrets = useRef({ password: "", passwordConfirmation: "" });

  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      generation.current += 1;
      secrets.current = { password: "", passwordConfirmation: "" };
    };
  }, []);

  const clearPasswords = () => {
    secrets.current = { password: "", passwordConfirmation: "" };
    setPassword("");
    setConfirmation("");
  };
  const close = () => {
    alive.current = false;
    generation.current += 1;
    clearPasswords();
    setDismissed(true);
    onClose();
  };
  const current = (version: number) =>
    alive.current && generation.current === version;
  const applyResult = (next: BundleExportResult) => {
    setResult(next);
    setUncertain(next.status === "unknown");
    setError(next.error);
  };
  const selectedProfiles = profiles.filter((profile) =>
    selected.has(profile.id),
  );
  const canSubmit =
    selectedProfiles.length > 0 &&
    selectedProfiles.length <= 50 &&
    password.trim().length > 0 &&
    password === confirmation &&
    !choosing &&
    !requestId;

  const submit = async () => {
    if (inFlight.current || originalRequest.current || !canSubmit) return;
    inFlight.current = true;
    setChoosing(true);
    setError(null);
    const version = ++generation.current;
    const profileIds = selectedProfiles.map((profile) => profile.id);
    let destination: string | null;
    try {
      destination = await save({
        defaultPath: "zcode-accounts.zsb",
        filters: [
          {
            name: copy("fileType", "ZCode encrypted backup"),
            extensions: ["zsb"],
          },
        ],
      });
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
      destination = null;
    }
    // Dismissal or a changed source must never turn a late chooser into a write.
    if (!current(version)) return;
    setChoosing(false);
    if (!destination) {
      clearPasswords();
      inFlight.current = false;
      return;
    }
    const credentials = secrets.current;
    clearPasswords();
    try {
      const id = crypto.randomUUID();
      originalRequest.current = id;
      setRequestId(id);
      const next = await zcodeBackupApi.exportBundle({
        requestId: id,
        ...(libraryDataRoot === undefined ? {} : { dataRoot: libraryDataRoot }),
        catalogRevision,
        profileIds,
        destination,
        password: credentials.password,
        passwordConfirmation: credentials.passwordConfirmation,
      });
      if (current(version)) applyResult(next);
    } catch (cause) {
      if (current(version)) {
        setError(safeAccountError(cause));
        setUncertain(originalRequest.current !== null);
      }
    } finally {
      credentials.password = "";
      credentials.passwordConfirmation = "";
      if (current(version)) inFlight.current = false;
    }
  };

  const query = async () => {
    const id = originalRequest.current;
    if (!alive.current || !id || readInFlight.current) return;
    readInFlight.current = true;
    setReading(true);
    setError(null);
    // A later query owns the displayed result even if the original write replies late.
    const version = ++generation.current;
    try {
      const next = await zcodeBackupApi.result(id);
      if (current(version)) applyResult(next);
    } catch (cause) {
      if (current(version)) {
        setUncertain(true);
        setError(safeAccountError(cause));
      }
    } finally {
      if (current(version)) {
        readInFlight.current = false;
        setReading(false);
      }
    }
  };

  if (dismissed) return null;
  const errorKey = error?.code.slice("zcode.account.".length);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogContent
        className="max-w-xl"
        closeButtonLabel={copy("close", "Close backup")}
      >
        <DialogHeader>
          <DialogTitle>{copy("title", "Encrypted account backup")}</DialogTitle>
          <DialogDescription>
            {copy(
              "intro",
              "Select up to 50 saved accounts to back up as an encrypted .zsb file.",
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 overflow-y-auto px-6 py-5">
          <div className="space-y-2 rounded-md border border-border-default bg-muted/30 p-3 text-sm">
            <p>
              {copy(
                "sensitiveScope",
                "The backup contains sensitive sign-in sessions for the selected accounts. Device IDs, machine keys, project history and other LoongPort secrets are excluded.",
              )}
            </p>
            <p>
              {copy(
                "environmentScope",
                "Restore in the original operating system, user and home environment. Compatibility with another computer or platform is not verified.",
              )}
            </p>
          </div>
          {!requestId && (
            <>
              <fieldset disabled={choosing} className="space-y-2">
                <legend className="mb-2 text-sm font-medium">
                  {copy("selectAccounts", "Accounts to back up")}
                </legend>
                <div className="max-h-52 space-y-2 overflow-y-auto rounded-md border border-border-default p-3">
                  {profiles.length === 0 && (
                    <p className="text-sm text-muted-foreground">
                      {copy("empty", "No saved accounts are available.")}
                    </p>
                  )}
                  {profiles.map((profile, index) => (
                    <label
                      key={profile.id}
                      htmlFor={`${inputId}-account-${index}`}
                      className="flex items-center gap-3 text-sm"
                    >
                      <Checkbox
                        id={`${inputId}-account-${index}`}
                        checked={selected.has(profile.id)}
                        disabled={
                          choosing ||
                          (!selected.has(profile.id) &&
                            selectedProfiles.length >= 50)
                        }
                        onCheckedChange={(checked) =>
                          setSelected((previous) => {
                            const next = new Set(previous);
                            if (checked && next.size < 50) next.add(profile.id);
                            else if (!checked) next.delete(profile.id);
                            return next;
                          })
                        }
                      />
                      <span className="min-w-0 break-all">
                        {profile.label && <span>{profile.label} · </span>}
                        {profile.family === "bigmodel" ? "BigModel" : "z.ai"}
                      </span>
                    </label>
                  ))}
                </div>
              </fieldset>
              <p className="text-sm" aria-live="polite">
                {copy("selected", "Selected: {{count}} / 50", {
                  count: selectedProfiles.length,
                })}
              </p>
              <div className="space-y-2">
                <Label htmlFor={`${inputId}-password`}>
                  {copy("password", "Backup password")}
                </Label>
                <Input
                  id={`${inputId}-password`}
                  type="password"
                  autoComplete="new-password"
                  disabled={choosing}
                  value={password}
                  onChange={(event) => {
                    secrets.current.password = event.target.value;
                    setPassword(event.target.value);
                  }}
                />
                <Label htmlFor={`${inputId}-confirmation`}>
                  {copy("confirmation", "Confirm backup password")}
                </Label>
                <Input
                  id={`${inputId}-confirmation`}
                  type="password"
                  autoComplete="new-password"
                  disabled={choosing}
                  value={confirmation}
                  onChange={(event) => {
                    secrets.current.passwordConfirmation = event.target.value;
                    setConfirmation(event.target.value);
                  }}
                />
                <p className="text-xs text-muted-foreground">
                  {copy(
                    "passwordHint",
                    "Keep this local backup password safe. Both entries must match exactly, including surrounding spaces.",
                  )}
                </p>
              </div>
            </>
          )}
          {requestId && (
            <div
              role="status"
              className="space-y-2 rounded-md border border-border-default p-3 text-sm"
            >
              {uncertain ? (
                <p>
                  {copy(
                    "unknown",
                    "The backup result is unknown. Query the original request before starting another backup.",
                  )}
                </p>
              ) : result?.status === "saved" ? (
                <>
                  <p>
                    {copy(
                      "saved",
                      "Backup saved and verified: {{count}} account(s).",
                      { count: result.count! },
                    )}
                  </p>
                  <p className="break-all">{result.destination}</p>
                </>
              ) : result?.status === "failed" ? (
                <p>
                  {copy(
                    "failed",
                    "The backup could not be verified as saved. Review the reported error and query the original result.",
                  )}
                </p>
              ) : (
                <p>
                  {copy(
                    "working",
                    "Backup submitted. Waiting for durable encrypted write and authenticated read-back verification.",
                  )}
                </p>
              )}
            </div>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {t(`zcode.accounts.errors.${errorKey}`, {
                defaultValue:
                  accountErrorText[errorKey as keyof typeof accountErrorText] ??
                  accountErrorText.operation_failed,
              })}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            {copy(
              "closeHint",
              "Closing this dialog does not undo a submitted backup. The backend may finish the original request after this dialog closes.",
            )}
          </p>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={close}>
            {requestId ? copy("done", "Close") : copy("cancel", "Cancel")}
          </Button>
          {requestId ? (
            result?.status !== "saved" && (
              <Button onClick={() => void query()} disabled={reading}>
                {reading
                  ? copy("querying", "Checking original result…")
                  : copy("query", "Query original result")}
              </Button>
            )
          ) : (
            <Button onClick={() => void submit()} disabled={!canSubmit}>
              {choosing
                ? copy("choosing", "Choosing backup location…")
                : copy("submit", "Choose location and back up")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
