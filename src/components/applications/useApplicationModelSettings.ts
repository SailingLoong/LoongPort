import { useCallback, useEffect, useRef, useState } from "react";
import type { AppId } from "@/lib/api";
import {
  applicationModelSettingsApi,
  type ApplicationModelSettings,
  type ApplicationModelSettingsPatch,
  completedApplicationModelSettingsEdit,
  isApplicationModelSettingsRequest,
  matchesApplicationModelSettingsResult,
  type ApplicationModelSettingsPreview,
  type ApplicationModelSettingsRequest,
  type ApplicationModelSettingsResult,
} from "@/lib/api/applicationModelSettings";

// Native commands return public codes. Never surface raw exception text.
const nativeErrorLabels: Record<string, string> = {
  capabilityConflict: "capabilityConflict",
  sourceConfirmationRequired: "sourceConfirmationRequired",
  unsupportedReasoning: "invalidFormat",
  templateProtocolMismatch: "invalidFormat",
  unknownTemplate: "invalidFormat",
  invalidConfig: "invalidFormat",
  invalidCatalog: "invalidFormat",
  invalidDeclaration: "invalidFormat",
  invalidModel: "invalidFormat",
  invalidRow: "invalidFormat",
  duplicateModel: "invalidFormat",
  modelRequired: "invalidFormat",
  declarationModelMismatch: "invalidFormat",
  restoreUnavailable: "restoreUnavailable",
  sourceChanged: "sourceChanged",
  providerMissing: "sourceChanged",
  sourceUnavailable: "ownerUnavailable",
  ownerUnavailable: "ownerUnavailable",
  vaultLocked: "vaultLocked",
  credentialConflict: "credentialConflict",
  pendingOperation: "pendingOperation",
  readOnly: "readOnly",
  requestMismatch: "blocked",
  outsideFieldScope: "blocked",
  unsupportedApplication: "blocked",
  unsupported: "invalidFormat",
  invalidFormat: "invalidFormat",
};
const nativeErrorLabel = (error: unknown, fallback: string) => {
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : "";
  const prefix = "provider.modelSettings.";
  const code = message.startsWith(prefix) ? message.slice(prefix.length) : "";
  return Object.hasOwn(nativeErrorLabels, code)
    ? nativeErrorLabels[code]
    : fallback;
};

