import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  readZCodeSourcePreference,
  saveZCodeSourcePreference,
} from "@/lib/zcodeSourcePreference";
import { settingsApi } from "@/lib/api/settings";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ZCodeBundleImport } from "./ZCodeBundleImport";
import { ZCodeClaimControls } from "./ZCodeClaimControls";
import { ZCodeAccountEvidence } from "./ZCodeAccountEvidence";
import { ZCodeOAuthAdd } from "./ZCodeOAuthAdd";
import { ZCodeBackupDialog } from "./ZCodeBackupDialog";
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
  zcodeAccountsApi,
  type AccountError,
  type ContextSelection,
  type CapturePreview,
  type LatestVersion,
  type SourceContext,
  type CurrentNativeIdentity,
  type CatalogStatus,
} from "@/lib/api/zcodeAccounts";

type ActionKind =
  | "capture"
  | "saveCapture"
  | "switch"
  | "archive"
  | "confirm"
  | "recapture"
  | "delete";
type ReviewedAction = {
  kind: ActionKind;
  id?: string;
  preview?: CapturePreview;
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
  const queryClient = useQueryClient();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string>,
  ) => t(`zcode.accounts.${key}`, { defaultValue, ...values });
  const preferredSource = useRef(readZCodeSourcePreference());
  const [source, setSource] = useState<ContextSelection>(() => ({
    ...initialSource,
    ...preferredSource.current,
    keyMode: "unknown",
  }));
  const [context, setContext] = useState<SourceContext | null>(null);
  const [busy, setBusy] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  const [codingAccount, setCodingAccount] = useState<
    { id: string; catalogRevision: string } | undefined
  >(undefined);
  const [backupOpen, setBackupOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [labelEditor, setLabelEditor] = useState<{
    id: string;
    value: string;
    revision: string;
    root: string | undefined;
  } | null>(null);
  const [currentIdentity, setCurrentIdentity] =
    useState<CurrentNativeIdentity | null>(null);
  const [identityReading, setIdentityReading] = useState(false);
  const identityEpoch = useRef(0);
  const identityInFlight = useRef(false);
  const [checkingId, setCheckingId] = useState<string | null>(null);
  const [connectionViews, setConnectionViews] = useState<
    Record<string, "checking" | "current" | "unknown">
  >({});
  const connectionEpoch = useRef(0);
  const connectionRequest = useRef<{
    requestId: string;
    id: string;
    root: string | undefined;
    revision: string;
    epoch: number;
    unconfirmed: boolean;
  } | null>(null);
  const inFlight = useRef(false);
  const [action, setAction] = useState<ReviewedAction | null>(null);
  const [error, setError] = useState<AccountError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [latestVersion, setLatestVersion] = useState<LatestVersion | null>(
    null,
  );
  const [needsReview, setNeedsReview] = useState(false);
  const manuallySelected = useRef(preferredSource.current !== null);
  const discoveryInstallation = useRef(preferredSource.current?.installPath);
  const discovery = useQuery({
    queryKey: ["zcodeMetadataDiscovery"],
    queryFn: () =>
      passive(() => zcodeAccountsApi.discover(discoveryInstallation.current)),
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  useEffect(() => {
    if (
      !discovery.data ||
      manuallySelected.current ||
      inFlight.current ||
      action ||
      loginOpen ||
      backupOpen ||
      importOpen ||
      labelEditor
    )
      return;
    // A single detected installation is shown without granting compatibility.
    setSource({
      installPath:
        discovery.data.candidates.length === 1
          ? discovery.data.candidates[0].installPath
          : discovery.data.candidates.length > 1
            ? ""
            : initialSource.installPath,
      dataRoot: discovery.data.dataRoot,
      keyMode: "unknown",
    });
    setContext(null);
    setNeedsReview(false);
  }, [discovery.data, action, loginOpen, backupOpen, importOpen, labelEditor]);
  const libraryRoot = source.dataRoot || undefined;
  const libraryKey = ["zcodeAccountLibrary", libraryRoot ?? null];
  const library = useQuery({
    queryKey: libraryKey,
    queryFn: () => passive(() => zcodeAccountsApi.library(libraryRoot)),
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const recovery = useQuery({
    queryKey: ["zcodeAccountRecovery"],
    queryFn: () => passive(zcodeAccountsApi.recoveryStatus),
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const nativeCatalogKey = [
    "zcodeAccountCatalog",
    source.installPath,
    source.dataRoot,
    source.keyMode,
    context?.contextRevision,
  ];
  const catalog = useQuery({
    queryKey: nativeCatalogKey,
    queryFn: () =>
      passive(() => zcodeAccountsApi.status(source, context!.contextRevision)),
    enabled: context !== null,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  useEffect(() => {
    onBusyChange?.(
      busy || loginOpen || backupOpen || importOpen || !!labelEditor,
    );
  }, [busy, loginOpen, backupOpen, importOpen, labelEditor, onBusyChange]);
  useEffect(() => () => onBusyChange?.(false), [onBusyChange]);
  const locked =
    busy || disabled || loginOpen || backupOpen || importOpen || !!labelEditor;
  const loading =
    recovery.isFetching ||
    library.isFetching ||
    (context !== null && catalog.isFetching);
  const shownCatalog =
    context && catalog.data && !catalog.isError ? catalog.data : library.data;
  const connectionBinding = useRef({
    root: libraryRoot,
    revision: shownCatalog?.revision,
  });
  connectionBinding.current = {
    root: libraryRoot,
    revision: shownCatalog?.revision,
  };
  const libraryReady =
    !!library.data &&
    !library.isError &&
    !library.isFetching &&
    !locked &&
    !checkingId;
  const localReady =
    !!recovery.data &&
    !recovery.isError &&
    !recovery.isFetching &&
    !locked &&
    !checkingId;
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
    (context && catalog.isError ? safeAccountError(catalog.error) : null) ??
    (library.isError ? safeAccountError(library.error) : null);
  const errorText = (failure: AccountError) => {
    const key =
      failure.committed &&
      ![
        "zcode.account.committed_restart_failed",
        "zcode.account.committed_result_unknown",
      ].includes(failure.code)
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
  const retireConnectionCheck = (render: boolean, explicit = false) => {
    const request = connectionRequest.current;
    connectionRequest.current = null;
    const cancelledEpoch = ++connectionEpoch.current;
    if (!request) return;
    if (render) {
      setCheckingId(null);
      setConnectionViews((previous) => {
        const next = { ...previous };
        if (explicit) next[request.id] = "unknown";
        else delete next[request.id];
        return next;
      });
    }
    void zcodeAccountsApi
      .cancelConnectionCheck(request.requestId)
      .then((outcome) => {
        if (
          render &&
          explicit &&
          connectionEpoch.current === cancelledEpoch &&
          connectionBinding.current.root === request.root
        ) {
          setNotice(
            outcome === "tooLate"
              ? copy(
                  "connectionCheckTooLate",
                  "The check may already have been saved. Refresh local account status to see its result.",
                )
              : copy(
                  "connectionCheckCancelled",
                  "Connection check cancelled before saving its result.",
                ),
          );
        }
      })
      .catch(() => {
        if (render && explicit && connectionEpoch.current === cancelledEpoch)
          setNotice(
            copy(
              "connectionCheckCancelUnknown",
              "The cancellation response is unknown. Refresh local account status before checking again.",
            ),
          );
      });
  };
  useLayoutEffect(() => {
    setCheckingId(null);
    setConnectionViews({});
    return () => retireConnectionCheck(false);
  }, [libraryRoot]);
  useLayoutEffect(() => {
    const request = connectionRequest.current;
    if (
      request &&
      (request.root !== libraryRoot ||
        request.revision !== shownCatalog?.revision)
    )
      retireConnectionCheck(true);
  }, [libraryRoot, shownCatalog?.revision]);
  const checkConnections = async (id: string) => {
    const profile = shownCatalog?.profiles.find((item) => item.id === id);
    if (
      !libraryReady ||
      loading ||
      locked ||
      action ||
      !shownCatalog ||
      profile?.canCheckConnections !== true ||
      inFlight.current
    )
      return;
    if (connectionRequest.current) {
      if (!connectionRequest.current.unconfirmed) return;
      retireConnectionCheck(false);
    }
    const request = {
      requestId: crypto.randomUUID(),
      id,
      root: libraryRoot,
      revision: shownCatalog.revision,
      epoch: ++connectionEpoch.current,
      unconfirmed: false,
    };
    const observedLibraryRevision =
      queryClient.getQueryData<CatalogStatus>(libraryKey)?.revision;
    const observedNativeRevision = context
      ? queryClient.getQueryData<CatalogStatus>(nativeCatalogKey)?.revision
      : undefined;
    const current = () =>
      connectionEpoch.current === request.epoch &&
      connectionBinding.current.root === request.root;
    const recordUnchanged = () =>
      connectionBinding.current.revision === request.revision &&
      queryClient.getQueryData<CatalogStatus>(libraryKey)?.revision ===
        observedLibraryRevision &&
      (!context ||
        queryClient.getQueryData<CatalogStatus>(nativeCatalogKey)?.revision ===
          observedNativeRevision);
    connectionRequest.current = request;
    setCheckingId(id);
    setConnectionViews((previous) => ({ ...previous, [id]: "checking" }));
    setError(null);
    setNotice(null);
    try {
      await queryClient.cancelQueries({ queryKey: libraryKey, exact: true });
      if (context)
        await queryClient.cancelQueries({
          queryKey: nativeCatalogKey,
          exact: true,
        });
      if (!current() || connectionRequest.current !== request) return;
      if (!recordUnchanged()) {
        retireConnectionCheck(true);
        return;
      }
      const result = await zcodeAccountsApi.checkConnections({
        requestId: request.requestId,
        ...(request.root === undefined ? {} : { dataRoot: request.root }),
        catalogRevision: request.revision,
        id,
        allowOfficialCheck: true,
      });
      if (!current()) return;
      if (!recordUnchanged()) {
        retireConnectionCheck(true);
        return;
      }
      connectionRequest.current = null;
      setCheckingId(null);
      setConnectionViews((previous) => ({ ...previous, [id]: "current" }));
      queryClient.setQueryData(libraryKey, result);
      // The returned library facts are authoritative. Native action eligibility
      // is re-read separately, without repeating the official account check.
      if (context) queryClient.setQueryData(nativeCatalogKey, result);
      setNotice(
        copy(
          "connectionCheckCompleted",
          "Connection and quota check completed for this account.",
        ),
      );
      if (context) {
        const native = await catalog.refetch();
        if (current() && native.error) {
          setError(safeAccountError(native.error));
          setNeedsReview(true);
        }
      }
    } catch (cause) {
      if (current()) {
        request.unconfirmed = true;
        setConnectionViews((previous) => ({ ...previous, [id]: "unknown" }));
        setError(safeAccountError(cause));
      }
    } finally {
      if (current() && connectionRequest.current === request) {
        if (!request.unconfirmed) connectionRequest.current = null;
        setCheckingId(null);
      }
    }
  };
  const queryLatest = async () => {
    if (!begin()) return;
    setLatestVersion(null);
    try {
      setLatestVersion(await zcodeAccountsApi.latestVersion());
    } catch (cause) {
      setError(safeAccountError(cause));
    } finally {
      finish();
    }
  };
  const selectSource = (next: ContextSelection) => {
    if (inFlight.current || locked || action) return;
    manuallySelected.current = true;
    setSource(next);
    setContext(null);
    setAction(null);
    setError(null);
    setNotice(null);
    setNeedsReview(false);
  };
  useLayoutEffect(() => {
    identityEpoch.current += 1;
    identityInFlight.current = false;
    setCurrentIdentity(null);
    setIdentityReading(false);
    return () => {
      identityEpoch.current += 1;
      identityInFlight.current = false;
    };
  }, [
    source.installPath,
    source.dataRoot,
    source.keyMode,
    context?.contextRevision,
  ]);
  useEffect(() => {
    const invalidate = () => {
      identityEpoch.current += 1;
      identityInFlight.current = false;
      setCurrentIdentity(null);
      setIdentityReading(false);
    };
    const visibility = () => {
      if (document.visibilityState !== "visible") invalidate();
    };
    window.addEventListener("blur", invalidate);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      window.removeEventListener("blur", invalidate);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, []);
  const readCurrentIdentity = async () => {
    if (!context || locked || identityInFlight.current) return;
    const epoch = ++identityEpoch.current;
    const revision = context.contextRevision;
    identityInFlight.current = true;
    setIdentityReading(true);
    setCurrentIdentity(null);
    setError(null);
    try {
      const result = await zcodeAccountsApi.readCurrentIdentity(
        source,
        revision,
      );
      if (
        identityEpoch.current === epoch &&
        result.contextRevision === revision
      )
        setCurrentIdentity(result);
    } catch (cause) {
      if (identityEpoch.current === epoch) setError(safeAccountError(cause));
    } finally {
      if (identityEpoch.current === epoch) {
        identityInFlight.current = false;
        setIdentityReading(false);
      }
    }
  };
  const refreshLibrary = async () => {
    const result = await library.refetch();
    if (result.error) throw result.error;
    if (context) {
      const nativeResult = await catalog.refetch();
      if (nativeResult.error) {
        setNeedsReview(true);
        throw nativeResult.error;
      }
    }
  };
  const saveLabel = async () => {
    if (!labelEditor || !begin()) return;
    const selected = labelEditor;
    setError(null);
    try {
      const result = await zcodeAccountsApi.setLabel(
        selected.root,
        selected.revision,
        selected.id,
        selected.value.trim() ? selected.value : null,
      );
      queryClient.setQueryData(
        ["zcodeAccountLibrary", selected.root ?? null],
        result,
      );
      setLabelEditor(null);
      if (context) await catalog.refetch();
    } catch (cause) {
      setError(safeAccountError(cause));
    } finally {
      finish();
    }
  };
  const queryOriginal = async (revision: string) => {
    const operation = await zcodeAccountsApi.queryLastOperation(
      source,
      revision,
    );
    if (!operation) return;
    if (operation.phase === "restartFailed") {
      setError({
        code: "zcode.account.committed_restart_failed",
        remedy: "openNativeSettings",
        committed: true,
      });
    } else if (
      operation.phase === "restartVerified" ||
      (operation.phase === "committed" && !operation.restartRequested)
    ) {
      setNotice(
        copy(
          "originalConfirmed",
          "The original local switch is confirmed. Online sign-in remains unverified.",
        ),
      );
    } else if (operation.phase === "failed") {
      setNotice(
        copy(
          "originalFailed",
          "The original switch ended before a confirmed commit. Review the current source before a new confirmation.",
        ),
      );
    } else {
      setError({
        code:
          operation.phase === "committed"
            ? "zcode.account.committed_result_unknown"
            : "zcode.account.operation_already_known",
        remedy: "queryOriginal",
        committed: operation.phase === "committed",
      });
      setNeedsReview(true);
    }
  };
  const inspect = async () => {
    if (!begin()) return;
    setError(null);
    setContext(null);
    setAction(null);
    try {
      const inspected = await zcodeAccountsApi.inspect(source);
      saveZCodeSourcePreference(source);
      setContext(inspected);
      setNeedsReview(false);
      await queryOriginal(inspected.contextRevision);
    } catch (cause) {
      setError(safeAccountError(cause));
    } finally {
      finish();
    }
  };
  const openForLogin = async () => {
    if (!begin()) return;
    setError(null);
    setNotice(null);
    try {
      await zcodeAccountsApi.openForLogin(source);
      setContext(null);
      setNeedsReview(false);
      setNotice(
        copy(
          "openedForLogin",
          "Official ZCode opened with the selected source. Sign in there, then inspect this source again. Online account status is not verified.",
        ),
      );
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
      const savedLibrary = await library.refetch();
      const accounts = context ? await catalog.refetch() : null;
      if (local.error || accounts?.error || savedLibrary.error)
        throw local.error ?? accounts?.error ?? savedLibrary.error;
      setNeedsReview(false);
      if (context) await queryOriginal(context.contextRevision);
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
          (profile) => profile.id === id && profile.canActivate === true,
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
  const cancel = async () => {
    if (!action || !begin()) return;
    const selected = action;
    setAction(null);
    try {
      if (selected.preview)
        await zcodeAccountsApi.cancelCapture(selected.preview.previewId);
    } catch (cause) {
      setError(safeAccountError(cause));
    } finally {
      finish();
    }
  };
  const perform = async (updateDuplicate = false) => {
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
          selected.kind === "saveCapture" ||
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
          const preview = await zcodeAccountsApi.previewCapture(
            selected.source,
            selected.contextRevision!,
            selected.catalogRevision!,
          );
          setAction({ ...selected, kind: "saveCapture", preview });
          return;
        case "saveCapture":
          result = await zcodeAccountsApi.commitCapture(
            selected.source,
            selected.contextRevision!,
            selected.catalogRevision!,
            selected.preview!.previewId,
            updateDuplicate,
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
      if (selected.kind === "saveCapture" || selected.kind === "recapture") {
        setNotice(
          result === "kept"
            ? copy("kept", "Existing saved account kept unchanged.")
            : result === "refreshed"
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
      await library.refetch();
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
          title: copy(
            "readPreviewTitle",
            "Read current ZCode account for preview?",
          ),
          message: copy(
            "readPreviewMessage",
            "Read the current native session from {{root}} to show a masked preview. Review and confirm saving separately. Canceling the preview does not save an account.",
            { root },
          ),
          confirm: copy("readPreviewConfirm", "Read masked preview"),
        };
      case "saveCapture":
        return {
          title: copy("savePreviewTitle", "Save reviewed ZCode account?"),
          message: copy(
            "savePreviewMessage",
            "Personal {{family}} account {{label}}. Save in LoongPort's encrypted local vault. Native ZCode files remain unchanged. {{duplicate}}",
            {
              family: action.preview!.family,
              label: action.preview!.label ?? "…",
              duplicate: action.preview!.duplicate
                ? copy(
                    "duplicateKeep",
                    "This account already exists; keep the saved account by default.",
                  )
                : copy("newSavedAccount", "This is a new saved account."),
            },
          ),
          confirm: copy("savePreviewConfirm", "Confirm account choice"),
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
            setCodingAccount(undefined);
            setLoginOpen(true);
          }}
        >
          {copy("addAccount", "Add account")}
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
              discoveryInstallation.current = source.installPath || undefined;
              manuallySelected.current = false;
              setContext(null);
              setNeedsReview(true);
              void discovery.refetch();
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
            "Verified operation range: macOS 3.14.4, build 3.14.4.7912, exact accepted artifacts only.",
          )}
        </p>
        <Button
          type="button"
          variant="link"
          size="sm"
          onClick={() => {
            void settingsApi
              .openExternal("https://zcode.z.ai/cn")
              .catch((cause) => setError(safeAccountError(cause)));
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
      {preferredSource.current && !context && (
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
          setLoginOpen(false);
          setCodingAccount(undefined);
        }}
        onSaved={refreshLibrary}
        libraryDataRoot={libraryRoot}
        savedAccount={codingAccount}
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
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {currentIdentity?.id
            ? copy(
                "currentNativeIdentity",
                "Current native account: {{account}} · {{family}}",
                {
                  account: currentIdentity.label ?? currentIdentity.id,
                  family: currentIdentity.family ?? "",
                },
              )
            : copy("currentUnknown", "Current account: unknown")}
        </p>
        {currentIdentity && (
          <p className="text-xs text-muted-foreground">
            {copy(
              "currentReadAt",
              "Read at {{time}}. This is a local observation, not an online status check.",
              { time: new Date(currentIdentity.readAt).toISOString() },
            )}
          </p>
        )}
        <Button
          variant="outline"
          size="sm"
          disabled={!context || locked || identityReading || !!action}
          onClick={() => void readCurrentIdentity()}
        >
          {copy("readCurrentIdentity", "Read current native identity")}
        </Button>
      </div>
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
        <ZCodeClaimControls
          dataRoot={libraryRoot}
          profiles={shownCatalog.profiles}
          disabled={disabled || busy}
        />
      )}
      {shownCatalog &&
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
            {shownCatalog.profiles
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
                      {profile.officialLabel &&
                        profile.officialLabel !== profile.label && (
                          <p className="text-xs text-muted-foreground">
                            {profile.officialLabel}
                          </p>
                        )}
                      {profile.identitySource && (
                        <p className="text-xs text-muted-foreground">
                          {profile.identitySource === "officialLogin"
                            ? copy("identityOfficialLogin", "Official sign-in")
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
                          ? copy("checkingConnections", "Checking connections…")
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
                            setCodingAccount({
                              id: profile.id,
                              catalogRevision: shownCatalog.revision,
                            });
                            setLoginOpen(true);
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
                        onClick={() =>
                          setLabelEditor({
                            id: profile.id,
                            value: profile.label ?? "",
                            revision: shownCatalog.revision,
                            root: libraryRoot,
                          })
                        }
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
                          full ||
                          !!action
                        }
                        onClick={() => review("switch", profile.id)}
                      >
                        {copy("switch", "Switch saved account")}
                      </Button>
                    </div>
                    <p className="w-full text-xs text-muted-foreground">
                      {copy(
                        "connectionCheckDisclosure",
                        "Only this account's related session credentials are sent to its official platform to read connections, plans and quota. No model calls, Key creation, account switching or automatic sign-in.",
                      )}
                    </p>
                  </CardContent>
                </Card>
              ))}
            {shownCatalog.profiles.every(
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
          if (!open) setLabelEditor(null);
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
              onChange={(event) =>
                setLabelEditor((previous) =>
                  previous ? { ...previous, value: event.target.value } : null,
                )
              }
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
            <Button variant="outline" onClick={() => setLabelEditor(null)}>
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
        onConfirm={(checked) => void perform(checked)}
      />
    </section>
  );
}
