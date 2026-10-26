import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { toast } from "sonner";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import {
  completedProviderEdit,
  isCodexCatalogRestorePreview,
  isCodexCatalogRestoreRequest,
  matchesCodexCatalogRestoreResult,
  type CodexCatalogRestorePreview,
  type CodexCatalogRestoreRequest,
  type CodexCatalogRestoreResult,
  isProviderEditRequest,
  matchesProviderEditResult,
  type ProviderCatalogDecision,
  type ProviderEditSettings,
  type ProviderEditPreview,
  type ProviderEditRequest,
  type ProviderEditResult,
  type ProviderUpdateInput,
  type ProviderUpdateResult,
} from "@/lib/api/providers";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import type { Provider } from "@/types";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import { CatalogChoiceFields } from "@/components/providers/CatalogChoiceFields";
import { AuthSettingsPanel } from "@/components/providers/AuthSettingsPanel";
import { providersApi, type AppId, type ManagedAuthProvider } from "@/lib/api";

interface EditProviderDialogProps {
  mutationsDisabled?: boolean;
  open: boolean;
  provider: Provider | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (
    payload: ProviderUpdateInput,
  ) => Promise<ProviderUpdateResult | void> | ProviderUpdateResult | void;
  appId: AppId;
  isProxyTakeover?: boolean; // 代理接管模式下不读取 live（避免显示被接管后的代理配置）
}

export function EditProviderDialog(props: EditProviderDialogProps) {
  return props.open && props.provider ? (
    <EditSession
      key={JSON.stringify([props.appId, props.provider.id])}
      {...props}
    />
  ) : null;
}

