import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  accountErrorText,
  safeAccountError,
  zcodeAccountsApi,
  type BundlePreview,
  type BundleCheckProgress,
  type AccountError,
} from "@/lib/api/zcodeAccounts";
type ImportProps = {
  open?: boolean;
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
export function useZCodeBundleImport({
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
  const [originalRequest, setOriginalRequest] = useState<string | null>(() => {
    try {
      return zcodeAccountsApi.pendingBundleImport(libraryDataRoot);
    } catch {
      return null;
    } // The pre-dispatch storage guard still fails closed.
  });
  const [recoveryUnknown, setRecoveryUnknown] = useState(false);
  const [notRecorded, setNotRecorded] = useState(false);
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
  const reviewRows = (review?.preview.rows ?? []).map((row) => ({
    row,
    checked: checkProgress?.rows.find((entry) => entry.index === row.index),
    selected: selected.has(row.index),
    updateDuplicate: updates.has(row.index),
    selectionDisabled: selectionLocked || !!row.error,
    updateDisabled: selectionLocked || !selected.has(row.index),
  }));
  const consentDisabled = selectionLocked || choices.length === 0;
  const canCheck =
    !!review &&
    !originalRequest &&
    choices.length > 0 &&
    consent &&
    !busy &&
    !checking &&
    !submitted;
  const canSave =
    !!review &&
    !originalRequest &&
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
      originalRequest ||
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
    setNotRecorded(false);
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
    setOriginalRequest(review.preview.previewId);
    setRecoveryUnknown(false);
    try {
      const outcomes = await zcodeAccountsApi.importBundle(
        libraryDataRoot,
        review.revision,
        review.preview.previewId,
        choices,
        review.preview.ownerBinding,
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
      setOriginalRequest(null);
      setResultRows(
        rows.map((row, index) => ({ row, outcome: outcomes[index] })),
      );
      zcodeAccountsApi.acknowledgeBundleImport(
        libraryDataRoot,
        review.preview.previewId,
      );
      try {
        await onImported();
      } catch {
        if (current(version)) setRefreshFailed(true);
      }
    } catch (cause) {
      if (current(version)) {
        // Another window may have retained a different request before dispatch.
        // Follow that existing pointer; absence/read failure never proves unsent.
        try {
          const pending = zcodeAccountsApi.pendingBundleImport(libraryDataRoot);
          if (pending) setOriginalRequest(pending);
        } catch {
          // Keep the current unresolved ID and the original safe storage error.
        }
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
  const queryOriginal = async () => {
    if (!alive.current || !originalRequest || inFlight.current) return;
    const version = generation.current;
    inFlight.current = true;
    setBusy("read");
    setError(null);
    setRecoveryUnknown(false);
    try {
      const result = await zcodeAccountsApi.bundleImportResult(
        libraryDataRoot,
        originalRequest,
      );
      if (!current(version)) return;
      if (!result || result.status === "unknown") {
        setRecoveryUnknown(true);
        return;
      }
      if (result.status === "notRecorded") {
        zcodeAccountsApi.acknowledgeBundleImport(
          libraryDataRoot,
          originalRequest,
        );
        setOriginalRequest(null);
        setNotRecorded(true);
        setSubmitted(false);
        didSubmit.current = false;
        lease.current = null;
        setReview(null);
        setResultRows([]);
        clearFile();
        invalidateCheck();
        setSelected(new Set());
        setUpdates(new Set());
        try {
          await onImported();
        } catch {
          if (current(version))
            fail({
              code: "zcode.account.catalog_changed",
              remedy: "inspectAgain",
              committed: false,
            });
        }
        return;
      }
      didSubmit.current = true;
      setSubmitted(true);
      setOriginalRequest(null);
      setResultRows(
        result.rows.map(({ index, id, family, outcome }) => ({
          row: {
            index,
            id,
            family,
            label: null,
            duplicate: false,
            ambiguous: false,
            error: null,
          },
          outcome,
        })),
      );
      zcodeAccountsApi.acknowledgeBundleImport(
        libraryDataRoot,
        originalRequest,
      );
      try {
        await onImported();
      } catch {
        if (current(version)) setRefreshFailed(true);
      }
    } catch (cause) {
      if (current(version)) fail(cause);
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
  // A partial or unknown outcome must never produce a completion summary.
  // Derive it from the same receipt rows used by both commit and recovery.
  const resultSummary =
    resultRows.length > 0 &&
    resultRows.every(({ outcome }) => outcome !== "unknown")
      ? {
          saved: resultRows.filter(({ outcome }) => outcome === "saved").length,
          updated: resultRows.filter(({ outcome }) => outcome === "refreshed")
            .length,
          kept: resultRows.filter(({ outcome }) => outcome === "kept").length,
        }
      : null;
  const chooseBundle = (next: File | null) => {
    setFile(next);
    setError(null);
  };
  const selectRow = (row: Row, checked: boolean) => {
    if (!review) return;
    setSelected((previous) => {
      const next = new Set(previous);
      if (checked) {
        for (const peer of review.preview.rows)
          if (peer.id === row.id) next.delete(peer.index);
        next.add(row.index);
      } else next.delete(row.index);
      return next;
    });
    invalidateCheck();
  };
  const updateRow = (index: number, checked: boolean) => {
    setUpdates((previous) => {
      const next = new Set(previous);
      if (checked) next.add(index);
      else next.delete(index);
      return next;
    });
    invalidateCheck();
  };
  return {
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
    selected,
    updates,
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
    choices,
    selectionLocked,
    canCheck,
    canSave,
    preview,
    check,
    queryCheck,
    commit,
    queryOriginal,
    errorText,
  };
}
