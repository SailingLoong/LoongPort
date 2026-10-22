import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
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
export function MiniMaxNativeImport({
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
  const id = useId();
  const alive = useRef(false);
  const generation = useRef(0);
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
    setPath("");
    setReadConsent(false);
    setVerifyConsent(false);
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
      invalidate();
    };
  }, [invalidate]);
  useEffect(() => {
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
  const changeOpen = (value: boolean) => {
    clear();
    setVisible(value && !disabled);
  };
  const choose = async () => {
    if (disabled || inFlight.current) return;
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
    if (disabled || inFlight.current || !visible) return;
    if (step === "read" && (!path || !readConsent)) return;
    if (step !== "read" && (!preview || !verifyConsent)) return;
    if (
      step === "save" &&
      (!saveConsent || preview?.stage !== "verified" || !preview.canSave)
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
      setVerifyConsent(false);
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
          setVerifyConsent(false);
          setSaveConsent(false);
          setFailed(true);
          return;
        }
        activePreview.current = result.previewId;
        setPreview(result);
        setSaveConsent(false);
      }
    } catch {
      if (current()) setFailed(true);
    } finally {
      if (current()) {
        inFlight.current = false;
        setBusy(false);
      }
    }
  };
  const canVerify = preview?.stage === "preview" && !expired;
  const canSave = preview?.stage === "verified" && preview.canSave && !expired;
  return (
    <>
      <Button
        type="button"
        variant="outline"
        disabled={disabled}
        onClick={() => changeOpen(true)}
      >
        {copy("open")}
      </Button>
      <Dialog open={visible} onOpenChange={changeOpen}>
        <DialogContent className="max-h-[90vh] overflow-y-auto">
          <DialogTitle>{copy("title")}</DialogTitle>
          <DialogDescription>{copy("description")}</DialogDescription>
          <p className="text-sm text-muted-foreground">
            {t("minimaxCode.region")}: {t(`minimaxCode.regions.${region}`)}
          </p>
          <Button
            type="button"
            variant="outline"
            disabled={busy || disabled}
            onClick={() => void choose()}
          >
            {copy("choose")}
          </Button>
          <Input readOnly aria-label={copy("source")} value={path} />
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-read`}
              checked={readConsent}
              disabled={!path || busy || disabled}
              onCheckedChange={(value) => {
                invalidate();
                setReadConsent(value === true);
                setPreview(null);
                setVerifyConsent(false);
                setSaveConsent(false);
                setFailed(false);
              }}
            />
            <Label htmlFor={`${id}-read`}>{copy("readConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!path || !readConsent || busy || disabled}
            onClick={() => void perform("read")}
          >
            {copy("read")}
          </Button>
          {preview && (
            <div role="status" className="space-y-1 text-sm">
              {preview.maskedSubject !== null && (
                <p>
                  {copy("unverified")}: <span>{preview.maskedSubject}</span>
                </p>
              )}
              <p>
                {copy("expires")}:{" "}
                {new Date(preview.expiresAtMs).toLocaleString()}
              </p>
              {preview.stage === "verified" && (
                <>
                  <p>
                    {copy("verified")}: <span>{preview.verifiedUserId}</span>
                  </p>
                  {preview.label !== null && <p>{preview.label}</p>}
                </>
              )}
            </div>
          )}
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-verify`}
              checked={verifyConsent}
              disabled={!canVerify || busy || disabled}
              onCheckedChange={(value) => setVerifyConsent(value === true)}
            />
            <Label htmlFor={`${id}-verify`}>{copy("verifyConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!canVerify || !verifyConsent || busy || disabled}
            onClick={() => void perform("verify")}
          >
            {copy("verify")}
          </Button>
          <p className="text-sm text-muted-foreground">{copy("accessOnly")}</p>
          {expired && (
            <p role="alert" className="text-sm text-destructive">
              {copy("expired")}
            </p>
          )}
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-save`}
              checked={saveConsent}
              disabled={!canSave || busy || disabled}
              onCheckedChange={(value) => setSaveConsent(value === true)}
            />
            <Label htmlFor={`${id}-save`}>{copy("saveConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!canSave || !saveConsent || busy || disabled}
            onClick={() => void perform("save")}
          >
            {copy("save")}
          </Button>
          {failed && (
            <p role="alert" className="text-sm text-destructive">
              {copy("error")}
            </p>
          )}
          <Button
            type="button"
            variant="ghost"
            onClick={() => changeOpen(false)}
          >
            {copy("cancel")}
          </Button>
        </DialogContent>
      </Dialog>
    </>
  );
}
