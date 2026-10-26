import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { LiteralKeyInput, type LiteralKeyInputHandle } from "./LiteralKeyInput";
import { Label } from "@/components/ui/label";
import { CredentialKeyRecoveryHelp } from "./CredentialKeyRecoveryHelp";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import {
  credentialProtectionApi,
  credentialProtectionKey,
  type CredentialProtectionMode,
  type CredentialProtectionStatus,
} from "@/lib/api/credentialProtection";

// Missing input is incomplete; supplied text has no format or strength rules.
// Never trim, case-fold or normalize the user’s text before comparison or IPC.
const hasKeyText = (value: string) => value.length > 0;

/** Local, voluntary controls: never a startup gate or an owner of runtime readiness. */
export function CredentialProtectionPanel({
  setupPrompt = false,
}: {
  setupPrompt?: boolean;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const headingId = useId();
  const protection = useQuery({
    queryKey: credentialProtectionKey,
    queryFn: credentialProtectionApi.status,
    retry: false,
  });
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const [saved, setSaved] = useState(false);

  // No useMutation here: its variables/error cache must never retain a user key.
  const perform = async (action: () => Promise<CredentialProtectionStatus>) => {
    setPending(true);
    setFailed(false);
    setSaved(false);
    try {
      const result = await action();
      queryClient.setQueryData(credentialProtectionKey, result);
      setSaved(result.status === "available" && !result.restartRequired);
      setFailed(result.status !== "available" && !result.restartRequired);
      // Only the runtime's exact authority may enable credential consumers.
      // A restart-required result does not imply a ready in-process runtime.
      await queryClient.invalidateQueries({
        queryKey: ["runtimeAvailability"],
      });
    } catch {
      // Native error text can contain user input. Never echo, log or cache it.
      setFailed(true);
      await queryClient.invalidateQueries({
        queryKey: credentialProtectionKey,
      });
    } finally {
      setPending(false);
    }
  };

  return (
    <section
      className="space-y-4 rounded-lg border p-5"
      aria-labelledby={headingId}
    >
      <div className="flex items-start gap-3">
        <ShieldCheck
          aria-hidden="true"
          className="mt-0.5 h-5 w-5 shrink-0 text-primary"
        />
        <div className="space-y-1">
          <h3 id={headingId} className="font-medium">
            {t("storageProtection.title")}
          </h3>
          <p className="text-sm text-muted-foreground">
            {t("storageProtection.description")}
          </p>
        </div>
      </div>
      {protection.isError ? (
        <div className="space-y-3">
          <p role="alert" className="text-sm text-destructive">
            {t("storageProtection.statusFailed")}
          </p>
          <Button
            variant="outline"
            onClick={() => void protection.refetch()}
            disabled={protection.isFetching}
          >
            {t("common.retry")}
          </Button>
        </div>
      ) : protection.data ? (
        <>
          <p className="text-sm text-muted-foreground">
            {t(`storageProtection.mode.${protection.data.mode}`)}
          </p>
          <p className="text-sm">
            {t(`storageProtection.status.${protection.data.status}`)}
          </p>
          {protection.data.mode === "encrypted" && (
            <CredentialKeyRecoveryHelp />
          )}
          {protection.data.status === "available" &&
            protection.data.errorCode ===
              "protection.key_cache_unavailable" && (
              <Notice
                tone="warning"
                title={t("storageProtection.keyCacheUnavailable")}
              />
            )}
          {protection.data.restartRequired ? (
            <Notice
              tone="neutral"
              title={t("storageProtection.restartRequired")}
            />
          ) : (
            <ProtectionActions
              key={`${protection.data.mode}:${protection.data.status}:${protection.data.reviewToken}`}
              status={protection.data}
              pending={pending}
              setupPrompt={setupPrompt}
              perform={perform}
            />
          )}
        </>
      ) : (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {failed && (
        <p role="alert" className="text-sm text-destructive">
          {t("storageProtection.actionFailed")}
        </p>
      )}
      <NoticeSlot>
        {saved && <Notice title={t("storageProtection.saved")} />}
      </NoticeSlot>
      <p className="text-xs text-muted-foreground">
        {t("storageProtection.backupIndependent")}
      </p>
    </section>
  );
}

function ProtectionActions({
  status,
  pending,
  perform,
  setupPrompt,
}: {
  setupPrompt: boolean;
  status: CredentialProtectionStatus;
  pending: boolean;
  perform: (action: () => Promise<CredentialProtectionStatus>) => Promise<void>;
}) {
  const { t } = useTranslation();
  const inputId = useId();
  const acknowledgmentId = useId();
  const confirmationId = useId();
  const input = useRef<LiteralKeyInputHandle>(null);
  const confirmation = useRef<LiteralKeyInputHandle>(null);
  const submitting = useRef(false);
  const [inputComplete, setInputComplete] = useState(false);
  const [choice, setChoice] = useState<CredentialProtectionMode | null>(
    setupPrompt && status.canConfigure === true ? "encrypted" : null,
  );
  const [separate, setSeparate] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  // The input keeps original text only in its transient ref, never React state
  // or browser/query storage. Clear both original and display on every exit.
  useEffect(() => {
    const element = input.current;
    const repeated = confirmation.current;
    return () => {
      if (element) element.value = "";
      if (repeated) repeated.value = "";
    };
  }, [choice, separate]);
  const clearKey = () => {
    if (input.current) input.current.value = "";
    if (confirmation.current) confirmation.current.value = "";
    setInputComplete(false);
  };
  const select = (next: CredentialProtectionMode | null) => {
    clearKey();
    setAcknowledged(false);
    setChoice(next);
  };
  const token =
    typeof status.reviewToken === "string" && status.reviewToken.length > 0
      ? status.reviewToken
      : null;
  if (!token) return null;
  const canConfigure =
    status.canConfigure === true ||
    (separate && status.canCreateSeparateConfiguration === true);
  const canPlaintext =
    status.canSwitchToPlaintext === true ||
    (separate && status.canCreateSeparateConfiguration === true);
  const recovering = status.canUnlock === true && !separate && choice === null;
  const needsKey = recovering || choice === "encrypted";
  const updateValidity = () => {
    const key = input.current?.value ?? "";
    setInputComplete(
      hasKeyText(key) && (recovering || key === confirmation.current?.value),
    );
  };
  const submit = async () => {
    if (pending || submitting.current) return;
    if (needsKey) {
      const keyText = input.current?.value ?? "";
      if (
        !hasKeyText(keyText) ||
        (!recovering &&
          (!canConfigure || keyText !== confirmation.current?.value))
      )
        return;
      submitting.current = true;
      clearKey();
      try {
        await perform(() =>
          recovering
            ? credentialProtectionApi.unlock(keyText, token)
            : credentialProtectionApi.configure({
                mode: "encrypted",
                keyText,
                preserveExisting: separate,
                plaintextAcknowledged: false,
                reviewToken: token,
              }),
        );
      } finally {
        submitting.current = false;
      }
    } else if (choice === "plaintext" && canPlaintext && acknowledged) {
      submitting.current = true;
      try {
        await perform(() =>
          credentialProtectionApi.configure({
            mode: "plaintext",
            preserveExisting: separate,
            plaintextAcknowledged: true,
            reviewToken: token,
          }),
        );
      } finally {
        submitting.current = false;
      }
    }
  };

  return (
    <div className="space-y-4">
      {separate && (
        <Notice
          tone="warning"
          title={t("storageProtection.originalsPreserved")}
        />
      )}
      {(canConfigure || canPlaintext) && (
        <div className="flex flex-wrap gap-2">
          {canConfigure && (
            <Button
              type="button"
              variant={choice === "encrypted" ? "default" : "outline"}
              aria-pressed={choice === "encrypted"}
              disabled={pending}
              onClick={() => select("encrypted")}
            >
              <KeyRound className="h-4 w-4" aria-hidden="true" />
              {t("storageProtection.chooseEncrypted")}
            </Button>
          )}
          {canPlaintext && (
            <Button
              type="button"
              variant={choice === "plaintext" ? "default" : "outline"}
              aria-pressed={choice === "plaintext"}
              disabled={pending}
              onClick={() => select("plaintext")}
            >
              {t(
                setupPrompt
                  ? "storageProtection.skipPlaintext"
                  : "storageProtection.choosePlaintext",
              )}
            </Button>
          )}
        </div>
      )}
      {(needsKey || choice === "plaintext") && (
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          {needsKey ? (
            <div className="space-y-2">
              {recovering && (
                <p className="text-sm text-muted-foreground">
                  {t("storageProtection.sameKeyRequired")}
                </p>
              )}
              <Label htmlFor={inputId}>{t("storageProtection.keyLabel")}</Label>
              <LiteralKeyInput
                id={inputId}
                placeholder={t("storageProtection.keyPlaceholder")}
                ref={input}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="none"
                spellCheck={false}
                aria-describedby={`${inputId}-help`}
                disabled={pending}
                onValueChange={updateValidity}
              />
              <p
                id={`${inputId}-help`}
                className="text-xs text-muted-foreground"
              >
                {t("storageProtection.keyFormat")}
              </p>
              {!recovering && (
                <>
                  <Label htmlFor={confirmationId}>
                    {t("storageProtection.confirmKeyLabel")}
                  </Label>
                  <LiteralKeyInput
                    id={confirmationId}
                    placeholder={t("storageProtection.confirmKeyPlaceholder")}
                    ref={confirmation}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="none"
                    spellCheck={false}
                    disabled={pending}
                    onValueChange={updateValidity}
                  />
                </>
              )}
              <Notice
                tone="neutral"
                title={t("storageProtection.localOnlyNotice")}
              />
              <p className="text-xs text-muted-foreground">
                {t("storageProtection.keepKeyCopy")}
              </p>
            </div>
          ) : (
            <div className="space-y-3">
              <Notice
                tone="warning"
                title={t("storageProtection.plaintextRisk")}
              />
              {status.mode === "encrypted" && !separate && (
                <p className="text-sm text-muted-foreground">
                  {t("storageProtection.plaintextConversion")}
                </p>
              )}
              <div className="flex items-start gap-2">
                <Checkbox
                  id={acknowledgmentId}
                  checked={acknowledged}
                  disabled={pending}
                  onCheckedChange={setAcknowledged}
                />
                <Label htmlFor={acknowledgmentId} className="text-sm leading-5">
                  {t("storageProtection.acknowledgePlaintext")}
                </Label>
              </div>
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="submit"
              disabled={pending || (needsKey ? !inputComplete : !acknowledged)}
            >
              {t(
                recovering
                  ? "storageProtection.unlock"
                  : choice === "encrypted"
                    ? "storageProtection.applyEncrypted"
                    : "storageProtection.applyPlaintext",
              )}
            </Button>
            {choice !== null && (
              <Button
                type="button"
                variant="ghost"
                disabled={pending}
                onClick={() => select(null)}
              >
                {t("common.cancel")}
              </Button>
            )}
          </div>
        </form>
      )}
      {status.canCreateSeparateConfiguration === true && !separate && (
        <div className="border-t pt-4">
          <Button
            type="button"
            variant="outline"
            disabled={pending}
            onClick={() => {
              select(null);
              setSeparate(true);
            }}
          >
            {t("storageProtection.createSeparate")}
          </Button>
        </div>
      )}
      {separate && (
        <Button
          type="button"
          variant="ghost"
          disabled={pending}
          onClick={() => {
            select(null);
            setSeparate(false);
          }}
        >
          {t("storageProtection.cancelSeparate")}
        </Button>
      )}
    </div>
  );
}
