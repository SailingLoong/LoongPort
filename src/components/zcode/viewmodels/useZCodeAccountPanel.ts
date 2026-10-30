import { settingsApi } from "@/lib/api/settings";

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  readZCodeSourcePreference,
  saveZCodeSourcePreference,
} from "@/lib/zcodeSourcePreference";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import {
  accountErrorText,
  ConfirmedSwitchFailure,
  safeAccountError,
  zcodeAccountsApi,
  type AccountError,
  type ContextSelection,
  type CapturePreview,
  type LatestVersion,
  type SourceContext,
  type CurrentNativeIdentity,
  type CatalogStatus,
  type SwitchOperationStatus,
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
export function useZCodeAccountPanel({
  disabled = false,
  unified = false,
  onBusyChange,
}: {
  disabled?: boolean;
  unified?: boolean;
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
  const [localImportOpen, setLocalImportOpen] = useState(false);
  const localImportRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (localImportOpen) localImportRef.current?.focus();
  }, [localImportOpen]);
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
  const [originalOperation, setOriginalOperation] =
    useState<SwitchOperationStatus | null>(null);
  const [operationQueryState, setOperationQueryState] = useState<
    "idle" | "pending" | "ready" | "error" | "changed"
  >("idle");
  const [operationPending, setOperationPending] = useState(false);
  const [originalUnresolved, setOriginalUnresolved] = useState(false);
  const unresolvedSources = useRef(
    new Map<
      string,
      {
        revision: string;
        operation: SwitchOperationStatus | null;
        request: Pick<SwitchOperationStatus, "requestId" | "target"> | null;
        identityUnknown?: boolean;
      }
    >(),
  );
  const operationEpoch = useRef(0);
  const operationBinding = useRef<{
    sourceKey: string;
    revision: string;
  } | null>(null);
  const selectedSourceKey = JSON.stringify(source);
  const recoveryRef = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (
      operationBinding.current?.sourceKey === selectedSourceKey &&
      operationBinding.current.revision === context?.contextRevision
    )
      return;
    operationEpoch.current += 1;
    operationBinding.current = null;
    setOriginalOperation(null);
    setOperationQueryState("idle");
    setOriginalUnresolved(unresolvedSources.current.has(selectedSourceKey));
  }, [selectedSourceKey, context?.contextRevision]);
  useEffect(
    () => () => {
      operationEpoch.current += 1;
    },
    [],
  );
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
  const begin = (preserveNotice = false) => {
    if (inFlight.current || disabled) return false;
    inFlight.current = true;
    setBusy(true);
    if (!preserveNotice) setNotice(null);
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
  const invalidateCurrentIdentity = useCallback(() => {
    identityEpoch.current += 1;
    identityInFlight.current = false;
    setCurrentIdentity(null);
    setIdentityReading(false);
  }, []);
  useLayoutEffect(() => {
    invalidateCurrentIdentity();
    return () => {
      identityEpoch.current += 1;
      identityInFlight.current = false;
    };
  }, [
    source.installPath,
    source.dataRoot,
    source.keyMode,
    context?.contextRevision,
    invalidateCurrentIdentity,
  ]);
  useEffect(() => {
    const visibility = () => {
      if (document.visibilityState !== "visible") invalidateCurrentIdentity();
    };
    window.addEventListener("blur", invalidateCurrentIdentity);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      window.removeEventListener("blur", invalidateCurrentIdentity);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [invalidateCurrentIdentity]);
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
  const queryOriginal = async (
    revision: string,
    preserveNotice = false,
    observedFailure: SwitchOperationStatus | null = null,
  ) => {
    const snapshot = { ...source };
    const sourceKey = JSON.stringify(snapshot);
    const retained = unresolvedSources.current.get(sourceKey);
    const epoch = ++operationEpoch.current;
    operationBinding.current = { sourceKey, revision };
    setOperationQueryState("pending");
    try {
      const result = await zcodeAccountsApi.queryLastOperationState(
        snapshot,
        revision,
        retained?.identityUnknown
          ? undefined
          : (retained?.request ?? undefined),
      );
      if (operationEpoch.current !== epoch) return;
      if (retained?.identityUnknown) {
        // Unreadable pointer storage cannot prove that any later request is
        // the previously unknown operation. This is not a new dispatch.
        setOriginalOperation(null);
        setOperationQueryState("error");
        setOriginalUnresolved(true);
        setNeedsReview(true);
        return;
      }
      // A locally verified failed receipt remains evidence if a later query
      // temporarily fails after its pointer was forgotten. Null alone is not clearance.
      const verifiedFailure =
        observedFailure ??
        (retained?.revision === revision &&
        retained.operation?.phase === "failed"
          ? retained.operation
          : null);
      const operation =
        result.operation ??
        (verifiedFailure &&
        (!result.request ||
          (result.request.requestId === verifiedFailure.requestId &&
            result.request.target === verifiedFailure.target))
          ? verifiedFailure
          : null);
      const prior = retained?.request;
      if (
        prior &&
        ((result.request &&
          (result.request.requestId !== prior.requestId ||
            result.request.target !== prior.target)) ||
          (operation &&
            (operation.requestId !== prior.requestId ||
              operation.target !== prior.target)))
      ) {
        // Another same-source request cannot resolve the original physical write.
        setOriginalOperation(
          retained.revision === revision ? retained.operation : null,
        );
        setOperationQueryState("changed");
        setOriginalUnresolved(true);
        setNeedsReview(true);
        return;
      }
      // Missing records and read failures retain the same captured request identity.
      // Absence cannot erase earlier unresolved physical-operation evidence.
      if (result.error || (!operation && (retained || result.request))) {
        if (!retained?.request)
          unresolvedSources.current.set(sourceKey, {
            ...(retained ?? { revision, operation: observedFailure }),
            identityUnknown:
              !!result.error && !result.request && !observedFailure,
            request:
              result.request ??
              (observedFailure
                ? {
                    requestId: observedFailure.requestId,
                    target: observedFailure.target,
                  }
                : null),
          });
        setOriginalOperation(
          retained?.revision === revision
            ? retained.operation
            : observedFailure,
        );
        if (result.error) setError(safeAccountError(result.error));
        setOriginalUnresolved(true);
        setOperationQueryState("error");
        setNeedsReview(true);
        return;
      }
      setOriginalOperation(operation);
      setOperationQueryState("ready");
      const unresolved =
        operation !== null &&
        operation.phase !== "failed" &&
        operation.phase !== "restartVerified" &&
        !(operation.phase === "committed" && !operation.restartRequested);
      if (unresolved)
        unresolvedSources.current.set(sourceKey, {
          revision,
          operation,
          request: operation
            ? { requestId: operation.requestId, target: operation.target }
            : null,
        });
      else unresolvedSources.current.delete(sourceKey);
      setOriginalUnresolved(unresolved);
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
        if (!preserveNotice)
          setNotice(
            copy(
              "originalConfirmed",
              "The original local switch is confirmed. Online sign-in remains unverified.",
            ),
          );
      } else if (operation.phase === "failed") {
        if (!preserveNotice)
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
    } catch (cause) {
      if (operationEpoch.current !== epoch) return;
      setOperationQueryState("error");
      if (!retained?.request) {
        unresolvedSources.current.set(sourceKey, {
          ...(retained ?? { revision, operation: observedFailure }),
          identityUnknown: !observedFailure,
          request: observedFailure
            ? {
                requestId: observedFailure.requestId,
                target: observedFailure.target,
              }
            : null,
        });
        if (observedFailure) setOriginalOperation(observedFailure);
      } else
        setOriginalOperation(
          retained.revision === revision ? retained.operation : null,
        );
      setOriginalUnresolved(true);
      setError(safeAccountError(cause));
      setNeedsReview(true);
    }
  };
  const queryOriginalExplicitly = async () => {
    if (
      !context ||
      locked ||
      action ||
      identityInFlight.current ||
      !begin(true)
    )
      return;
    setError(null);
    try {
      await queryOriginal(context.contextRevision);
    } finally {
      finish();
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
    if (!begin(true)) return;
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
        originalUnresolved ||
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
    let switchDispatched = false;
    let switchEpoch: number | null = null;
    const invocation: {
      request: Pick<SwitchOperationStatus, "requestId" | "target"> | null;
    } = { request: null };
    let confirmedFailure: SwitchOperationStatus | null = null;
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
          // The previous observation is stale even if the switch reply is lost.
          invalidateCurrentIdentity();
          switchDispatched = true;
          switchEpoch = operationEpoch.current;
          operationBinding.current = {
            sourceKey: JSON.stringify(selected.source),
            revision: selected.contextRevision!,
          };
          unresolvedSources.current.set(JSON.stringify(selected.source), {
            revision: selected.contextRevision!,
            operation: null,
            request: null,
          });
          setOriginalUnresolved(true);
          setOriginalOperation(null);
          setOperationQueryState("idle");
          setOperationPending(true);
          try {
            result = await zcodeAccountsApi.switch(
              selected.source,
              selected.contextRevision!,
              selected.id!,
              selected.catalogRevision!,
              (evidence) => {
                if (
                  switchEpoch !== operationEpoch.current ||
                  evidence.contextRevision !== selected.contextRevision ||
                  JSON.stringify(evidence.source) !==
                    JSON.stringify(selected.source)
                )
                  return;
                invocation.request = {
                  requestId: evidence.requestId,
                  target: evidence.target,
                };
                // Only this wrapper invocation can advance from an inspected
                // previous terminal request to its newly allocated request.
                unresolvedSources.current.set(JSON.stringify(selected.source), {
                  revision: selected.contextRevision!,
                  operation: null,
                  request: invocation.request,
                });
              },
            );
          } finally {
            setOperationPending(false);
          }
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
      if (
        switchDispatched &&
        switchEpoch === operationEpoch.current &&
        cause instanceof ConfirmedSwitchFailure &&
        invocation.request !== null &&
        cause.operation.requestId === invocation.request.requestId &&
        cause.operation.target === invocation.request.target &&
        cause.operation.phase === "failed" &&
        !unresolvedSources.current.get(JSON.stringify(selected.source))
          ?.identityUnknown &&
        cause.contextRevision === selected.contextRevision &&
        JSON.stringify(cause.source) === JSON.stringify(selected.source) &&
        cause.operation.target === selected.id
      ) {
        // Only the wrapper's request-bound terminal evidence permits a fresh
        // review. Generic errors and missing records never provide clearance.
        confirmedFailure = cause.operation;
        unresolvedSources.current.set(JSON.stringify(selected.source), {
          revision: selected.contextRevision!,
          operation: cause.operation,
          request: invocation.request,
        });
        setOriginalUnresolved(false);
      }
      setError(safeAccountError(cause));
      setNeedsReview(true);
      setAction(null);
    } finally {
      // The backend query shares the native mutex: query only after settlement.
      if (switchDispatched) {
        if (!invocation.request && switchEpoch === operationEpoch.current)
          unresolvedSources.current.set(JSON.stringify(selected.source), {
            revision: selected.contextRevision!,
            operation: null,
            request: null,
            identityUnknown: true,
          });
        await queryOriginal(selected.contextRevision!, true, confirmedFailure);
      }
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
            "nativeSwitch.confirmation",
            "Target: {{label}} · {{id}} · {{family}}\nInstallation: {{installation}}\nData directory: {{root}}\nInspected version/build: {{version}} / {{build}}\nThe existing safe process handles a running client; it will not continue if it cannot exit safely. Local write completion does not verify online sign-in. Read local identity separately and verify in the official client.",
            {
              root,
              id,
              label:
                catalog.data?.profiles.find((profile) => profile.id === id)
                  ?.label ?? id,
              family:
                catalog.data?.profiles.find((profile) => profile.id === id)
                  ?.family ?? "",
              installation: action.source.installPath,
              version: context?.version ?? "",
              build: context?.build ?? "",
            },
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
  const accountFamilies: Array<"zai" | "bigmodel" | null> = unified
    ? [null]
    : ["zai", "bigmodel"];
  // All consumers derive account membership from the same displayed catalog.
  // Keep grouping and device bindings out of the View so source changes cannot
  // leave child workflows using a different set of profiles.
  const accountGroups = accountFamilies.map((family) => {
    const profiles = (shownCatalog?.profiles ?? []).filter(
      (profile) => family === null || profile.family === family,
    );
    return { family, profiles, empty: profiles.length === 0 };
  });
  const boundDeviceMids = (shownCatalog?.profiles ?? []).flatMap((profile) =>
    profile.deviceMid ? [profile.deviceMid] : [],
  );
  const confirmation = actionCopy();
  const recheckMetadata = () => {
    discoveryInstallation.current = source.installPath || undefined;
    manuallySelected.current = false;
    setContext(null);
    setNeedsReview(true);
    void discovery.refetch();
  };
  const openOfficialGuide = () =>
    settingsApi
      .openExternal("https://zcode.z.ai/cn")
      .catch((cause) => setError(safeAccountError(cause)));
  const addAccount = () => {
    setCodingAccount(undefined);
    setLocalImportOpen(false);
    setLoginOpen(true);
  };
  const completeSavedCoding = (id: string) => {
    if (!shownCatalog) return;
    setCodingAccount({ id, catalogRevision: shownCatalog.revision });
    setLoginOpen(true);
  };
  const hasSourcePreference = !!preferredSource.current;
  const closeLogin = () => {
    setLoginOpen(false);
    setCodingAccount(undefined);
  };
  const editLabel = (id: string) => {
    const profile = shownCatalog?.profiles.find((profile) => profile.id === id);
    if (
      !profile ||
      !libraryReady ||
      library.data?.actions.canEditLabels !== true ||
      action
    )
      return;
    setLabelEditor({
      id,
      value: profile.label ?? "",
      revision: shownCatalog!.revision,
      root: libraryRoot,
    });
  };
  const changeLabel = (value: string) =>
    setLabelEditor((previous) => (previous ? { ...previous, value } : null));
  const closeLabel = () => setLabelEditor(null);
  return {
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
    confirmReviewedAction: perform,
    accountGroups,
    boundDeviceMids,
    confirmation,
  };
}