function EditSession({
  mutationsDisabled = false,
  open,
  provider: openingProvider,
  onOpenChange,
  onSubmit,
  appId,
  isProxyTakeover = false,
}: EditProviderDialogProps) {
  const { t } = useTranslation();
  // One immutable initial record per open/app/id session, never a second draft.
  const [provider] = useState(() => structuredClone(openingProvider));
  const alive = useRef(false);
  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [authSettingsTarget, setAuthSettingsTarget] =
    useState<ManagedAuthProvider | null>(null);

  useEffect(() => {
    setAuthSettingsTarget(null);
  }, [appId, open, provider?.id]);

  const formReadyToken = useMemo(
    () => Symbol("provider-form-ready"),
    [appId, open, provider?.id],
  );
  const currentFormReadyToken = useRef(formReadyToken);
  currentFormReadyToken.current = formReadyToken;
  const [formReadyState, setFormReadyState] = useState({
    token: formReadyToken,
    ready: appId !== "pi",
  });
  const isFormReady =
    formReadyState.token === formReadyToken
      ? formReadyState.ready
      : appId !== "pi";
  const handleSubmitReadyChange = useCallback(
    (ready: boolean) => {
      if (currentFormReadyToken.current === formReadyToken) {
        setFormReadyState({ token: formReadyToken, ready });
      }
    },
    [formReadyToken],
  );

  const [editRead, setEditRead] = useState<ProviderEditSettings | null>(null);
  const [readFailed, setReadFailed] = useState(false);
  const [readAttempt, setReadAttempt] = useState(0);

  const closeDialog = useCallback(() => {
    alive.current = false;
    setAuthSettingsTarget(null);
    onOpenChange(false);
  }, [onOpenChange]);

  const handlePanelClose = useCallback(() => {
    if (authSettingsTarget) {
      setAuthSettingsTarget(null);
      return;
    }
    closeDialog();
  }, [authSettingsTarget, closeDialog]);

  useEffect(() => {
    let cancelled = false;
    setReadFailed(false);
    if (!provider) return;
    void providersApi
      .getEditSettings(provider.id, appId)
      .then((value) => {
        if (cancelled || !alive.current) return;
        if (
          !value ||
          !Object.hasOwn(value, "modeState") ||
          !value.settingsConfig ||
          typeof value.settingsConfig !== "object" ||
          Array.isArray(value.settingsConfig) ||
          (appId === "mcode" &&
            (typeof value.mcodeConfigRevision !== "string" ||
              !value.mcodeConfigRevision.trim()))
        ) {
          setReadFailed(true);
          return;
        }
        setEditRead(value);
        const original = value.originalSave;
        if (value.modeState && original) {
          if (
            !isProviderEditRequest(original.request) ||
            original.request.providerId !== provider.id ||
            !matchesProviderEditResult(original, appId, original.request)
          ) {
            setEditRead(null);
            setReadFailed(true);
            return;
          }
          // A reopened journal result is historical. It can be queried, but
          // cannot claim that this newly opened draft was saved or close it.
          setSubmitted({ request: original.request, version: null });
          setResult(original);
        }
        const originalRestore = value.originalCatalogRestore;
        if (value.modeState && originalRestore) {
          if (
            appId !== "codex" ||
            !isCodexCatalogRestoreRequest(originalRestore.request) ||
            !matchesCodexCatalogRestoreResult(
              originalRestore,
              originalRestore.request,
            )
          ) {
            setEditRead(null);
            setReadFailed(true);
            return;
          }
          // Reopening only recovers the native receipt for querying. It never
          // creates a restore preview or treats this new form as submitted.
          setRestoreResult(originalRestore);
        }
      })
      .catch(() => {
        if (!cancelled && alive.current) setReadFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [provider, appId, readAttempt]);

  const initialSettingsConfig = editRead?.settingsConfig;

  // 固定 initialData，防止 provider 对象更新时重置表单
  const initialData = useMemo(() => {
    if (!provider || !initialSettingsConfig) return null;
    return {
      name:
        appId === "mcode" && typeof initialSettingsConfig.name === "string"
          ? initialSettingsConfig.name
          : provider.name,
      notes: provider.notes,
      websiteUrl: provider.websiteUrl,
      settingsConfig: initialSettingsConfig,
      category: provider.category,
      meta:
        appId === "mcode"
          ? {
              ...provider.meta,
              mcodeConfigRevision: editRead?.mcodeConfigRevision,
            }
          : provider.meta,
      icon: provider.icon,
      iconColor: provider.iconColor,
    };
  }, [
    open, // 修复：编辑保存后再次打开显示旧数据，依赖 open 确保每次打开时重新读取最新 provider 数据
    provider?.id, // 只依赖 ID，provider 对象更新不会触发重新计算
    provider?.meta, // 供应商元数据变化时重新初始化表单
    initialSettingsConfig,
    appId,
    editRead?.mcodeConfigRevision,
  ]);

  const controlled = editRead?.modeState != null;
  const [deleteCredential, setDeleteCredential] = useState(false);
  const draftVersion = useRef(0);
  const formVersion = useRef<number | null>(null);
  const inFlight = useRef(false);
  const capturedSubmit = useRef(false);
  const intent = useRef<"preview" | "confirm">("preview");
  const nextCatalogDecision = useRef<ProviderCatalogDecision | undefined>(
    undefined,
  );
  const [catalogSelection, setCatalogSelection] = useState<
    ProviderCatalogDecision["choice"] | null
  >(null);
  const ticket = useRef<{
    intent: "preview" | "confirm";
    version: number;
    catalogDecision?: ProviderCatalogDecision;
  } | null>(null);
  const [busy, setBusy] = useState<"preview" | "save" | "query" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [review, setReview] = useState<{
    value: ProviderEditPreview;
    catalogDecision?: ProviderCatalogDecision;
    signature: string;
    version: number;
  } | null>(null);
  const [submitted, setSubmitted] = useState<{
    request: ProviderEditRequest;
    version: number | null;
  } | null>(null);
  const [result, setResult] = useState<ProviderEditResult | null>(null);
  const restoreGeneration = useRef(0);
  const [restoreReview, setRestoreReview] = useState<{
    value: CodexCatalogRestorePreview;
    version: number;
  } | null>(null);
  const [restoreResult, setRestoreResult] =
    useState<CodexCatalogRestoreResult | null>(null);
  const invalidateRestore = useCallback(() => {
    restoreGeneration.current += 1;
    setRestoreReview(null);
  }, []);
  useEffect(invalidateRestore, [readAttempt, invalidateRestore]);

  const invalidate = useCallback(() => {
    draftVersion.current += 1;
    invalidateRestore();
    nextCatalogDecision.current = undefined;
    setCatalogSelection(null);
    if (review) setNotice("sourceChanged");
    setReview(null);
  }, [review, invalidateRestore]);
  const handleDraftRevision = useCallback(
    (revision: number) => {
      if (formVersion.current !== revision) {
        formVersion.current = revision;
        invalidate();
      }
    },
    [invalidate],
  );
  const finishResult = useCallback(
    (answer: unknown, request: ProviderEditRequest, version: number | null) => {
      if (!alive.current) return;
      const checked: ProviderEditResult = matchesProviderEditResult(
        answer,
        appId,
        request,
      )
        ? answer
        : { app: appId, request, status: "unknown" };
      setResult(checked);
      // A journal response never leaves an old catalog choice writable.
      setReview(null);
      setCatalogSelection(null);
      nextCatalogDecision.current = undefined;
      if (completedProviderEdit(checked, appId, request)) {
        if (draftVersion.current === version) {
          toast.success(t("notifications.updateSuccess"), {
            closeButton: true,
          });
          closeDialog();
        } else {
          setNotice("savedPreviousDraft");
        }
      }
    },
    [appId, closeDialog, t],
  );

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      if (
        !provider ||
        !initialData ||
        mutationsDisabled ||
        editRead?.modeState?.canWrite === false ||
        inFlight.current ||
        submitted ||
        restoreReview ||
        restoreResult
      )
        return;
      const attempt = ticket.current ?? {
        intent: "preview" as const,
        version: draftVersion.current,
      };
      if (controlled && attempt.version !== draftVersion.current) {
        setNotice("sourceChanged");
        return;
      }
      let parsedConfig: Record<string, unknown>;
      try {
        parsedConfig = JSON.parse(values.settingsConfig) as Record<
          string,
          unknown
        >;
      } catch {
        setNotice("invalidFormat");
        return;
      }
      const nextProviderId =
        (appId === "opencode" || appId === "openclaw" || appId === "pi") &&
        values.providerKey?.trim()
          ? values.providerKey.trim()
          : provider.id;

      const updatedProvider: Provider = {
        ...provider,
        id: nextProviderId,
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        // 保留或更新 meta 字段
        ...(values.meta ? { meta: values.meta } : {}),
      };

      if (!controlled) {
        await onSubmit({
          provider: updatedProvider,
          originalId: provider.id,
          ...(values.credentialWrite ? { credentialWrite: true } : {}),
        });
        if (alive.current) closeDialog();
        return;
      }
      // This immutable comparison witness is never rendered or persisted and
      // never becomes an editable draft. The form remains the only value owner.
      const signature = JSON.stringify({
        values,
        provider: updatedProvider,
        deleteCredential,
      });
      inFlight.current = true;
      setNotice(null);
      try {
        if (attempt.intent === "confirm") {
          if (
            !review ||
            review.value.status !== "ready" ||
            review.version !== attempt.version ||
            review.signature !== signature ||
            catalogSelection !== null
          ) {
            setReview(null);
            setNotice("sourceChanged");
            return;
          }
          const request = review.value.request;
          setSubmitted({ request, version: attempt.version });
          setBusy("save");
          let answer: unknown;
          try {
            answer = await onSubmit({
              provider: updatedProvider,
              originalId: provider.id,
              edit: {
                request,
                deleteCredential,
                ...(values.credentialWrite ? { credentialWrite: true } : {}),
                ...(review.catalogDecision
                  ? { catalogDecision: review.catalogDecision }
                  : {}),
              },
            });
          } catch {
            answer = { app: appId, request, status: "unknown" };
          }
          finishResult(answer, request, attempt.version);
        } else {
          setBusy("preview");
          const id = crypto.randomUUID();
          const value = await providersApi.previewEdit(
            updatedProvider,
            appId,
            provider.id,
            id,
            deleteCredential,
            attempt.catalogDecision,
            ...(values.credentialWrite ? ([true] as const) : []),
          );
          if (!alive.current || draftVersion.current !== attempt.version)
            return;
          if (
            value.app !== appId ||
            value.request?.id !== id ||
            value.request.providerId !== provider.id ||
            !/^[a-f0-9]{64}$/.test(value.request.draftDigest) ||
            !/^[a-f0-9]{64}$/.test(value.request.revision) ||
            !["saveOnly", "saveAndApply"].includes(value.action) ||
            !["ready", "blocked"].includes(value.status)
          ) {
            setNotice("sourceUnavailable");
            return;
          }
          // A requested choice must be acknowledged by this preview. Missing or
          // mismatched facts cannot revive the ready preview from before it.
          if (
            value.status === "ready" &&
            ((attempt.catalogDecision &&
              (!value.catalog ||
                value.catalog.choice !== attempt.catalogDecision.choice ||
                value.catalog.revision !== attempt.catalogDecision.revision)) ||
              (!attempt.catalogDecision && value.catalog?.choice === "manage"))
          ) {
            setNotice("sourceUnavailable");
            return;
          }
          setReview({
            value,
            signature,
            version: attempt.version,
            ...(value.status === "ready" && attempt.catalogDecision
              ? { catalogDecision: attempt.catalogDecision }
              : {}),
          });
        }
      } catch {
        if (alive.current) setNotice("sourceUnavailable");
      } finally {
        inFlight.current = false;
        if (alive.current) setBusy(null);
      }
    },
    [
      appId,
      onSubmit,
      closeDialog,
      provider,
      initialData,
      editRead,
      mutationsDisabled,
      controlled,
      deleteCredential,
      review,
      catalogSelection,
      submitted,
      restoreReview,
      restoreResult,
      finishResult,
    ],
  );

  const handleFormSubmitting = useCallback((value: boolean) => {
    setIsFormSubmitting(value);
    if (!value) capturedSubmit.current = false;
  }, []);
  const queryOriginal = async () => {
    if (!provider || !submitted || inFlight.current) return;
    inFlight.current = true;
    setBusy("query");
    try {
      // Route the read through the original cache owner. No draft/credential
      // payload reaches the native query command, and no write is retried.
      const answer = await onSubmit({
        provider,
        originalId: provider.id,
        edit: {
          request: submitted.request,
          deleteCredential: false,
          queryOnly: true,
        },
      });
      finishResult(answer, submitted.request, submitted.version);
    } catch {
      finishResult(null, submitted.request, submitted.version);
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const previewRestore = async () => {
    if (
      appId !== "codex" ||
      !review?.value.catalog?.canRestore ||
      mutationsDisabled ||
      editRead?.modeState?.canWrite === false ||
      inFlight.current ||
      isFormSubmitting ||
      submitted ||
      restoreResult
    )
      return;
    const generation = ++restoreGeneration.current;
    const version = draftVersion.current;
    setRestoreReview(null);
    setNotice(null);
    inFlight.current = true;
    setBusy("preview");
    try {
      const id = crypto.randomUUID();
      const value = await providersApi.previewCodexCatalogRestore(id);
      if (
        !alive.current ||
        generation !== restoreGeneration.current ||
        version !== draftVersion.current
      )
        return;
      if (!isCodexCatalogRestorePreview(value, id)) {
        setNotice("catalog.restoreUnavailable");
        return;
      }
      setRestoreReview({ value, version });
    } catch {
      if (alive.current && generation === restoreGeneration.current)
        setNotice("catalog.restoreUnavailable");
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const finishRestore = (
    answer: unknown,
    request: CodexCatalogRestoreRequest,
  ) => {
    if (!alive.current) return;
    setRestoreResult(
      matchesCodexCatalogRestoreResult(answer, request)
        ? answer
        : { app: "codex", request, status: "unknown" },
    );
  };
  const confirmRestore = async () => {
    if (
      !restoreReview ||
      restoreReview.version !== draftVersion.current ||
      mutationsDisabled ||
      editRead?.modeState?.canWrite === false ||
      inFlight.current ||
      submitted ||
      restoreResult
    )
      return;
    const request = restoreReview.value.request;
    inFlight.current = true;
    setBusy("save");
    // Retain this association across draft edits and transport failures. Only
    // this receipt is queried; the provider form never enters the restore IPC.
    setRestoreResult({ app: "codex", request, status: "pending" });
    invalidateRestore();
    setReview(null);
    setCatalogSelection(null);
    nextCatalogDecision.current = undefined;
    setNotice(null);
    try {
      finishRestore(
        await providersApi.confirmCodexCatalogRestore(request),
        request,
      );
    } catch {
      finishRestore(null, request);
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const queryRestore = async () => {
    if (!restoreResult || inFlight.current) return;
    const request = restoreResult.request;
    inFlight.current = true;
    setBusy("query");
    try {
      finishRestore(
        await providersApi.queryCodexCatalogRestore(request),
        request,
      );
    } catch {
      finishRestore(null, request);
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const submitDisabled =
    mutationsDisabled ||
    !initialData ||
    editRead?.modeState?.canWrite === false ||
    isFormSubmitting ||
    !isFormReady ||
    !!busy ||
    !!submitted ||
    !!restoreReview ||
    !!restoreResult;
  const fieldRoles = [
    "connection",
    "authentication",
    "models",
    "dedicated",
    "metadata",
  ];
  const fileRoles = [
    "claudeSettings",
    "codexAuth",
    "codexConfig",
    "codexCatalog",
    "managedAuth",
    "deviceAuthStash",
    "geminiEnv",
    "geminiSettings",
    "grokConfig",
  ];
  const preserveRoles = [
    "unownedJsonKeys",
    "untouchedTomlBytes",
    "untouchedDotenvBytes",
    "sharedSettings",
    "externalCatalog",
  ];
  const statusRoles = [
    "completed",
    "notRecorded",
    "pending",
    "partial",
    "verificationRequired",
    "discarded",
    "abandoned",
    "unknown",
    "conflict",
    "stale",
    "blocked",
  ];
  const codeRoles = [
    "invalidFormat",
    "readOnly",
    "sourceChanged",
    "vaultLocked",
    "pendingOperation",
    "ownerUnavailable",
    "credentialConflict",
    "unsupported",
    "sourceUnavailable",
    "savedPreviousDraft",
    "catalog.restoreUnavailable",
  ];

  if (!provider) {
    return null;
  }

  return (
    <FullScreenPanel
      isOpen={open}
      title={t("provider.editProvider")}
      onClose={handlePanelClose}
      contentClassName={appId === "pi" ? "pb-0" : undefined}
      footer={
        <div className="flex items-center gap-2">
          <Button
            type="submit"
            form="provider-form"
            disabled={submitDisabled}
            onClick={() => {
              intent.current = "preview";
              nextCatalogDecision.current = undefined;
            }}
            variant={review ? "outline" : "default"}
          >
            <Save className="h-4 w-4 mr-2" />
            {t(controlled ? "provider.preview.preview" : "common.save")}
          </Button>
          {controlled && review && (
            <Button
              type="submit"
              form="provider-form"
              disabled={
                submitDisabled ||
                review.value.status !== "ready" ||
                catalogSelection !== null
              }
              onClick={() => {
                intent.current = "confirm";
              }}
            >
              {t(
                review.value.action === "saveOnly"
                  ? "provider.preview.saveOnly"
                  : "provider.preview.saveAndApply",
              )}
            </Button>
          )}
        </div>
      }
    >
      <NoticeSlot>
        {!initialData && (
          <Notice
            tone={readFailed ? "danger" : "neutral"}
            title={t(
              readFailed
                ? "provider.preview.sourceUnavailable"
                : "provider.preview.reading",
            )}
            actions={
              readFailed ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setReadAttempt((value) => value + 1)}
                >
                  {t("provider.preview.reread")}
                </Button>
              ) : undefined
            }
          />
        )}
        {editRead?.modeState?.canWrite === false && (
          <Notice
            tone="warning"
            title={t("provider.preview.pendingOperation")}
          />
        )}
      </NoticeSlot>
      {notice && (
        <NoticeSlot>
          <Notice
            tone="warning"
            title={t(
              `provider.preview.${codeRoles.includes(notice) ? notice : "sourceUnavailable"}`,
            )}
          />
        </NoticeSlot>
      )}
      {result && (
        <NoticeSlot>
          <Notice
            tone={result.status === "completed" ? "neutral" : "warning"}
            title={t(
              `provider.preview.result.${statusRoles.includes(result.status) ? result.status : "unknown"}`,
            )}
            actions={
              <>
                <Button
                  type="button"
                  variant="outline"
                  disabled={!!busy}
                  onClick={() => void queryOriginal()}
                >
                  {t("provider.preview.queryOriginal")}
                </Button>
                {[
                  "completed",
                  "notRecorded",
                  "discarded",
                  "abandoned",
                  "stale",
                  "blocked",
                  "conflict",
                ].includes(result.status) && (
                  <Button
                    type="button"
                    variant="outline"
                    disabled={!!busy}
                    onClick={() => {
                      setResult(null);
                      setSubmitted(null);
                      setReview(null);
                      setNotice(null);
                      setCatalogSelection(null);
                      nextCatalogDecision.current = undefined;
                    }}
                  >
                    {t("provider.preview.backToEdit")}
                  </Button>
                )}
              </>
            }
          />
        </NoticeSlot>
      )}
      {restoreResult && (
        <NoticeSlot>
          <Notice
            tone={restoreResult.status === "completed" ? "neutral" : "warning"}
            title={t(
              restoreResult.status === "completed"
                ? "provider.preview.catalog.restored"
                : `provider.preview.result.${statusRoles.includes(restoreResult.status) ? restoreResult.status : "unknown"}`,
            )}
            actions={
              <>
                <Button
                  type="button"
                  variant="outline"
                  disabled={!!busy}
                  onClick={() => void queryRestore()}
                >
                  {t("provider.preview.queryOriginal")}
                </Button>
                {[
                  "completed",
                  "notRecorded",
                  "discarded",
                  "abandoned",
                  "stale",
                  "blocked",
                  "conflict",
                ].includes(restoreResult.status) && (
                  <Button
                    type="button"
                    variant="outline"
                    disabled={!!busy}
                    onClick={() => setRestoreResult(null)}
                  >
                    {t("provider.preview.backToEdit")}
                  </Button>
                )}
              </>
            }
          >
            {t("provider.preview.catalog.restoreHint")}
          </Notice>
        </NoticeSlot>
      )}
      <ConfirmDialog
        isOpen={!!restoreReview}
        title={t("provider.preview.catalog.restoreTitle")}
        message={
          restoreReview
            ? [
                t("provider.preview.catalog.restoreHint"),
                `${t("provider.preview.catalog.beforePointer")}: ${restoreReview.value.beforePointer ?? t("provider.preview.catalog.noPointer")}`,
                `${t("provider.preview.catalog.afterPointer")}: ${restoreReview.value.afterPointer ?? t("provider.preview.catalog.noPointer")}`,
              ].join("\n\n")
            : ""
        }
        variant="info"
        confirmText={t("provider.preview.catalog.confirmRestore")}
        confirmDisabled={
          mutationsDisabled ||
          editRead?.modeState?.canWrite === false ||
          !!busy ||
          !!submitted ||
          !!restoreResult
        }
        onConfirm={() => void confirmRestore()}
        onCancel={invalidateRestore}
      />
      <div
        className={
          controlled ? "grid items-start gap-6 xl:grid-cols-2" : undefined
        }
      >
        <div
          onSubmitCapture={(event) => {
            if (!controlled) return;
            if (
              capturedSubmit.current ||
              inFlight.current ||
              submitted ||
              restoreReview ||
              restoreResult
            ) {
              event.preventDefault();
              event.stopPropagation();
              return;
            }
            capturedSubmit.current = true;
            invalidateRestore();
            ticket.current = {
              intent: intent.current,
              version: draftVersion.current,
              ...(intent.current === "preview" && nextCatalogDecision.current
                ? { catalogDecision: nextCatalogDecision.current }
                : {}),
            };
            if (intent.current === "preview") {
              // Retire the prior ready result before validation or IPC begins.
              setReview(null);
              setCatalogSelection(null);
            }
            nextCatalogDecision.current = undefined;
            intent.current = "preview";
          }}
        >
          {initialData && (
            <ProviderForm
              appId={appId}
              providerId={provider.id}
              submitLabel={t("common.save")}
              onSubmit={handleSubmit}
              onCancel={closeDialog}
              onManageAuthAccounts={setAuthSettingsTarget}
              onSubmittingChange={handleFormSubmitting}
              onSubmitReadyChange={handleSubmitReadyChange}
              onDraftRevisionChange={
                controlled ? handleDraftRevision : undefined
              }
              preserveBlankCredential={controlled}
              initialData={initialData}
              showButtons={false}
              isProxyTakeover={isProxyTakeover}
            />
          )}
          {controlled && initialData && (
            <label className="mt-4 flex items-start gap-2 text-body">
              <Checkbox
                checked={deleteCredential}
                disabled={
                  !!busy ||
                  !!submitted ||
                  !!restoreReview ||
                  !!restoreResult ||
                  mutationsDisabled
                }
                onCheckedChange={(value) => {
                  setDeleteCredential(value === true);
                  invalidate();
                }}
              />
              <span>
                {t("provider.preview.deleteCredential")}
                <span className="block text-caption text-fg-2">
                  {t("provider.preview.deleteCredentialHint")}
                </span>
              </span>
            </label>
          )}
        </div>
        {controlled && (
          <aside className="rounded-panel border border-border-strong p-4">
            <Tabs defaultValue="fields">
              <TabsList>
                <TabsTrigger value="fields">
                  {t("provider.preview.fields")}
                </TabsTrigger>
                <TabsTrigger value="files">
                  {t("provider.preview.files")}
                </TabsTrigger>
              </TabsList>
              <TabsContent value="fields">
                {!review ? (
                  <p>{t("provider.preview.noPreview")}</p>
                ) : (
                  <ul>
                    {review.value.fields
                      .filter((role) => fieldRoles.includes(role))
                      .map((role) => (
                        <li key={role}>
                          {t(`provider.preview.field.${role}`)}
                        </li>
                      ))}
                  </ul>
                )}
              </TabsContent>
              <TabsContent value="files">
                {!review ? (
                  <p>{t("provider.preview.noPreview")}</p>
                ) : (
                  <ul>
                    {review.value.files
                      .filter((file) => fileRoles.includes(file.role))
                      .map((file) => (
                        <li key={file.role}>
                          {t(`provider.preview.file.${file.role}`)}:{" "}
                          {t(
                            `provider.preview.change.${["write", "delete", "unchanged"].includes(file.change) ? file.change : "unchanged"}`,
                          )}
                        </li>
                      ))}
                  </ul>
                )}
                {review?.value.catalog && (
                  <dl className="mt-3 space-y-2 break-all text-caption">
                    {review.value.catalog.beforePointer !== null && (
                      <div>
                        <dt>{t("provider.preview.catalog.beforePointer")}</dt>
                        <dd>{review.value.catalog.beforePointer}</dd>
                      </div>
                    )}
                    {review.value.catalog.afterPointer !== null && (
                      <div>
                        <dt>{t("provider.preview.catalog.afterPointer")}</dt>
                        <dd>{review.value.catalog.afterPointer}</dd>
                      </div>
                    )}
                  </dl>
                )}
                {review?.value.action === "saveOnly" && (
                  <p>{t("provider.preview.noLiveWrite")}</p>
                )}
              </TabsContent>
            </Tabs>
            {review?.value.catalog?.ownership === "unclaimedExternal" &&
              review.value.catalog.canManage && (
                <CatalogChoiceFields
                  choice={catalogSelection ?? review.value.catalog.choice}
                  disabled={submitDisabled || review.value.status !== "ready"}
                  onChange={(choice) => {
                    invalidateRestore();
                    setCatalogSelection(
                      choice === review.value.catalog?.choice ? null : choice,
                    );
                  }}
                  onPreviewChoice={(choice) => {
                    intent.current = "preview";
                    nextCatalogDecision.current = {
                      choice,
                      revision: review.value.catalog!.revision,
                    };
                  }}
                  onCancel={invalidate}
                />
              )}
            {appId === "codex" &&
              review?.value.catalog?.canRestore === true && (
                <div className="mt-4 space-y-2">
                  <Button
                    type="button"
                    variant="outline"
                    disabled={submitDisabled}
                    onClick={() => void previewRestore()}
                  >
                    {t("provider.preview.catalog.restore")}
                  </Button>
                  <p className="text-caption text-fg-2">
                    {t("provider.preview.catalog.restoreHint")}
                  </p>
                </div>
              )}
            {review && (
              <p className="mt-3 text-caption text-fg-2">
                {t("provider.preview.plannedOnly")}
              </p>
            )}
            {review?.value.preserves
              .filter((role) => preserveRoles.includes(role))
              .map((role) => (
                <p className="text-caption text-fg-2" key={role}>
                  {t(`provider.preview.preserve.${role}`)}
                </p>
              ))}
            {review?.value.status === "blocked" && (
              <Notice
                tone="warning"
                title={t(
                  `provider.preview.${codeRoles.includes(review.value.code ?? "") ? review.value.code : "sourceUnavailable"}`,
                )}
              />
            )}
          </aside>
        )}
      </div>
      <AuthSettingsPanel
        target={authSettingsTarget}
        onClose={() => setAuthSettingsTarget(null)}
      />
    </FullScreenPanel>
  );
}
