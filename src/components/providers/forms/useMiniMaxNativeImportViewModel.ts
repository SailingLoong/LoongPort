import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import {
  minimaxCodeApi,
  type MiniMaxCodeNativePreview,
  type MiniMaxCodeRegion,
} from "@/lib/api/minimaxCode";

function discard(previewId: string | undefined) {
  if (typeof previewId === "string" && previewId)
    void minimaxCodeApi.cancelNativeImport(previewId).catch(() => {});
}

/** Each permission is independent. Only the backend holds imported credentials. */
export function useMiniMaxNativeImportViewModel({
  region,
  disabled = false,
  onSaved,
}: {
  region: MiniMaxCodeRegion;
  disabled?: boolean;
  onSaved?: () => void;
}) {
  const { t } = useTranslation();
  const copy = (key: string) => t(`minimaxCode.nativeImport.${key}`);
  const [visible, setVisible] = useState(false);
  const [path, setPath] = useState("");
  const [readConsent, setReadConsent] = useState(false);
  const [verifyConsent, setVerifyConsent] = useState(false);
  const [saveConsent, setSaveConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [expired, setExpired] = useState(false);
  const [preview, setPreview] = useState<MiniMaxCodeNativePreview | null>(null);
  const alive = useRef(false);
  const dialogActive = useRef(false);
  const permissions = useRef({ read: false, verify: false, save: false });
  const generation = useRef(0);
  const commandGeneration = generation.current;
  const inFlight = useRef(false);
  const activePreview = useRef<string | null>(null);
  const invalidate = useCallback(() => {
    generation.current += 1;
    inFlight.current = false;
    discard(activePreview.current ?? undefined);
    activePreview.current = null;
  }, []);
  const clear = useCallback(() => {
    invalidate();
    permissions.current.read = false;
    setPath("");
    setReadConsent(false);
    permissions.current.verify = false;
    setVerifyConsent(false);
    permissions.current.save = false;
    setSaveConsent(false);
    setBusy(false);
    setFailed(false);
    setExpired(false);
    setPreview(null);
  }, [invalidate]);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      dialogActive.current = false;
      invalidate();
    };
  }, [invalidate]);
  useEffect(() => {
    dialogActive.current = false;
    clear();
    setVisible(false);
  }, [region, disabled, clear]);
  useEffect(() => {
    if (!preview) return;
    let timer: ReturnType<typeof setTimeout>;
    const update = () => {
      const remaining = preview.expiresAtMs - Date.now();
      setExpired(remaining <= 0);
      if (remaining > 0)
        timer = setTimeout(update, Math.min(remaining, 2147483647));
    };
    update();
    return () => clearTimeout(timer);
  }, [preview]);
  const commandIsCurrent = () =>
    alive.current &&
    dialogActive.current &&
    commandGeneration === generation.current &&
    !disabled &&
    !inFlight.current;
  const changeOpen = (value: boolean) => {
    if (!alive.current) return;
    dialogActive.current = value && !disabled && alive.current;
    clear();
    setVisible(value && !disabled);
  };
  const choose = async () => {
    if (!commandIsCurrent()) return;
    clear();
    const request = ++generation.current;
    inFlight.current = true;
    setBusy(true);
    const current = () => alive.current && generation.current === request;
    try {
      // Choosing a source does not read it or grant any later permission.
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: copy("fileFilter"), extensions: ["json"] }],
      });
      if (current() && typeof selected === "string") setPath(selected);
    } catch {
      if (current()) setFailed(true);
    } finally {
      if (current()) {
        inFlight.current = false;
        setBusy(false);
      }
    }
  };
  const valid = (
    result: MiniMaxCodeNativePreview,
    stage: "preview" | "verified",
  ) =>
    typeof result.previewId === "string" &&
    result.previewId.length > 0 &&
    result.source === "publicCliOAuthV1" &&
    result.region === region &&
    result.stage === stage &&
    (result.maskedSubject === null ||
      (typeof result.maskedSubject === "string" &&
        result.maskedSubject.length > 0 &&
        result.maskedSubject.length <= 16)) &&
    Number.isSafeInteger(result.expiresAtMs) &&
    result.expiresAtMs > 0 &&
    typeof result.canSave === "boolean" &&
    (stage === "preview"
      ? result.canSave === false && result.verifiedUserId === null
      : typeof result.verifiedUserId === "string" &&
        result.verifiedUserId.length > 0);
  const perform = async (step: "read" | "verify" | "save") => {
    if (!commandIsCurrent()) return;
    if (step === "read" && (!path || !readConsent || !permissions.current.read))
      return;
    if (
      step !== "read" &&
      (!preview ||
        !verifyConsent ||
        !permissions.current.verify ||
        activePreview.current !== preview.previewId)
    )
      return;
    if (
      step === "save" &&
      (!saveConsent ||
        !permissions.current.save ||
        preview?.stage !== "verified" ||
        !preview.canSave)
    )
      return;
    if (step !== "read" && preview && preview.expiresAtMs <= Date.now()) {
      setExpired(true);
      return;
    }
    const previous = preview;
    if (step === "read") {
      invalidate();
      setPreview(null);
      permissions.current.verify = false;
      setVerifyConsent(false);
      permissions.current.save = false;
      setSaveConsent(false);
    }
    const request = ++generation.current;
    inFlight.current = true;
    setBusy(true);
    setFailed(false);
    const current = () => alive.current && generation.current === request;
    try {
      if (step === "save") {
        await minimaxCodeApi.saveNativeImport(previous!.previewId, true);
        if (!current()) return;
        // Saving consumes this preview. Refresh the authoritative catalog rather
        // than publishing a command response that may race a reauthorization.
        activePreview.current = null;
        dialogActive.current = false;
        clear();
        setVisible(false);
        onSaved?.();
      } else {
        const result =
          step === "read"
            ? await minimaxCodeApi.previewNativeFile(path, region, true)
            : await minimaxCodeApi.verifyNativeImport(
                previous!.previewId,
                true,
              );
        if (!current()) {
          discard(result.previewId);
          return;
        }
        if (
          !valid(result, step === "read" ? "preview" : "verified") ||
          (step === "verify" &&
            (result.previewId !== previous!.previewId ||
              result.expiresAtMs !== previous!.expiresAtMs))
        ) {
          discard(result.previewId);
          if (activePreview.current !== result.previewId)
            discard(activePreview.current ?? undefined);
          activePreview.current = null;
          setPreview(null);
          permissions.current.verify = false;
          setVerifyConsent(false);
          permissions.current.save = false;
          setSaveConsent(false);
          setFailed(true);
          return;
        }
        activePreview.current = result.previewId;
        setPreview(result);
        permissions.current.save = false;
        setSaveConsent(false);
      }
    } catch {
      if (current()) {
        if (step === "save") {
          // A lost save reply is not permission to replay the consumed ticket.
          // Clear local material and reread the catalog without claiming success.
          clear();
          setFailed(true);
          onSaved?.();
        } else setFailed(true);
      }
    } finally {
      if (current()) {
        inFlight.current = false;
        setBusy(false);
      }
    }
  };
  const canVerify = preview?.stage === "preview" && !expired;
  const canSave = preview?.stage === "verified" && preview.canSave && !expired;
  const changeReadConsent = (value: boolean) => {
    if (!commandIsCurrent()) return;
    permissions.current.read = value;
    invalidate();
    setReadConsent(value);
    setPreview(null);
    permissions.current.verify = false;
    setVerifyConsent(false);
    permissions.current.save = false;
    setSaveConsent(false);
    setFailed(false);
  };
  const changeVerifyConsent = (value: boolean) => {
    if (!commandIsCurrent()) return;
    permissions.current.verify = value;
    setVerifyConsent(value);
  };
  const changeSaveConsent = (value: boolean) => {
    if (!commandIsCurrent()) return;
    permissions.current.save = value;
    setSaveConsent(value);
  };
  return {
    visible,
    path,
    readConsent,
    verifyConsent,
    saveConsent,
    busy,
    failed,
    expired,
    preview,
    canVerify,
    canSave,
    changeOpen,
    choose,
    perform,
    changeReadConsent,
    changeVerifyConsent,
    changeSaveConsent,
  };
}
