import { useZCodeOAuthAdd } from "./viewmodels/useZCodeOAuthAdd";

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
import { accountErrorText } from "@/lib/api/zcodeAccounts";
import { type LoginFamily } from "@/lib/api/zcodeLogin";

export function ZCodeOAuthAdd({
  open,
  onClose,
  onSaved,
  onImportLocal,
  libraryDataRoot,
  savedAccount,
  boundDeviceMids = [],
}: {
  open: boolean;
  onClose: () => void;
  onSaved: () => Promise<void>;
  onImportLocal?: () => void;
  libraryDataRoot?: string;
  savedAccount?: { id: string; catalogRevision: string };
  boundDeviceMids?: string[];
}) {
  const {
    reloadDevices,
    importLocal,
    queryOriginal,
    restart,
    t,
    copy,
    family,
    setFamily,
    devices,
    deviceMid,
    setDeviceMid,
    deviceLoading,
    deviceLoadFailed,
    progress,
    reading,
    error,
    uncertain,
    updateDuplicate,
    setUpdateDuplicate,
    listRefreshFailed,
    previousReadFailed,
    restore,
    openExternal,
    begin,
    write,
    close,
    startAnother,
    cancelRead,
    phase,
    isBoundDevice,
    deviceTaken,
    currentDeviceTaken,
    completeCoding,
    blocked,
    keyUncertain,
    saveUncertain,
    title,
    capability,
    failureKey,
  } = useZCodeOAuthAdd({
    open,
    onClose,
    onSaved,
    onImportLocal,
    libraryDataRoot,
    savedAccount,
    boundDeviceMids,
  });

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
          {!progress && reading && (
            <p role="status" className="text-sm">
              {savedAccount
                ? copy(
                    "preparingSaved",
                    "Preparing the saved account connection",
                  )
                : copy(
                    "checkingPrevious",
                    "Checking the previous sign-in result…",
                  )}
            </p>
          )}
          {!progress && !savedAccount && (
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
              {devices.length > 0 && (
                <Select
                  value={deviceMid}
                  onValueChange={setDeviceMid}
                  disabled={blocked}
                >
                  <SelectTrigger
                    aria-label={copy("device", "Account device ID")}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="current" disabled={currentDeviceTaken}>
                      {copy("deviceCurrent", "Use current environment")}
                    </SelectItem>
                    {devices.map((id) => (
                      <SelectItem
                        key={id}
                        value={id}
                        disabled={isBoundDevice(id)}
                      >
                        {id}
                        {isBoundDevice(id)
                          ? ` · ${copy("deviceBound", "Bound")}`
                          : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              {devices.length > 0 && (
                <p className="text-xs text-muted-foreground">
                  {copy(
                    "deviceHint",
                    "Choose an unassigned device ID when adding or reauthorizing an account.",
                  )}
                </p>
              )}
              {devices.length === 0 && currentDeviceTaken && (
                <p role="status" className="text-xs text-muted-foreground">
                  {copy(
                    "deviceTakenNoAvailable",
                    "The current device ID is already bound. Import another device ID before continuing.",
                  )}
                </p>
              )}
              {deviceLoadFailed && (
                <div role="alert">
                  <p>
                    {copy("deviceLoadFailed", "Unable to load device IDs.")}
                  </p>
                  <Button variant="outline" onClick={() => reloadDevices()}>
                    {copy("retryDevices", "Retry")}
                  </Button>
                </div>
              )}
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
                      {progress.account.identitySource === "officialLogin"
                        ? copy("officialIdentity", "Verified official sign-in")
                        : progress.account.identitySource === "nativeCapture"
                          ? copy("nativeIdentity", "Captured native session")
                          : copy(
                              "packageIdentity",
                              "Declared by account bundle",
                            )}
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
                      {completeCoding
                        ? copy(
                            "saveCodingScope",
                            "Save the verified Coding connection to this selected account in the encrypted vault. Its stable identity and the current ZCode sign-in remain unchanged.",
                          )
                        : copy(
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
              {phase === "review" &&
                progress.account?.duplicate &&
                !completeCoding && (
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
                  {completeCoding
                    ? copy(
                        "savedCodingNext",
                        "The selected account's connection was saved. Return to the account list to review its capabilities.",
                      )
                    : copy(
                        "savedNext",
                        "You can add another account. To switch later, choose the account in the list and review the client impact.",
                      )}
                </p>
              )}
            </>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {previousReadFailed
                ? copy(
                    "previousReadFailed",
                    "The previous sign-in result could not be read. Query it again to recover the original operation.",
                  )
                : failureKey === "operation_failed"
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
          {onImportLocal &&
            !savedAccount &&
            !completeCoding &&
            !previousReadFailed &&
            !uncertain &&
            !progress?.keyCreated &&
            !progress?.keyMayExist &&
            (phase === "failed" || (!progress && !!error)) && (
              <Button
                variant="outline"
                disabled={blocked}
                onClick={() => {
                  importLocal();
                }}
              >
                {copy("importLocal", "Import from local ZCode instead")}
              </Button>
            )}
          {uncertain && (
            <p className="text-sm">
              {copy(
                "uncertain",
                "The response was not confirmed. Query the original result before another action; do not repeat the write.",
              )}
            </p>
          )}
          {!progress && (
            <Button
              variant="outline"
              size="sm"
              disabled={blocked}
              onClick={() => void restore()}
            >
              {savedAccount
                ? copy("prepareSaved", "Prepare saved Coding connection")
                : copy("queryPrevious", "Query previous sign-in result")}
            </Button>
          )}
          {!progress && reading && (
            <Button variant="outline" size="sm" onClick={cancelRead}>
              {copy("cancelRead", "Cancel reading")}
            </Button>
          )}
          {progress && (
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={blocked}
                onClick={() => {
                  void queryOriginal();
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
          {!progress && !savedAccount && (
            <Button
              disabled={
                blocked || deviceLoading || deviceLoadFailed || deviceTaken
              }
              onClick={() => void begin()}
            >
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
              {completeCoding
                ? copy("saveCoding", "Save verified Coding connection")
                : copy("save", "Save to encrypted account vault")}
            </Button>
          )}
          {phase === "saved" && !completeCoding && (
            <Button disabled={blocked} onClick={startAnother}>
              {copy("addAnother", "Add another account")}
            </Button>
          )}
          {(phase === "failed" ||
            phase === "expired" ||
            phase === "cancelled") && (
            <Button
              disabled={blocked || !!progress?.keyMayExist}
              onClick={() => {
                restart();
              }}
            >
              {savedAccount
                ? copy("prepareSaved", "Prepare saved Coding connection")
                : copy("restart", "Start a new sign-in")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
