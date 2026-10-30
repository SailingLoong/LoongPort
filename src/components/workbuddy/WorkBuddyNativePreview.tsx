import { useId, useMemo, useRef, useState } from "react";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  useWorkBuddyNativePreviewViewModel,
  useWorkBuddyNativeCaptureViewModel,
} from "./useWorkBuddyNativePreviewViewModel";

export function WorkBuddyNativePreview({
  onCaptured,
}: { onCaptured?: () => void } = {}) {
  const { t } = useTranslation();
  const { result, error, busy, inspect } = useWorkBuddyNativePreviewViewModel();
  const [captureOpen, setCaptureOpen] = useState(false);
  const captureOpenRef = useRef(false);
  const captureSession = useRef(0);
  const session = captureSession.current;
  const visible = usePreservedViewActive();
  const owner = useMemo(() => Symbol("native-capture-dialog-owner"), [visible]);
  const currentOwner = useRef(owner);
  currentOwner.current = owner;
  const openCapture = () => {
    if (!visible || currentOwner.current !== owner || captureOpenRef.current)
      return;
    captureOpenRef.current = true;
    ++captureSession.current;
    setCaptureOpen(true);
  };
  const closeCapture = () => {
    if (
      !visible ||
      currentOwner.current !== owner ||
      captureSession.current !== session
    )
      return false;
    captureOpenRef.current = false;
    ++captureSession.current;
    setCaptureOpen(false);
    return true;
  };
  return (
    <section
      aria-label={t("workbuddy.nativePreview.title")}
      className="space-y-2 rounded-xl border border-border bg-card p-3 text-xs"
    >
      <h3 className="text-sm font-medium">
        {t("workbuddy.nativePreview.title")}
      </h3>
      <p className="text-muted-foreground">
        {t("workbuddy.nativePreview.description")}
      </p>
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => void inspect()}
      >
        {t(
          busy
            ? "workbuddy.nativePreview.inspecting"
            : "workbuddy.nativePreview.inspect",
        )}
      </Button>
      {error && (
        <p role="alert">{t(`workbuddy.nativePreview.error.${error}`)}</p>
      )}
      {result && (
        <div role="status" className="space-y-1">
          <p>
            {t(
              result.authFile === "missing"
                ? "workbuddy.nativePreview.missing"
                : "workbuddy.nativePreview.recognized",
            )}
          </p>
          {result.realm && (
            <p>
              {t(
                result.realm === "www.workbuddy.cn"
                  ? "workbuddy.nativePreview.realmWorkbuddy"
                  : "workbuddy.nativePreview.realmCodebuddy",
              )}
            </p>
          )}
          <p>
            {t(
              result.logoutMarkerPresent
                ? "workbuddy.nativePreview.markerPresent"
                : "workbuddy.nativePreview.markerAbsent",
            )}
          </p>
        </div>
      )}
      <p className="text-muted-foreground">
        {t("workbuddy.nativePreview.notActivation")}
      </p>
      {!captureOpen && (
        <Button size="sm" variant="outline" onClick={openCapture}>
          {t("workbuddy.nativeCapture.open", {
            defaultValue: "Save current client account",
          })}
        </Button>
      )}
      {captureOpen && (
        <NativeCaptureConfirmation
          onClose={closeCapture}
          onCaptured={onCaptured}
        />
      )}
    </section>
  );
}

function NativeCaptureConfirmation({
  onClose,
  onCaptured,
}: {
  onClose: () => boolean;
  onCaptured?: () => void;
}) {
  const { t } = useTranslation();
  const vm = useWorkBuddyNativeCaptureViewModel(onCaptured);
  const labelId = useId();
  return (
    <div
      role="group"
      aria-label={t("workbuddy.nativeCapture.title", {
        defaultValue: "Save current client account",
      })}
      className="space-y-2 rounded-lg border p-3"
    >
      <p>
        {t("workbuddy.nativeCapture.description", {
          defaultValue:
            "Confirm saving the current client's local authentication snapshot to the LoongPort vault. This does not verify login or grant API access.",
        })}
      </p>
      <p>
        {t("workbuddy.nativeCapture.existing", {
          defaultValue:
            "If this account already exists, this action cannot add a native snapshot to it. Existing accounts are not replaced or deleted.",
        })}
      </p>
      <Label htmlFor={labelId}>
        {t("workbuddy.nativeCapture.label", {
          defaultValue: "Account label (no credentials)",
        })}
      </Label>
      <Input
        id={labelId}
        value={vm.label}
        onChange={(event) => vm.setLabel(event.target.value)}
        maxLength={120}
        disabled={vm.locked}
      />
      <div className="flex gap-2">
        <Button
          size="sm"
          disabled={vm.locked || !vm.label.trim()}
          onClick={vm.confirm}
        >
          {t("workbuddy.nativeCapture.confirm", {
            defaultValue: "Confirm and save",
          })}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            if (onClose()) vm.retire();
          }}
        >
          {vm.locked
            ? t("workbuddy.nativeCapture.close", { defaultValue: "Close" })
            : t("workbuddy.nativeCapture.cancel", { defaultValue: "Cancel" })}
        </Button>
      </div>
      {vm.status === "pending" && (
        <p role="status">
          {t("workbuddy.nativeCapture.busy", {
            defaultValue:
              "Saving; the operation may continue if you leave this view.",
          })}
        </p>
      )}
      {vm.conflict && (
        <p>
          {t("workbuddy.nativeCapture.conflict", {
            defaultValue:
              "The source conflicts with an existing account or changed state. Existing accounts are not replaced or deleted.",
          })}
        </p>
      )}
      {vm.status === "error" && (
        <p role="alert">
          {t("workbuddy.nativeCapture.unknown", {
            defaultValue:
              "The result is unconfirmed. Check the account list; a list alone cannot prove this request failed. Do not repeat it automatically.",
          })}
        </p>
      )}
      {vm.status === "success" && (
        <p role="status">
          {t("workbuddy.nativeCapture.saved", {
            defaultValue:
              "The most recent local snapshot was saved; login and API access are not verified.",
          })}
        </p>
      )}
    </div>
  );
}
