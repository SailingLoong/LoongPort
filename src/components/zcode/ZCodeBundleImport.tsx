import { useZCodeBundleImport } from "./viewmodels/useZCodeBundleImport";

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

type ImportProps = {
  open: boolean;
  onClose: () => void;
  onImported: () => Promise<void>;
  libraryDataRoot?: string;
  catalogRevision: string;
};

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
  const {
    chooseBundle,
    selectRow,
    updateRow,
    t,
    copy,
    dismissed,
    file,
    password,
    setPassword,
    review,
    reviewRows,
    consentDisabled,
    consent,
    setConsent,
    checkRequested,
    checkProgress,
    checkMismatch,
    busy,
    error,
    refreshFailed,
    originalRequest,
    recoveryUnknown,
    notRecorded,
    resultRows,
    resultSummary,
    submitted,
    fileInput,
    close,
    canCheck,
    canSave,
    preview,
    check,
    queryCheck,
    commit,
    queryOriginal,
    errorText,
  } = useZCodeBundleImport({
    onClose,
    onImported,
    libraryDataRoot,
    catalogRevision,
  });

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
                  disabled={!!busy || !!review || !!originalRequest}
                  onChange={(event) => {
                    chooseBundle(event.target.files?.[0] ?? null);
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
                  disabled={!!busy || !!review || !!originalRequest}
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
                    {reviewRows.map(
                      ({
                        row,
                        checked,
                        selected,
                        updateDuplicate,
                        selectionDisabled,
                        updateDisabled,
                      }) => {
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
                                checked={selected}
                                disabled={selectionDisabled}
                                onCheckedChange={(value) => {
                                  selectRow(row, value === true);
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
                                  checked={updateDuplicate}
                                  disabled={updateDisabled}
                                  onCheckedChange={(value) => {
                                    updateRow(row.index, value === true);
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
                      },
                    )}
                  </div>
                  <label className="flex items-start gap-2 rounded-md border border-border-default bg-muted/30 p-3 text-sm">
                    <Checkbox
                      checked={consent}
                      disabled={consentDisabled}
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
          {originalRequest && (!review || submitted) && (
            <Button
              variant="outline"
              disabled={!!busy}
              onClick={() => void queryOriginal()}
            >
              {copy("bundleQueryOriginal", "Query original result")}
            </Button>
          )}
          {notRecorded && (
            <p role="status" className="text-sm">
              {copy(
                "bundleNotRecorded",
                "This authenticated library has no saved result for the request. The old preview cannot be reused. Select a bundle for a new review and confirmation.",
              )}
            </p>
          )}
          {recoveryUnknown && (
            <p role="status" className="text-sm">
              {copy(
                "bundleReceiptUnknown",
                "No original receipt is available. The import result is still unknown; this does not mean it failed. Do not repeat the import to recover its result.",
              )}
            </p>
          )}
          {resultRows.length > 0 && (
            <>
              {resultSummary && (
                <p role="status" className="text-sm">
                  {copy(
                    "bundleImportedV2",
                    "Import completed: {{saved}} saved, {{updated}} updated, {{kept}} kept. Connection evidence remains separate from the native current account.",
                    resultSummary,
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
                    {row.label ||
                      row.id ||
                      copy("bundleMaskedAccount", "Masked account")}{" "}
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
                "Closing this dialog does not undo a submitted import. Use Query original result after a lost response, including after restarting LoongPort.",
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
              disabled={
                !!busy || !!originalRequest || !file || password.length === 0
              }
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