/** The original provider owner holds the durable request and write lifecycle. */
export function useApplicationModelSettings(
  app: AppId,
  providerId: string,
  open: boolean,
  onSaved: () => void | Promise<void>,
) {
  const [data, setData] = useState<ApplicationModelSettings | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [previewConflicts, setPreviewConflicts] = useState<string[]>([]);
  const [review, setReview] = useState<{
    preview: ApplicationModelSettingsPreview;
    patch: ApplicationModelSettingsPatch;
  } | null>(null);
  const [result, setResult] = useState<ApplicationModelSettingsResult | null>(
    null,
  );
  const scope = `${app}:${providerId}`;
  const activeScope = useRef(scope);
  activeScope.current = scope;
  const inFlight = useRef(false);
  const loadSequence = useRef(0);
  const completedRequest = useRef<string | null>(null);
  useEffect(() => {
    activeScope.current = scope;
    return () => {
      if (activeScope.current === scope) activeScope.current = "";
    };
  }, [scope]);

  const reload = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    setError(null);
    try {
      const next = await applicationModelSettingsApi.get(app, providerId);
      if (sequence !== loadSequence.current || activeScope.current !== scope)
        return;
      if (next.app !== app || next.providerId !== providerId)
        throw new Error("scopeMismatch");
      const original = next.originalSave;
      if (
        original &&
        (original.app !== app ||
          !isApplicationModelSettingsRequest(original.request) ||
          original.request.providerId !== providerId)
      )
        throw new Error("scopeMismatch");
      setData(next);
      if (
        original &&
        original.status !== "completed" &&
        original.app === app &&
        isApplicationModelSettingsRequest(original.request) &&
        original.request.providerId === providerId
      ) {
        setResult(original);
      }
    } catch (error) {
      if (sequence === loadSequence.current && activeScope.current === scope)
        setError(nativeErrorLabel(error, "loadFailed"));
    } finally {
      if (sequence === loadSequence.current && activeScope.current === scope)
        setLoading(false);
    }
  }, [app, providerId, scope]);

  useEffect(() => {
    setData(null);
    setReview(null);
    setResult(null);
    setError(null);
    setPreviewConflicts([]);
    completedRequest.current = null;
    if (open) void reload();
    return () => {
      ++loadSequence.current;
    };
  }, [open, reload]);

  const prepare = async (patch: ApplicationModelSettingsPatch) => {
    if (inFlight.current || !data?.editable || data.manualConfig || result)
      return;
    if (
      data.sourceChanged &&
      (!data.sourceChangeEffects ||
        !patch.confirmSourceChange ||
        patch.sourceChangeRevision !== data.revision ||
        patch.model?.action !== "set" ||
        patch.reasoning?.action !== "set")
    )
      return;
    if (
      !data.sourceChanged &&
      (patch.confirmSourceChange || patch.sourceChangeRevision !== undefined)
    )
      return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    setPreviewConflicts([]);
    const frozen = structuredClone(patch);
    const requestId = crypto.randomUUID();
    try {
      const preview = await applicationModelSettingsApi.preview(
        app,
        providerId,
        frozen,
        requestId,
      );
      if (activeScope.current !== scope) return;
      if (
        preview.app !== app ||
        !isApplicationModelSettingsRequest(preview.request) ||
        preview.request.id !== requestId ||
        preview.request.providerId !== providerId
      )
        throw new Error("scopeMismatch");
      if (preview.status !== "ready" || preview.conflicts?.length) {
        setPreviewConflicts(preview.conflicts ?? []);
        setError(
          preview.code
            ? (nativeErrorLabels[preview.code] ?? "blocked")
            : "blocked",
        );
        return;
      }
      // Catalog takeover/import stays with the existing advanced editor.
      if (preview.catalog && preview.catalog.choice !== "keep") {
        setError("manualConfig");
        return;
      }
      setReview({ preview, patch: frozen });
    } catch (error) {
      if (activeScope.current === scope)
        setError(nativeErrorLabel(error, "previewFailed"));
    } finally {
      inFlight.current = false;
      if (activeScope.current === scope) setBusy(false);
    }
  };

  const acceptResult = async (
    next: ApplicationModelSettingsResult,
    request: ApplicationModelSettingsRequest,
  ) => {
    if (activeScope.current !== scope) return;
    const verified = matchesApplicationModelSettingsResult(next, app, request)
      ? next
      : { app, request, status: "unknown" as const };
    setResult(verified);
    if (
      completedApplicationModelSettingsEdit(verified, app, request) &&
      completedRequest.current !== request.id
    ) {
      completedRequest.current = request.id;
      // A cache-refresh failure must not turn a completed native write into a retry.
      try {
        await onSaved();
      } catch {
        /* Original outcome stays completed. */
      }
    }
  };

  const finish = async (queryOnly: boolean) => {
    if (
      inFlight.current ||
      (!review && !result) ||
      (!queryOnly && (!review || result))
    )
      return;
    const current = review;
    const request = current?.preview.request ?? result!.request;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    let next: ApplicationModelSettingsResult;
    try {
      next = queryOnly
        ? await applicationModelSettingsApi.query(app, request)
        : await applicationModelSettingsApi.confirm(
            app,
            providerId,
            current!.patch,
            request,
          );
    } catch {
      next = { app, request, status: "unknown" };
    }
    await acceptResult(next, request);
    inFlight.current = false;
    if (activeScope.current === scope) setBusy(false);
  };

  return {
    data,
    loading,
    busy,
    error,
    previewConflicts,
    review,
    result,
    reload,
    prepare,
    confirm: () => finish(false),
    recheck: () => finish(true),
    backToEdit: () => {
      if (
        !inFlight.current &&
        result &&
        [
          "completed",
          "notRecorded",
          "discarded",
          "abandoned",
          "stale",
          "blocked",
          "conflict",
        ].includes(result.status)
      ) {
        setResult(null);
        setReview(null);
        setError(null);
        setPreviewConflicts([]);
      }
    },
    canReturnToEdit: Boolean(
      result &&
      [
        "completed",
        "notRecorded",
        "discarded",
        "abandoned",
        "stale",
        "blocked",
        "conflict",
      ].includes(result.status),
    ),
    cancelReview: () => {
      if (!inFlight.current && !result) setReview(null);
    },
    unresolved: Boolean(
      result &&
      ["pending", "partial", "verificationRequired", "unknown"].includes(
        result.status,
      ),
    ),
  };
}
