import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  accountErrorText,
  safeAccountError,
  zcodeAccountsApi,
  type AccountError,
  type ContextSelection,
  type SourceContext,
} from "@/lib/api/zcodeAccounts";

type ActionKind =
  "capture" | "switch" | "archive" | "confirm" | "recapture" | "delete";
type ReviewedAction = {
  kind: ActionKind;
  id?: string;
  source: ContextSelection;
  contextRevision?: string;
  catalogRevision?: string;
  recoveryRevision: string;
};
const initialSource: ContextSelection = {
  installPath: "/Applications/ZCode.app",
  dataRoot: "",
  keyMode: "unknown",
};
async function passive<T>(operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
  } catch (cause) {
    // Query error state is cached too. Keep arbitrary backend payloads out of it.
    throw safeAccountError(cause);
  }
}

/** Selection is ephemeral; native admission and account identity stay in Rust. */
export function ZCodeAccountPanel({
  disabled = false,
  onBusyChange,
}: {
  disabled?: boolean;
  onBusyChange?: (busy: boolean) => void;
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string>,
  ) => t(`zcode.accounts.${key}`, { defaultValue, ...values });
  const [source, setSource] = useState<ContextSelection>(initialSource);
  const [context, setContext] = useState<SourceContext | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [action, setAction] = useState<ReviewedAction | null>(null);
  const [error, setError] = useState<AccountError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [needsReview, setNeedsReview] = useState(false);
  const recovery = useQuery({
    queryKey: ["zcodeAccountRecovery"],
    queryFn: () => passive(zcodeAccountsApi.recoveryStatus),
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const catalog = useQuery({
    queryKey: [
      "zcodeAccountCatalog",
      source.installPath,
      source.dataRoot,
      source.keyMode,
      context?.contextRevision,
    ],
    queryFn: () =>
      passive(() => zcodeAccountsApi.status(source, context!.contextRevision)),
    enabled: context !== null,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  useEffect(() => {
    onBusyChange?.(busy);
  }, [busy, onBusyChange]);
  useEffect(() => () => onBusyChange?.(false), [onBusyChange]);
  const locked = busy || disabled;
  const loading =
    recovery.isFetching || (context !== null && catalog.isFetching);
  const localReady =
    !!recovery.data && !recovery.isError && !recovery.isFetching && !locked;
  const reviewedSourceReady = localReady && !!context;
  const catalogReady =
    reviewedSourceReady &&
    !!catalog.data &&
    !catalog.isError &&
    !catalog.isFetching &&
    !needsReview;
  const pending = recovery.data?.pending || catalog.data?.pending;
  const unconfirmed =
    recovery.data?.nativeUnconfirmed || catalog.data?.nativeUnconfirmed;
  const full =
    (recovery.data?.records.filter((record) => !record.latestCompleted)
      .length ?? 0) >= 2;
  const ordinaryReady = catalogReady && !pending && !unconfirmed;
  const displayedError =
    error ??
    (recovery.isError ? safeAccountError(recovery.error) : null) ??
    (context && catalog.isError ? safeAccountError(catalog.error) : null);
  const errorText = (failure: AccountError) => {
    const key = failure.committed
      ? "committed_recovery_required"
      : failure.code.slice("zcode.account.".length);
    const fallback =
      accountErrorText[key as keyof typeof accountErrorText] ??
      accountErrorText.operation_failed;
    return t(`zcode.accounts.errors.${key}`, { defaultValue: fallback });
  };
  const begin = () => {
    if (inFlight.current || disabled) return false;
    inFlight.current = true;
    setBusy(true);
    setNotice(null);
    return true;
  };
  const finish = () => {
    inFlight.current = false;
    setBusy(false);
  };
  const selectSource = (next: ContextSelection) => {
    if (inFlight.current || disabled) return;
    setSource(next);
    setContext(null);
    setAction(null);
    setError(null);
    setNotice(null);
    setNeedsReview(false);
  };
  const inspect = async () => {
    if (!begin()) return;
    setError(null);
    setContext(null);
    setAction(null);
    try {
      const inspected = await zcodeAccountsApi.inspect(source);
      setContext(inspected);
      setNeedsReview(false);
    } catch (cause) {
      setError(safeAccountError(cause));
    } finally {
      finish();
    }
  };
  const refresh = async () => {
    if (!begin()) return;
    setAction(null);
    setError(null);
    try {
      const local = await recovery.refetch();
      const accounts = context ? await catalog.refetch() : null;
      if (local.error || accounts?.error) throw local.error ?? accounts?.error;
      setNeedsReview(false);
    } catch (cause) {
      setError(safeAccountError(cause));
      setNeedsReview(true);
    } finally {
      finish();
    }
  };
  const review = (kind: ActionKind, id?: string) => {
    if (!localReady || !recovery.data || inFlight.current) return;
    if (kind !== "archive" && kind !== "delete" && !reviewedSourceReady) return;
    if (
      (kind === "capture" || kind === "switch" || kind === "recapture") &&
      !catalogReady
    )
      return;
    if ((kind === "capture" || kind === "switch") && !ordinaryReady) return;
    if (
      kind === "switch" &&
      (full ||
        !catalog.data?.profiles.some(
          (profile) => profile.id === id && profile.family === context?.family,
        ))
    )
      return;
    if (
      kind === "delete" &&
      !recovery.data.records.some(
        (record) =>
          record.id === id && record.disposition !== "native-unconfirmed",
      )
    )
      return;
    setError(null);
    setNotice(null);
    setAction({
      kind,
      id,
      source: { ...source },
      contextRevision: context?.contextRevision,
      catalogRevision: catalog.data?.revision,
      recoveryRevision: recovery.data.revision,
    });
  };
  const perform = async () => {
    if (!action || !begin()) return;
    const selected = action;
    setError(null);
    try {
      // An open confirmation never silently adopts a newly fetched revision.
      if (
        selected.recoveryRevision !== recovery.data?.revision ||
        (selected.kind !== "archive" &&
          selected.kind !== "delete" &&
          selected.contextRevision !== context?.contextRevision) ||
        ((selected.kind === "capture" ||
          selected.kind === "switch" ||
          selected.kind === "recapture") &&
          selected.catalogRevision !== catalog.data?.revision)
      ) {
        throw {
          code: "zcode.account.recovery_changed",
          remedy: "refreshContext",
          committed: false,
        };
      }
      let result: string | void = undefined;
      switch (selected.kind) {
        case "capture":
          result = await zcodeAccountsApi.capture(
            selected.source,
            selected.contextRevision!,
            selected.catalogRevision!,
          );
          break;
        case "switch":
          result = await zcodeAccountsApi.switch(
            selected.source,
            selected.contextRevision!,
            selected.id!,
            selected.catalogRevision!,
          );
          break;
        case "archive":
          result = await zcodeAccountsApi.archive(selected.recoveryRevision);
          break;
        case "confirm":
          await zcodeAccountsApi.confirmRecovery(
            selected.source,
            selected.contextRevision!,
            selected.id!,
            selected.recoveryRevision,
          );
          break;
        case "recapture":
          result = await zcodeAccountsApi.recapture(
            selected.source,
            selected.contextRevision!,
            selected.id!,
            selected.recoveryRevision,
            selected.catalogRevision!,
          );
          break;
        case "delete":
          await zcodeAccountsApi.deleteRecovery(
            selected.id!,
            selected.recoveryRevision,
          );
          break;
      }
      setAction(null);
      if (selected.kind === "capture" || selected.kind === "recapture") {
        setNotice(
          result === "refreshed"
            ? copy("refreshed", "Saved account refreshed locally.")
            : copy("saved", "Account saved locally."),
        );
      } else if (selected.kind === "switch") {
        setNotice(
          result === "refreshed"
            ? copy("refreshed", "Saved account refreshed locally.")
            : copy(
                "switched",
                "Switched {{root}} locally. Start official ZCode and verify the account there.",
                { root: selected.source.dataRoot },
              ),
        );
      } else if (selected.kind === "archive") {
        setNotice(
          result === "nothingPending"
            ? copy("nothingPending", "No pending recovery to archive.")
            : copy("archived", "Pending recovery preserved locally."),
        );
      } else if (selected.kind === "confirm") {
        setNotice(copy("confirmed", "Selected recovery record confirmed."));
      } else {
        setNotice(
          copy("deleted", "Selected recovery record permanently deleted."),
        );
      }
      const local = await recovery.refetch();
      const accounts = context ? await catalog.refetch() : null;
      if (local.error || accounts?.error) {
        setError(safeAccountError(local.error ?? accounts?.error));
        setNeedsReview(true);
        return;
      }
      setNeedsReview(false);
    } catch (cause) {
      setError(safeAccountError(cause));
      setNeedsReview(true);
      setAction(null);
    } finally {
      finish();
    }
  };
  const actionCopy = () => {
    const root = action?.source.dataRoot ?? "";
    const id = action?.id ?? "";
    switch (action?.kind) {
      case "capture":
        return {
          title: copy("captureTitle", "Save current ZCode account?"),
          message: copy(
            "captureMessage",
            "Read the current native session from {{root}} and persist an encrypted account in LoongPort's local vault for later account switching. This explicitly authorizes reading and saving the session; inspecting or refreshing does not save it.",
            { root },
          ),
          confirm: copy("captureConfirm", "Save encrypted account"),
        };
      case "switch":
        return {
          title: copy("switchTitle", "Switch ZCode account?"),
          message: copy(
            "switchMessage",
            "Switch the native session in {{root}} to saved account {{id}}. Quit ZCode normally first. LoongPort preserves the outgoing saved account and local recovery material. Current account status remains unknown until independently verified.",
            { root, id },
          ),
          confirm: copy("switchConfirm", "Switch to this account"),
        };
      case "archive":
        return {
          title: copy("archiveTitle", "Preserve pending recovery?"),
          message: copy(
            "archiveMessage",
            "Preserve the pending transaction in LoongPort's encrypted local recovery store. This does not inspect, replay or overwrite the native ZCode session. The record will still need confirmation before permanent cleanup.",
          ),
          confirm: copy("archiveConfirm", "Preserve pending recovery"),
        };
      case "confirm":
        return {
          title: copy("confirmTitle", "Confirm selected recovery record?"),
          message: copy(
            "confirmMessage",
            "Read the native session in {{root}} and check whether it matches recovery record {{id}}. This confirms only this exact record and does not replace the native session. Quit ZCode normally first.",
            { root, id },
          ),
          confirm: copy("confirmConfirm", "Check selected recovery record"),
        };
      case "recapture":
        return {
          title: copy("recaptureTitle", "Recapture after official sign-in?"),
          message: copy(
            "recaptureMessage",
            "First sign in again through official ZCode and quit ZCode normally. Read the current session from {{root}}, persist it in LoongPort's encrypted local vault for later switching, and use this explicit capture to confirm recovery record {{id}}. Other records are not confirmed.",
            { root, id },
          ),
          confirm: copy("recaptureConfirm", "Save and confirm selected record"),
        };
      case "delete":
        return {
          title: copy(
            "deleteTitle",
            "Permanently delete selected recovery record?",
          ),
          message: copy(
            "deleteMessage",
            "Permanently delete recovery record {{id}} from LoongPort's local vault. Its source and target recovery material cannot be recovered, and this record can no longer be used to check or recover that transaction. This affects only the selected record; saved accounts are not deleted.",
            { id },
          ),
          confirm: copy("deleteConfirm", "Delete selected record permanently"),
        };
      default:
        return { title: "", message: "", confirm: "" };
    }
  };
  const confirmation = actionCopy();
  return (
    <section
      className="space-y-4 border-t pt-4"
      aria-label={copy("title", "Saved ZCode accounts")}
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h3 className="font-semibold">
            {copy("title", "Saved ZCode accounts")}
          </h3>
          <p className="text-sm text-muted-foreground">
            {copy(
              "description",
              "Save and switch personal Z.ai or BigModel sessions in the encrypted local LoongPort vault. Select and inspect the source for each visit.",
            )}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={locked || loading || !!action}
          onClick={() => void refresh()}
        >
          {copy("refresh", "Refresh account status")}
        </Button>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-1">
          <Label htmlFor="zcode-account-install">
            {copy("install", "ZCode installation")}
          </Label>
          <Input
            id="zcode-account-install"
            value={source.installPath}
            disabled={locked || !!action}
            onChange={(event) =>
              selectSource({ ...source, installPath: event.target.value })
            }
          />
        </div>
        <div className="space-y-1">
          <Label htmlFor="zcode-account-data-root">
            {copy("dataRoot", "ZCode data directory")}
          </Label>
          <Input
            id="zcode-account-data-root"
            value={source.dataRoot}
            placeholder="/absolute/path/.zcode/v2"
            disabled={locked || !!action}
            onChange={(event) =>
              selectSource({ ...source, dataRoot: event.target.value })
            }
          />
        </div>
      </div>
      <p className="text-xs text-muted-foreground">
        {copy(
          "sourceHelp",
          "Enter the exact absolute path to the .zcode/v2 directory inside the data base directory configured in official ZCode. Enter this nested directory, not the data base directory itself. Custom key contexts and team accounts are unsupported. Quit ZCode normally before account actions.",
        )}
      </p>
      <label className="flex items-start gap-2 text-sm">
        <Checkbox
          checked={source.keyMode === "standard"}
          disabled={locked || !!action}
          onCheckedChange={(checked) =>
            selectSource({
              ...source,
              keyMode: checked === true ? "standard" : "unknown",
            })
          }
        />
        {copy(
          "standardKey",
          "Use only the standard local key to verify the selected data",
        )}
      </label>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={
            locked ||
            !!action ||
            !source.installPath.startsWith("/") ||
            !source.dataRoot.startsWith("/") ||
            source.keyMode !== "standard"
          }
          onClick={() => void inspect()}
        >
          {copy("inspect", "Inspect selected source")}
        </Button>
        <Button
          type="button"
          disabled={!ordinaryReady || !!action}
          onClick={() => review("capture")}
        >
          {copy("capture", "Save current account")}
        </Button>
      </div>
      {context && (
        <p className="break-all text-sm text-muted-foreground">
          {copy(
            "inspected",
            "Inspected: {{root}} · {{family}} · {{version}} ({{build}})",
            {
              root: context.dataRoot,
              family: context.family,
              version: context.version,
              build: context.build,
            },
          )}
        </p>
      )}
      <p className="text-sm text-muted-foreground">
        {copy("currentUnknown", "Current account: unknown")}
      </p>
      {displayedError && (
        <p role="alert" className="text-sm text-destructive">
          {notice
            ? copy(
                "refreshFailed",
                "The local action completed, but refreshing its status failed. {{detail}}",
                { detail: errorText(displayedError) },
              )
            : errorText(displayedError)}
        </p>
      )}
      {notice && (
        <p role="status" className="text-sm">
          {notice}
        </p>
      )}
      {(pending || unconfirmed) && (
        <p className="text-sm text-amber-600">
          {copy(
            "recoveryRequired",
            "Account capture and switching are paused until pending or unconfirmed recovery is resolved. Preserve pending recovery, then confirm or explicitly recapture each affected record.",
          )}
        </p>
      )}
      {full && (
        <p className="text-sm text-amber-600">
          {copy(
            "recoveryFull",
            "Recovery storage is full. Confirm an older record, or sign in again through official ZCode, quit normally and explicitly recapture that old record. Then review and permanently delete that individually selected record to free space for switching. Preserving pending recovery can still be attempted.",
          )}
        </p>
      )}
      {context &&
        catalog.data &&
        (["zai", "bigmodel"] as const).map((family) => (
          <section
            key={family}
            aria-label={
              family === "zai"
                ? copy("zaiFamily", "Z.ai accounts")
                : copy("bigmodelFamily", "BigModel accounts")
            }
            className="space-y-2"
          >
            <h4 className="text-sm font-medium">
              {family === "zai" ? "Z.ai" : "BigModel"}
            </h4>
            {catalog.data.profiles
              .filter((profile) => profile.family === family)
              .map((profile) => (
                <Card key={profile.id}>
                  <CardContent className="flex flex-wrap items-center justify-between gap-3 p-3">
                    <div className="min-w-0">
                      <p className="break-all text-sm">
                        {profile.label ?? profile.id}
                      </p>
                      <p className="break-all text-xs text-muted-foreground">
                        {profile.id}
                      </p>
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={
                        !ordinaryReady ||
                        full ||
                        family !== context.family ||
                        !!action
                      }
                      onClick={() => review("switch", profile.id)}
                    >
                      {copy("switch", "Switch saved account")}
                    </Button>
                  </CardContent>
                </Card>
              ))}
            {catalog.data.profiles.every(
              (profile) => profile.family !== family,
            ) && (
              <p className="text-sm text-muted-foreground">
                {copy("empty", "No saved accounts in this family.")}
              </p>
            )}
          </section>
        ))}
      <section
        className="space-y-3"
        aria-label={copy("recoveryTitle", "Local account recovery")}
      >
        <h4 className="text-sm font-medium">
          {copy("recoveryTitle", "Local account recovery")}
        </h4>
        <p className="text-xs text-muted-foreground">
          {copy(
            "localHelp",
            "Local recovery can be inspected, preserved and cleaned up without a verified ZCode installation. Unconfirmed records require explicit verification before deletion.",
          )}
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!localReady || !recovery.data?.pending || !!action}
          onClick={() => review("archive")}
        >
          {copy("archive", "Archive pending recovery")}
        </Button>
        {recovery.data?.records.map((record) => (
          <Card key={record.id}>
            <CardContent
              role="group"
              aria-label={copy("recordLabel", "Recovery record {{id}}", {
                id: record.id,
              })}
              className="space-y-2 p-3"
            >
              <p className="break-all text-sm font-medium">{record.id}</p>
              <p className="text-xs text-muted-foreground">
                {t(`zcode.accounts.dispositions.${record.disposition}`, {
                  defaultValue: record.disposition,
                })}
                {record.latestCompleted &&
                  ` · ${copy("latest", "Latest completed transaction")}`}
              </p>
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!reviewedSourceReady || !!action}
                  onClick={() => review("confirm", record.id)}
                >
                  {copy("confirm", "Confirm this recovery record")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!catalogReady || !!action}
                  onClick={() => review("recapture", record.id)}
                >
                  {copy("recapture", "Recapture after official sign-in")}
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  size="sm"
                  disabled={
                    !localReady ||
                    record.disposition === "native-unconfirmed" ||
                    !!action
                  }
                  onClick={() => review("delete", record.id)}
                >
                  {copy("delete", "Permanently delete recovery record")}
                </Button>
              </div>
            </CardContent>
          </Card>
        ))}
        {recovery.data?.records.length === 0 && (
          <p className="text-sm text-muted-foreground">
            {copy("noRecovery", "No retained recovery records.")}
          </p>
        )}
      </section>
      <ConfirmDialog
        isOpen={action !== null}
        pending={busy}
        title={confirmation.title}
        message={confirmation.message}
        confirmText={confirmation.confirm}
        cancelText={copy("cancel", "Cancel account action")}
        variant={action?.kind === "delete" ? "destructive" : "info"}
        onCancel={() => {
          if (!inFlight.current) setAction(null);
        }}
        onConfirm={() => void perform()}
      />
    </section>
  );
}
