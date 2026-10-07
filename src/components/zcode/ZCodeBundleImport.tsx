import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
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
import { ZCodeAccountEvidence } from "./ZCodeAccountEvidence";
import {
  accountErrorText,
  safeAccountError,
  zcodeAccountsApi,
  type BundlePreview,
  type BundleCheckProgress,
  type AccountError,
} from "@/lib/api/zcodeAccounts";

type ImportProps = {
  open: boolean;
  onClose: () => void;
  onImported: () => Promise<void>;
  libraryDataRoot?: string;
  catalogRevision: string;
};
type Row = BundlePreview["rows"][number];
type Choice = { index: number; updateDuplicate: boolean };
const choiceKey = (choices: Choice[]) =>
  JSON.stringify(
    [...choices]
      .sort((a, b) => a.index - b.index)
      .map(({ index, updateDuplicate }) => [index, updateDuplicate]),
  );
type Outcome = "saved" | "refreshed" | "kept" | "unknown";
function readFile(file: File): Promise<ArrayBuffer> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("read failed"));
    reader.onload = () =>
      reader.result instanceof ArrayBuffer
        ? resolve(reader.result)
        : reject(new Error("invalid file"));
    reader.readAsArrayBuffer(file);
  });
}
export function ZCodeBundleImport(props: ImportProps) {
  // Changing the library retires the old preview before any new interaction.
  return props.open ? (
    <ImportSession
      key={JSON.stringify(props.libraryDataRoot ?? null)}
      {...props}
    />
  ) : null;
}
function ImportSession({
  onClose,
  onImported,
  libraryDataRoot,
  catalogRevision,
}: ImportProps) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.${key}`, { defaultValue, ...values });
  const [dismissed, setDismissed] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [password, setPassword] = useState("");
  const [review, setReview] = useState<{
    preview: BundlePreview;
    revision: string;
  } | null>(null);
  const [selected, setSelected] = useState(new Set<number>());
  const [updates, setUpdates] = useState(new Set<number>());
  const [consent, setConsent] = useState(false);
  const [checkRequested, setCheckRequested] = useState(false);
  const [checkProgress, setCheckProgress] =
    useState<BundleCheckProgress | null>(null);
  const [checkedSelection, setCheckedSelection] = useState<string | null>(null);
  const [checkMismatch, setCheckMismatch] = useState(false);
  const [busy, setBusy] = useState<
    "preview" | "check" | "read" | "save" | null
  >(null);
  const [error, setError] = useState<AccountError | null>(null);
  const [refreshFailed, setRefreshFailed] = useState(false);
  const [resultRows, setResultRows] = useState<
    { row: Row; outcome: Outcome }[]
  >([]);
  const [submitted, setSubmitted] = useState(false);
  const alive = useRef(false);
  const generation = useRef(0);
  const checkEpoch = useRef(0);
  const inFlight = useRef(false);
  const lease = useRef<string | null>(null);
  const didSubmit = useRef(false);
  const seenRevision = useRef(catalogRevision);
  const fileInput = useRef<HTMLInputElement>(null);

  const retire = () => {
    generation.current += 1;
    checkEpoch.current += 1;
    inFlight.current = false;
    const previous = lease.current;
    lease.current = null;
    if (previous) void zcodeAccountsApi.cancelBundle(previous).catch(() => {});
  };
  const clearFile = () => {
    setFile(null);
    setPassword("");
    if (fileInput.current) fileInput.current.value = "";
  };
  const invalidateCheck = () => {
    checkEpoch.current += 1;
    setCheckProgress(null);
    setCheckRequested(false);
    setCheckedSelection(null);
    setConsent(false);
    setError(null);
    setCheckMismatch(false);
  };
  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      retire();
    };
  }, []);
  useLayoutEffect(() => {
    if (seenRevision.current === catalogRevision) return;
    seenRevision.current = catalogRevision;
    // A completed save remains a fact when the caller refreshes its catalog.
    if (didSubmit.current) return;
    retire();
    setReview(null);
    clearFile();
    setSelected(new Set());
    setUpdates(new Set());
    invalidateCheck();
    setBusy(null);
  }, [catalogRevision]);
  const current = (version: number) =>
    alive.current && generation.current === version;
  const close = () => {
    alive.current = false;
    retire();
    clearFile();
    setDismissed(true);
    onClose();
  };
  const fail = (cause: unknown) => setError(safeAccountError(cause));
  const choices: Choice[] =
    review?.preview.rows
      .filter((row) => selected.has(row.index) && !row.error)
      .map((row) => ({
        index: row.index,
        updateDuplicate: updates.has(row.index),
      })) ?? [];
  const selectionKey = choiceKey(choices);
  const checking =
    checkRequested && (!checkProgress || checkProgress.status === "checking");
  const selectionLocked = !!busy || checking || submitted;
  const canCheck =
    !!review &&
    choices.length > 0 &&
    consent &&
    !busy &&
    !checking &&
    !submitted;
  const canSave =
    !!review &&
    checkProgress?.status === "ready" &&
    checkedSelection === selectionKey &&
    choices.length > 0 &&
    !busy &&
    !submitted;

  const preview = async () => {
    if (
      !alive.current ||
      inFlight.current ||
      review ||
      !file ||
      password.length === 0
    )
      return;
    if (
      !/\.zsb$/i.test(file.name) ||
      file.size === 0 ||
      file.size > 10 * 1024 * 1024
    ) {
      fail({
        code: "zcode.account.bundle_invalid",
        remedy: "reviewSavedData",
        committed: false,
      });
      return;
    }
    inFlight.current = true;
    setBusy("preview");
    setError(null);
    const version = generation.current;
    const inputPassword = password;
    const revision = catalogRevision;
    setPassword("");
    let bytes: Uint8Array | null = null;
    try {
      bytes = new Uint8Array(await readFile(file));
      if (!current(version)) return;
      const next = await zcodeAccountsApi.previewBundle(
        libraryDataRoot,
        revision,
        Array.from(bytes),
        inputPassword,
      );
      if (!current(version)) {
        void zcodeAccountsApi.cancelBundle(next.previewId).catch(() => {});
        return;
      }
      lease.current = next.previewId;
      setReview({ preview: next, revision });
      setSelected(
        new Set(
          next.rows
            .filter((row) => !row.error && !row.ambiguous)
            .map((row) => row.index),
        ),
      );
      setUpdates(new Set());
      invalidateCheck();
    } catch (cause) {
      if (current(version)) fail(cause);
    } finally {
      bytes?.fill(0);
      if (current(version)) {
        inFlight.current = false;
        setBusy(null);
        clearFile();
      }
    }
  };
  const applyCheck = (
    result: BundleCheckProgress,
    version: number,
    epoch: number,
    previewId: string,
    expectedSelection: string,
  ) => {
    if (
      !current(version) ||
      checkEpoch.current !== epoch ||
      result.previewId !== previewId
    )
      return;
    if (choiceKey(result.selected ?? []) !== expectedSelection) {
      setCheckMismatch(true);
      setCheckProgress(null);
      setCheckRequested(false);
      setCheckedSelection(null);
      setConsent(false);
      setError(null);
      return;
    }
    setCheckProgress(result);
    setError(result.error ? safeAccountError(result.error) : null);
  };
  const check = async () => {
    if (!alive.current || !review || !canCheck || inFlight.current) return;
    const version = generation.current;
    const epoch = ++checkEpoch.current;
    const previewId = review.preview.previewId;
    inFlight.current = true;
    setBusy("check");
    setError(null);
    setCheckRequested(true);
    setCheckProgress(null);
    setCheckMismatch(false);
    setCheckedSelection(selectionKey);
    try {
      applyCheck(
        await zcodeAccountsApi.checkBundle(previewId, choices, true),
        version,
        epoch,
        previewId,
        selectionKey,
      );
    } catch (cause) {
      if (current(version) && checkEpoch.current === epoch) fail(cause);
    } finally {
      if (current(version) && checkEpoch.current === epoch) {
        inFlight.current = false;
        setBusy(null);
      }
    }
  };
  const queryCheck = async () => {
    if (
      !alive.current ||
      !review ||
      !checkRequested ||
      inFlight.current ||
      submitted
    )
      return;
    const version = generation.current;
    const epoch = checkEpoch.current;
    const previewId = review.preview.previewId;
    inFlight.current = true;
    setBusy("read");
    setError(null);
    try {
      applyCheck(
        await zcodeAccountsApi.bundleCheck(previewId),
        version,
        epoch,
        previewId,
        checkedSelection ?? "",
      );
    } catch (cause) {
      if (current(version) && checkEpoch.current === epoch) fail(cause);
    } finally {
      if (current(version) && checkEpoch.current === epoch) {
        inFlight.current = false;
        setBusy(null);
      }
    }
  };
  useEffect(() => {
    if (
      !alive.current ||
      busy ||
      error ||
      submitted ||
      checkProgress?.status !== "checking"
    )
      return;
    const timer = setTimeout(() => void queryCheck(), 1000);
    return () => clearTimeout(timer);
  }, [checkProgress, busy, error, submitted]);
  const commit = async () => {
    if (
      !alive.current ||
      !review ||
      !canSave ||
      inFlight.current ||
      didSubmit.current
    )
      return;
    const version = generation.current;
    const rows = review.preview.rows.filter((row) =>
      choices.some((choice) => choice.index === row.index),
    );
    inFlight.current = true;
    didSubmit.current = true;
    setSubmitted(true);
    setBusy("save");
    setError(null);
    try {
      const outcomes = await zcodeAccountsApi.importBundle(
        libraryDataRoot,
        review.revision,
        review.preview.previewId,
        choices,
      );
      if (
        !Array.isArray(outcomes) ||
        outcomes.length !== rows.length ||
        !outcomes.every((outcome) =>
          ["saved", "refreshed", "kept"].includes(outcome),
        )
      )
        throw {
          code: "zcode.account.saved_data_invalid",
          remedy: "reviewSavedData",
          committed: false,
        };
      if (!current(version)) return;
      setResultRows(
        rows.map((row, index) => ({ row, outcome: outcomes[index] })),
      );
      try {
        await onImported();
      } catch {
        if (current(version)) setRefreshFailed(true);
      }
    } catch (cause) {
      if (current(version)) {
        setResultRows(rows.map((row) => ({ row, outcome: "unknown" })));
        fail(cause);
      }
    } finally {
      if (current(version)) {
        inFlight.current = false;
        setBusy(null);
      }
    }
  };
  const errorText = (failure: AccountError) => {
    const key = safeAccountError(failure).code.slice("zcode.account.".length);
    return t(`zcode.accounts.errors.${key}`, {
      defaultValue:
        accountErrorText[key as keyof typeof accountErrorText] ??
        accountErrorText.operation_failed,
    });
  };
  if (dismissed) return null;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogContent
        className="max-w-2xl"
        closeButtonLabel={copy("bundleClose", "Close bundle import")}
      >
        <DialogHeader>
          <DialogTitle>
            {copy("bundleReviewTitle", "Review bundle accounts")}
          </DialogTitle>
          <DialogDescription>
            {copy(
              "bundleIntroV2",
              "Decrypt a .zsb locally, select complete account sessions, authorize official checks, then review and save. Importing does not switch ZCode.",
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto px-6 py-5">
          {!submitted && (
            <>
              <div className="space-y-2">
                <Label htmlFor="zcode-bundle-file">
                  {copy("bundleFile", "Account bundle (.zsb)")}
                </Label>
                <Input
                  ref={fileInput}
                  id="zcode-bundle-file"
                  type="file"
                  accept=".zsb"
                  disabled={!!busy || !!review}
                  onChange={(event) => {
                    setFile(event.target.files?.[0] ?? null);
                    setError(null);
                  }}
                />
                <Label htmlFor="zcode-bundle-password">
                  {copy("bundlePassword", "Bundle password")}
                </Label>
                <Input
                  id="zcode-bundle-password"
                  type="password"
                  value={password}
                  autoComplete="off"
                  spellCheck={false}
                  disabled={!!busy || !!review}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </div>
              <p className="text-xs text-muted-foreground">
                {copy(
                  "bundleEnvironmentV2",
                  "Supports .zsb outer v1 / inner v2, up to 10 MiB and 50 accounts. Credentials encrypted for another operating system, user or home may require the original environment.",
                )}
              </p>
              {review && (
                <>
                  <p className="text-sm">
                    {copy(
                      "bundleReviewV2",
                      "Existing accounts are kept by default. Select one complete record for each identity; file order does not establish freshness. An incomplete replacement will not erase a working saved record.",
                    )}
                  </p>
                  <div className="space-y-3">
                    {review.preview.rows.map((row) => {
                      const checked = checkProgress?.rows.find(
                        (entry) => entry.index === row.index,
                      );
                      return (
                        <div
                          key={row.index}
                          className="space-y-2 rounded border border-border-default p-3"
                        >
                          <label className="flex items-start gap-2">
                            <Checkbox
                              aria-label={copy(
                                "bundleSelect",
                                "Select account {{number}}",
                                { number: row.index + 1 },
                              )}
                              checked={selected.has(row.index)}
                              disabled={selectionLocked || !!row.error}
                              onCheckedChange={(value) => {
                                setSelected((previous) => {
                                  const next = new Set(previous);
                                  if (value === true) {
                                    for (const peer of review.preview.rows)
                                      if (peer.id === row.id)
                                        next.delete(peer.index);
                                    next.add(row.index);
                                  } else next.delete(row.index);
                                  return next;
                                });
                                invalidateCheck();
                              }}
                            />
                            <span className="break-all text-sm">
                              {row.index + 1}.{" "}
                              {row.label ??
                                row.id ??
                                copy(
                                  "bundleInvalidEntry",
                                  "Unusable account entry",
                                )}{" "}
                              · {row.family}
                            </span>
                          </label>
                          {row.error && (
                            <p className="text-sm">
                              {row.error === "incompatibleCredentials"
                                ? copy(
                                    "bundleIncompatibleV2",
                                    "These credentials cannot be decrypted in this environment. Restore them in the original environment; this is not an account-expiration result.",
                                  )
                                : copy(
                                    "bundleInvalidEntry",
                                    "Unusable account entry",
                                  )}
                            </p>
                          )}
                          {row.ambiguous && (
                            <p className="text-sm">
                              {copy(
                                "bundleAmbiguous",
                                "Same identity appears more than once. Explicitly choose one record; none is selected by default.",
                              )}
                            </p>
                          )}
                          {row.duplicate && (
                            <label className="flex items-start gap-2">
                              <Checkbox
                                aria-label={copy(
                                  "bundleUpdate",
                                  "Update existing account {{number}}",
                                  { number: row.index + 1 },
                                )}
                                checked={updates.has(row.index)}
                                disabled={
                                  selectionLocked || !selected.has(row.index)
                                }
                                onCheckedChange={(value) => {
                                  setUpdates((previous) => {
                                    const next = new Set(previous);
                                    if (value === true) next.add(row.index);
                                    else next.delete(row.index);
                                    return next;
                                  });
                                  invalidateCheck();
                                }}
                              />
                              <span className="text-sm">
                                {copy(
                                  "updateDuplicate",
                                  "Explicitly update this existing saved account",
                                )}
                              </span>
                            </label>
                          )}
                          {checked?.capabilities && (
                            <ZCodeAccountEvidence
                              value={checked.capabilities}
                            />
                          )}
                          {checked?.error && (
                            <p className="text-sm text-destructive">
                              {errorText(checked.error)}
                            </p>
                          )}
                          {checked && !checked.capabilities && (
                            <p className="text-xs text-muted-foreground">
                              {copy(
                                "bundleChecksUnknown",
                                "No verified capability details are available for this account. Unknown does not mean expired.",
                              )}
                            </p>
                          )}
                        </div>
                      );
                    })}
                  </div>
                  <label className="flex items-start gap-2 rounded-md border border-border-default bg-muted/30 p-3 text-sm">
                    <Checkbox
                      checked={consent}
                      disabled={selectionLocked || choices.length === 0}
                      onCheckedChange={(value) => setConsent(value === true)}
                      aria-label={copy(
                        "bundleCheckConsentLabel",
                        "Allow official checks for the selected accounts",
                      )}
                    />
                    <span>
                      {copy(
                        "bundleCheckConsent",
                        "Send each selected account's session credentials only to its corresponding official platform to verify connections and quota. This does not call models, create Keys or switch the native account.",
                      )}
                    </span>
                  </label>
                  {checkProgress && (
                    <p role="status" className="text-sm">
                      {checkProgress.status === "ready"
                        ? copy(
                            "bundleChecksReady",
                            "Official checks finished. Review each capability before saving.",
                          )
                        : copy(
                            "bundleCheckProgress",
                            "Official checks: {{completed}} / {{total}}",
                            {
                              completed: checkProgress.completed,
                              total: checkProgress.total,
                            },
                          )}
                    </p>
                  )}
                  {checkRequested && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!!busy}
                      onClick={() => void queryCheck()}
                    >
                      {copy("bundleQueryChecks", "Query verification result")}
                    </Button>
                  )}
                </>
              )}
              {checkMismatch && (
                <p role="status" className="text-sm">
                  {copy(
                    "bundleCheckSelectionChanged",
                    "This result belongs to an earlier selection. Authorize checks for the current selection again.",
                  )}
                </p>
              )}
            </>
          )}
          {submitted && busy === "save" && (
            <p role="status" className="text-sm">
              {copy("bundleSaving", "Saving the reviewed account sessions…")}
            </p>
          )}
          {resultRows.length > 0 && (
            <>
              {resultRows.every((row) => row.outcome !== "unknown") && (
                <p role="status" className="text-sm">
                  {copy(
                    "bundleImportedV2",
                    "Import completed: {{saved}} saved, {{updated}} updated, {{kept}} kept. Connection evidence remains separate from the native current account.",
                    {
                      saved: resultRows.filter((row) => row.outcome === "saved")
                        .length,
                      updated: resultRows.filter(
                        (row) => row.outcome === "refreshed",
                      ).length,
                      kept: resultRows.filter((row) => row.outcome === "kept")
                        .length,
                    },
                  )}
                </p>
              )}
              <ul
                aria-label={copy(
                  "bundleResults",
                  "Last import results by account",
                )}
                className="space-y-1 text-sm"
              >
                {resultRows.map(({ row, outcome }) => (
                  <li key={row.index}>
                    {row.index + 1}.{" "}
                    {row.label || copy("bundleMaskedAccount", "Masked account")}{" "}
                    · {row.family} ·{" "}
                    {outcome === "saved"
                      ? copy("bundleResultSaved", "Saved")
                      : outcome === "refreshed"
                        ? copy("bundleResultUpdated", "Updated")
                        : outcome === "kept"
                          ? copy("bundleResultKept", "Kept existing")
                          : copy(
                              "bundleResultUnknown",
                              "Unconfirmed — inspect saved accounts",
                            )}
                  </li>
                ))}
              </ul>
            </>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {errorText(error)}
            </p>
          )}
          {refreshFailed && (
            <p role="alert" className="text-sm text-destructive">
              {copy(
                "bundleListRefreshFailedV2",
                "Import completed, but the saved account list could not be refreshed. Refresh the account list; do not repeat the import.",
              )}
            </p>
          )}
          {submitted && (
            <p className="text-xs text-muted-foreground">
              {copy(
                "bundleSubmittedHint",
                "Closing this dialog does not undo a submitted import. If its response is unknown, inspect the saved accounts before another import.",
              )}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={close}>
            {submitted
              ? copy("bundleDone", "Close")
              : t("common.cancel", { defaultValue: "Cancel" })}
          </Button>
          {!review && !submitted && (
            <Button
              disabled={!!busy || !file || password.length === 0}
              onClick={() => void preview()}
            >
              {copy("bundlePreview", "Preview account bundle")}
            </Button>
          )}
          {review && !submitted && (
            <>
              <Button
                variant="outline"
                disabled={!canCheck}
                onClick={() => void check()}
              >
                {copy("bundleCheck", "Verify selected accounts")}
              </Button>
              <Button disabled={!canSave} onClick={() => void commit()}>
                {copy("bundleImport", "Import selected into encrypted vault")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
