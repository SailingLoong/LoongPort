import { useZCodeAccountPanel } from "./viewmodels/useZCodeAccountPanel";
import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ZCodeBundleImport } from "./ZCodeBundleImport";
import { ZCodeClaimControls } from "./ZCodeClaimControls";
import { ZCodeDeviceManager } from "./ZCodeDeviceManager";
import { ZCodeAccountEvidence } from "./ZCodeAccountEvidence";
import { ZCodeOAuthAdd } from "./ZCodeOAuthAdd";
import { ZCodeNativeSwitchSection } from "./ZCodeNativeSwitchSection";
import { ZCodeBackupDialog } from "./ZCodeBackupDialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { safeAccountError } from "@/lib/api/zcodeAccounts";

/** Selection is ephemeral; native admission and account identity stay in Rust. */
export function ZCodeAccountPanel({
  disabled = false,
  unified = false,
  onBusyChange,
}: {
  disabled?: boolean;
  unified?: boolean;
  onBusyChange?: (busy: boolean) => void;
}) {
  const {
    recheckMetadata,
    openOfficialGuide,
    addAccount,
    completeSavedCoding,
    t,
    copy,
    hasSourcePreference,
    source,
    context,
    busy,
    loginOpen,
    closeLogin,
    localImportOpen,
    setLocalImportOpen,
    localImportRef,
    codingAccount,

    backupOpen,
    setBackupOpen,
    importOpen,
    setImportOpen,
    labelEditor,
    editLabel,
    changeLabel,
    closeLabel,
    currentIdentity,
    identityReading,
    originalOperation,
    operationQueryState,
    operationPending,
    originalUnresolved,
    recoveryRef,
    checkingId,
    connectionViews,
    action,
    error,
    notice,
    latestVersion,
    needsReview,
    discovery,
    libraryRoot,
    library,
    recovery,
    locked,
    loading,
    shownCatalog,
    libraryReady,
    localReady,
    reviewedSourceReady,
    catalogReady,
    pending,
    unconfirmed,
    full,
    ordinaryReady,
    displayedError,
    errorText,
    retireConnectionCheck,
    checkConnections,
    queryLatest,
    selectSource,
    readCurrentIdentity,
    refreshLibrary,
    saveLabel,
    queryOriginalExplicitly,
    inspect,
    openForLogin,
    refresh,
    review,
    cancel,
    confirmReviewedAction,
    accountGroups,
    boundDeviceMids,
    confirmation,
  } = useZCodeAccountPanel({ disabled, unified, onBusyChange });
  const nativeTools = (
    <>
      <section
        aria-label={copy("installationStatus", "Installation and versions")}
        className="space-y-2 rounded-md border p-3"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 className="font-medium">
            {copy("installationStatus", "Installation and versions")}
          </h4>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={locked || !!action || discovery.isFetching}
            onClick={() => {
              recheckMetadata();
            }}
          >
            {copy("recheckMetadata", "Recheck installation")}
          </Button>
        </div>
        {discovery.isFetching && (
          <p>{copy("discovering", "Checking public installation metadata…")}</p>
        )}
        {discovery.isError && (
          <p role="status">{errorText(safeAccountError(discovery.error))}</p>
        )}
        {discovery.data?.candidates.length === 0 && (
          <p>
            {copy(
              "notDetected",
              "No installation detected. Use the official installation guide or select a source manually.",
            )}
          </p>
        )}
        {discovery.data?.candidates.map((candidate) => (
          <div key={candidate.installPath} className="space-y-1 text-sm">
            <p className="break-all">{candidate.installPath}</p>
            <p>
              {copy("currentBuild", "Current: {{version}} · {{build}}", {
                version: candidate.version ?? copy("unknownVersion", "Unknown"),
                build: candidate.build ?? copy("unknownVersion", "Unknown"),
              })}
            </p>
            <p>
              {candidate.verifiedBuild
                ? copy(
                    "verifiedBuild",
                    "Exact installed build verified. Session actions still require a fresh source and account check.",
                  )
                : copy(
                    "unverifiedNativeBuild",
                    "Installed build is unverified; native capture and switching remain blocked.",
                  )}
            </p>
            {discovery.data!.candidates.length > 1 && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={locked || !!action}
                onClick={() =>
                  selectSource({
                    installPath: candidate.installPath,
                    dataRoot: discovery.data!.dataRoot,
                    keyMode: "unknown",
                  })
                }
              >
                {copy("chooseInstallation", "Choose this installation")}
              </Button>
            )}
          </div>
        ))}
        <p className="text-xs text-muted-foreground">
          {!latestVersion
            ? copy("latestNotQueried", "Official latest version: not queried")
            : latestVersion.error
              ? copy(
                  "latestQueryFailed",
                  "Official version query failed at {{time}}. No current result is available.",
                  {
                    time: new Date(
                      latestVersion.checkedAt * 1000,
                    ).toISOString(),
                  },
                )
              : copy(
                  "latestQueried",
                  "Official latest version: {{version}} · checked {{time}}",
                  {
                    version:
                      latestVersion.version ??
                      copy("unknownVersion", "Unknown"),
                    time: new Date(
                      latestVersion.checkedAt * 1000,
                    ).toISOString(),
                  },
                )}
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={locked || !!action}
          onClick={() => void queryLatest()}
        >
          {copy("queryLatest", "Query official latest version")}
        </Button>
        <p className="text-xs text-muted-foreground">
          {copy(
            "supportedBuilds",
            "Verified operation range: macOS 3.14.4 (build 3.14.4.7912) and 3.16.2 arm64 (build 3.16.2.8769), exact accepted artifacts only.",
          )}
        </p>
        <Button
          type="button"
          variant="link"
          size="sm"
          onClick={() => {
            void openOfficialGuide();
          }}
        >
          {copy("officialGuide", "Official installation / update guide")}
        </Button>
      </section>
      <p className="text-sm">
        {discovery.data?.sourceBasis === "bootstrapDataBaseDir"
          ? copy(
              "bootstrapSource",
              "Data source discovered from the official desktop bootstrap data base directory.",
            )
          : copy(
              "homeSource",
              "Default data source uses the OS account home. Advanced source selection remains available below.",
            )}
      </p>
      {hasSourcePreference && !context && (
        <p className="text-xs text-muted-foreground">
          {copy(
            "preferenceUnverified",
            "Saved paths restored as a preference. Installation, data source and key context must be checked again.",
          )}
        </p>
      )}
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
          "nativeSourceHelp",
          "Use the exact .zcode/v2 directory inside the data base directory configured in official ZCode. Standard local verification applies to native capture and switching. Custom key contexts and team accounts are unsupported; managing the encrypted account library does not require closing ZCode.",
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
          variant="outline"
          disabled={
            locked ||
            !!action ||
            !source.installPath.startsWith("/") ||
            !source.dataRoot.startsWith("/") ||
            source.keyMode !== "standard"
          }
          onClick={() => void openForLogin()}
        >
          {copy("openForLogin", "Open official ZCode for login")}
        </Button>
      </div>
    </>
  );
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
              "libraryDescription",
              "Manage personal BigModel and z.ai accounts in the encrypted local vault. Native capture and switching have separate source requirements.",
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
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={
            !libraryReady || library.data?.actions.canAdd !== true || !!action
          }
          onClick={() => {
            addAccount();
          }}
        >
          {copy("addAccount", "Add account")}
        </Button>
        <Button
          variant="outline"
          disabled={locked || !!action || discovery.isFetching}
          onClick={() => setLocalImportOpen(true)}
        >
          {copy("localImport.open", "Import from local ZCode")}
        </Button>
        <Button
          variant="outline"
          disabled={!ordinaryReady || !!action}
          onClick={() => review("capture")}
        >
          {copy("capture", "Save current account")}
        </Button>
        <Button
          variant="outline"
          disabled={
            !libraryReady ||
            library.data?.actions.canImport !== true ||
            !!action
          }
          onClick={() => setImportOpen(true)}
        >
          {copy("importAccounts", "Import .zsb")}
        </Button>
        <Button
          variant="outline"
          disabled={
            !libraryReady ||
            library.data?.actions.canBackup !== true ||
            !!action
          }
          onClick={() => setBackupOpen(true)}
        >
          {copy("backupAccounts", "Encrypted backup")}
        </Button>
      </div>
      {localImportOpen && (
        <section
          ref={localImportRef}
          tabIndex={-1}
          aria-label={copy(
            "localImport.title",
            "Import the signed-in ZCode account",
          )}
          className="space-y-3 rounded-md border border-border-default p-4"
        >
          <div className="flex items-center justify-between gap-3">
            <h4 className="font-medium">
              {copy("localImport.title", "Import the signed-in ZCode account")}
            </h4>
            <Button
              variant="ghost"
              size="sm"
              disabled={locked || !!action}
              onClick={() => setLocalImportOpen(false)}
            >
              {copy("localImport.close", "Close local import")}
            </Button>
          </div>
          <p className="text-sm text-muted-foreground">
            {copy(
              "localImport.intro",
              "Reuse the account already signed in to official ZCode on this computer. No folder is moved, and the current ZCode sign-in stays unchanged.",
            )}
          </p>
          <ol className="list-decimal space-y-1 pl-5 text-sm">
            <li>
              {copy(
                "localImport.stepLogin",
                "Sign in in official ZCode, then quit it normally.",
              )}
            </li>
            <li>
              {copy(
                "localImport.stepInspect",
                "Review the detected installation and data directory below, select the standard local key, then inspect the source.",
              )}
            </li>
            <li>
              {copy(
                "localImport.stepPreview",
                "Preview the local account, then confirm saving it separately. Unsupported or changed sources remain blocked.",
              )}
            </li>
          </ol>
          {nativeTools}
          <Button
            disabled={!ordinaryReady || !!action}
            onClick={() => review("capture")}
          >
            {copy("localImport.preview", "Preview local account")}
          </Button>
        </section>
      )}
      {library.data?.actions.blockedReason && (
        <p role="status" className="text-sm text-muted-foreground">
          {errorText(library.data.actions.blockedReason)}
        </p>
      )}
      {!context && (
        <p className="text-xs text-muted-foreground">
          {copy(
            "nativeAdmissionRequired",
            "Inspect a supported native source before saving its current login or switching accounts. Account library management remains separate.",
          )}
        </p>
      )}
      {!unified && !localImportOpen && nativeTools}
      {library.data && (
        <ZCodeBundleImport
          open={importOpen}
          onClose={() => setImportOpen(false)}
          libraryDataRoot={libraryRoot}
          catalogRevision={library.data.revision}
          onImported={refreshLibrary}
        />
      )}
      <ZCodeOAuthAdd
        open={loginOpen}
        onClose={() => {
          closeLogin();
        }}
        onSaved={refreshLibrary}
        onImportLocal={() => setLocalImportOpen(true)}
        libraryDataRoot={libraryRoot}
        savedAccount={codingAccount}
        boundDeviceMids={boundDeviceMids}
      />
      {library.data && (
        <ZCodeBackupDialog
          open={backupOpen}
          onClose={() => setBackupOpen(false)}
          libraryDataRoot={libraryRoot}
          catalogRevision={library.data.revision}
          profiles={library.data.profiles}
        />
      )}
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
      <ZCodeNativeSwitchSection
        currentIdentity={currentIdentity}
        identityReading={identityReading}
        operation={originalOperation}
        queryState={operationQueryState}
        operationPending={operationPending}
        controlsDisabled={locked || !!action || !!checkingId}
        hasInspectedSource={context !== null}
        onReadIdentity={() => void readCurrentIdentity()}
        onQueryOriginal={() => void queryOriginalExplicitly()}
        onViewRecovery={() => {
          recoveryRef.current?.scrollIntoView({ block: "start" });
          recoveryRef.current?.focus();
        }}
      />
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
      {shownCatalog && (
        <ZCodeDeviceManager
          dataRoot={libraryRoot}
          disabled={disabled || busy}
        />
      )}
      {shownCatalog && (
        <ZCodeClaimControls
          dataRoot={libraryRoot}
          profiles={shownCatalog.profiles}
          disabled={disabled || busy}
        />
      )}
      {shownCatalog &&
        accountGroups.map(({ family, profiles, empty }) => (
          <section
            key={family ?? "all"}
            aria-label={
              family === null
                ? copy("title", "Saved ZCode accounts")
                : family === "zai"
                  ? copy("zaiFamily", "Z.ai accounts")
                  : copy("bigmodelFamily", "BigModel accounts")
            }
            className="space-y-2"
          >
            {family && (
              <h4 className="text-sm font-medium">
                {family === "zai" ? "Z.ai" : "BigModel"}
              </h4>
            )}
            {
              <AccountWorkspace
                accounts={profiles}
                label={(profile) => profile.label ?? profile.id}
                identityDetails={(profile) => (
                  <span>{profile.family === "zai" ? "Z.ai" : "BigModel"}</span>
                )}
                enabled={unified}
              >
                {(profile) => {
                  const nativeAuthorization = (
                    <Card key={profile.id}>
                      <CardContent className="flex flex-wrap items-center justify-between gap-3 p-3">
                        <div className="min-w-0">
                          <p className="break-all text-sm">
                            {profile.label ?? profile.id}
                          </p>
                          <p className="break-all text-xs text-muted-foreground">
                            {profile.id}
                          </p>
                          {profile.deviceMid && (
                            <p className="break-all text-xs text-muted-foreground">
                              {copy("deviceIdentity", "Device ID: {{device}}", {
                                device: profile.deviceMid,
                              })}
                            </p>
                          )}
                          {profile.officialLabel &&
                            profile.officialLabel !== profile.label && (
                              <p className="text-xs text-muted-foreground">
                                {profile.officialLabel}
                              </p>
                            )}
                          {profile.identitySource && (
                            <p className="text-xs text-muted-foreground">
                              {profile.identitySource === "officialLogin"
                                ? copy(
                                    "identityOfficialLogin",
                                    "Official sign-in",
                                  )
                                : profile.identitySource === "nativeCapture"
                                  ? copy(
                                      "identityNativeCapture",
                                      "Captured native session",
                                    )
                                  : copy(
                                      "identityPackageDeclared",
                                      "Declared by account bundle",
                                    )}
                            </p>
                          )}
                          {currentIdentity?.id === profile.id && (
                            <p className="text-xs text-blue-600">
                              {copy(
                                "currentNativeMarker",
                                "Current native identity at last explicit read",
                              )}
                            </p>
                          )}
                          {profile.capabilities ? (
                            <ZCodeAccountEvidence
                              section={unified ? "authorization" : "all"}
                              value={profile.capabilities}
                              quotaPresentation={
                                connectionViews[profile.id] ?? "previous"
                              }
                            />
                          ) : (
                            <p className="text-xs text-muted-foreground">
                              {copy(
                                "capabilitiesNotQueried",
                                "Connection and quota checks have not been queried.",
                              )}
                            </p>
                          )}
                          {profile.activationBlockedReason && (
                            <p className="text-xs text-muted-foreground">
                              {errorText(profile.activationBlockedReason)}
                            </p>
                          )}
                          {profile.needsKey && (
                            <p className="text-xs text-muted-foreground">
                              {copy(
                                "savedCodingPending",
                                "Coding connection needs a Key; other capabilities are shown independently.",
                              )}
                            </p>
                          )}
                          {profile.completeCodingBlockedReason && (
                            <p className="text-xs text-muted-foreground">
                              {errorText(profile.completeCodingBlockedReason)}
                            </p>
                          )}
                          {profile.checkConnectionsBlockedReason && (
                            <p className="text-xs text-muted-foreground">
                              {errorText(profile.checkConnectionsBlockedReason)}
                            </p>
                          )}
                        </div>
                        <div className="flex flex-wrap gap-2">
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={
                              !libraryReady ||
                              loading ||
                              profile.canCheckConnections !== true ||
                              !!action
                            }
                            onClick={() => void checkConnections(profile.id)}
                          >
                            {checkingId === profile.id
                              ? copy(
                                  "checkingConnections",
                                  "Checking connections…",
                                )
                              : copy(
                                  "checkConnections",
                                  "Check connections and quota",
                                )}
                          </Button>
                          {checkingId === profile.id && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => retireConnectionCheck(true, true)}
                            >
                              {copy("cancelConnectionCheck", "Cancel check")}
                            </Button>
                          )}
                          {profile.needsKey && (
                            <Button
                              variant="outline"
                              size="sm"
                              disabled={
                                !libraryReady ||
                                profile.canCompleteCoding !== true ||
                                !!action
                              }
                              onClick={() => {
                                completeSavedCoding(profile.id);
                              }}
                            >
                              {copy(
                                "completeSavedCoding",
                                "Complete Coding connection",
                              )}
                            </Button>
                          )}
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={
                              !libraryReady ||
                              library.data?.actions.canEditLabels !== true ||
                              !!action
                            }
                            onClick={() => editLabel(profile.id)}
                          >
                            {copy("editDisplayName", "Edit display name")}
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={
                              !ordinaryReady ||
                              profile.canActivate !== true ||
                              originalUnresolved ||
                              full ||
                              !!action
                            }
                            onClick={() => review("switch", profile.id)}
                          >
                            {copy(
                              "nativeSwitch.switch",
                              "Switch to this account",
                            )}
                          </Button>
                        </div>
                        {originalUnresolved && (
                          <p className="w-full text-xs text-muted-foreground">
                            {copy(
                              "nativeSwitch.resolveOriginal",
                              "Query the unresolved original operation before starting another switch.",
                            )}
                          </p>
                        )}
                        {needsReview && !originalUnresolved && (
                          <p className="w-full text-xs text-muted-foreground">
                            {copy(
                              "nativeSwitch.refreshBeforeReview",
                              "Refresh account status before reviewing a new switch.",
                            )}
                          </p>
                        )}
                        {!context && (
                          <p className="w-full text-xs text-muted-foreground">
                            {copy(
                              "nativeSwitch.switchInspectFirst",
                              "Inspect a supported native source before switching to this account.",
                            )}
                          </p>
                        )}
                        {context &&
                          profile.canActivate !== true &&
                          !profile.activationBlockedReason && (
                            <p className="w-full text-xs text-muted-foreground">
                              {copy(
                                "nativeSwitch.notEligible",
                                "The backend has not granted native activation for this account.",
                              )}
                            </p>
                          )}
                        <p className="w-full text-xs text-muted-foreground">
                          {copy(
                            "connectionCheckDisclosure",
                            "Only this account's related session credentials are sent to its official platform to read connections, plans and quota. No model calls, Key creation, account switching or automatic sign-in.",
                          )}
                        </p>
                      </CardContent>
                    </Card>
                  );
                  if (!unified) return nativeAuthorization;
                  return (
                    <AccountDetailSections
                      authorization={nativeAuthorization}
                      entitlements={
                        profile.capabilities ? (
                          <ZCodeAccountEvidence
                            value={profile.capabilities}
                            section="entitlements"
                            quotaPresentation={
                              connectionViews[profile.id] ?? "previous"
                            }
                          />
                        ) : (
                          <p className="text-xs text-muted-foreground">
                            {copy(
                              "capabilitiesNotQueried",
                              "Connection and quota checks have not been queried.",
                            )}
                          </p>
                        )
                      }
                      inference={
                        <p className="text-xs text-muted-foreground">
                          真实推理消费尚未验收。会话检查、套餐额度和本机切号均不等于实际应用调用通过；保留原本机切号、备份与恢复操作。
                        </p>
                      }
                    />
                  );
                }}
              </AccountWorkspace>
            }
            {family && empty && (
              <p className="text-sm text-muted-foreground">
                {copy("empty", "No saved accounts in this family.")}
              </p>
            )}
          </section>
        ))}
      {unified && !localImportOpen && (
        <Collapsible className="space-y-3 rounded-md border p-3">
          <CollapsibleTrigger asChild>
            <Button type="button" variant="outline" size="sm">
              {copy("installationStatus", "Installation and versions")}
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent className="space-y-3">
            {nativeTools}
          </CollapsibleContent>
        </Collapsible>
      )}
      <section
        className="space-y-3"
        ref={recoveryRef}
        tabIndex={-1}
        aria-label={copy("recoveryTitle", "Local account recovery")}
      >
        <h4 className="text-sm font-medium">
          {copy("recoveryTitle", "Local account recovery")}
        </h4>
        {recovery.data && !recovery.isError && (
          <p className="text-xs text-muted-foreground">
            {copy(
              "localHelp",
              "Local recovery can be inspected, preserved and cleaned up without a verified ZCode installation. Unconfirmed records require explicit verification before deletion.",
            )}
          </p>
        )}
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
      <Dialog
        open={labelEditor !== null}
        onOpenChange={(open) => {
          if (!open) closeLabel();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {copy("editDisplayName", "Edit display name")}
            </DialogTitle>
            <DialogDescription>
              {copy(
                "localLabelOnly",
                "This label is stored locally. It does not change the official identity or account deduplication.",
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2 px-6 py-5">
            <Label htmlFor="zcode-local-label">
              {copy("displayName", "Display name")}
            </Label>
            <Input
              id="zcode-local-label"
              value={labelEditor?.value ?? ""}
              disabled={busy}
              onChange={(event) => changeLabel(event.target.value)}
            />
            <p className="text-xs text-muted-foreground">
              {copy(
                "localLabelReset",
                "Leave empty to use the account's official display label.",
              )}
            </p>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {errorText(error)}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => closeLabel()}>
              {copy("cancel", "Cancel account action")}
            </Button>
            <Button disabled={busy} onClick={() => void saveLabel()}>
              {copy("saveDisplayName", "Save display name")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        key={action?.kind ?? "closed"}
        isOpen={action !== null}
        pending={busy}
        title={confirmation.title}
        message={confirmation.message}
        confirmText={confirmation.confirm}
        cancelText={copy("cancel", "Cancel account action")}
        variant={action?.kind === "delete" ? "destructive" : "info"}
        onCancel={() => void cancel()}
        checkboxLabel={
          action?.kind === "saveCapture" && action.preview?.duplicate
            ? copy(
                "updateDuplicate",
                "Explicitly update this existing saved account",
              )
            : undefined
        }
        checkboxDefaultChecked={false}
        onConfirm={(checked) => void confirmReviewedAction(checked)}
      />
    </section>
  );
}
