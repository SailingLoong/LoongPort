import { useId, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";

import { safeAccountError, type AccountError } from "@/lib/api/zcodeAccounts";
import { zcodeBackupApi, type BundleExportResult } from "@/lib/api/zcodeBackup";
type BackupDialogProps = {
  open?: boolean;
  onClose: () => void;
  profiles: { id: string; family: "bigmodel" | "zai"; label: string | null }[];
  catalogRevision: string;
  libraryDataRoot?: string;
};
export function useZCodeBackupDialog({
  onClose,
  profiles,
  catalogRevision,
  libraryDataRoot,
}: BackupDialogProps) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.backup.${key}`, { defaultValue, ...values });
  const inputId = useId();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [choosing, setChoosing] = useState(false);
  const [reading, setReading] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [requestId, setRequestId] = useState<string | null>(null);
  const [result, setResult] = useState<BundleExportResult | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<AccountError | null>(null);
  const alive = useRef(false);
  const generation = useRef(0);
  const inFlight = useRef(false);
  const readInFlight = useRef(false);
  const originalRequest = useRef<string | null>(null);
  const secrets = useRef({ password: "", passwordConfirmation: "" });
  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      generation.current += 1;
      secrets.current = { password: "", passwordConfirmation: "" };
    };
  }, []);
  const clearPasswords = () => {
    secrets.current = { password: "", passwordConfirmation: "" };
    setPassword("");
    setConfirmation("");
  };
  const close = () => {
    alive.current = false;
    generation.current += 1;
    clearPasswords();
    setDismissed(true);
    onClose();
  };
  const current = (version: number) =>
    alive.current && generation.current === version;
  const applyResult = (next: BundleExportResult) => {
    setResult(next);
    setUncertain(next.status === "unknown");
    setError(next.error);
  };
  const selectedProfiles = profiles.filter((profile) =>
    selected.has(profile.id),
  );
  const canSubmit =
    selectedProfiles.length > 0 &&
    selectedProfiles.length <= 50 &&
    password.trim().length > 0 &&
    password === confirmation &&
    !choosing &&
    !requestId;
  const submit = async () => {
    if (inFlight.current || originalRequest.current || !canSubmit) return;
    inFlight.current = true;
    setChoosing(true);
    setError(null);
    const version = ++generation.current;
    const profileIds = selectedProfiles.map((profile) => profile.id);
    let destination: string | null;
    try {
      destination = await save({
        defaultPath: "zcode-accounts.zsb",
        filters: [
          {
            name: copy("fileType", "ZCode encrypted backup"),
            extensions: ["zsb"],
          },
        ],
      });
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
      destination = null;
    }
    // Dismissal or a changed source must never turn a late chooser into a write.
    if (!current(version)) return;
    setChoosing(false);
    if (!destination) {
      clearPasswords();
      inFlight.current = false;
      return;
    }
    const credentials = secrets.current;
    clearPasswords();
    try {
      const id = crypto.randomUUID();
      originalRequest.current = id;
      setRequestId(id);
      const next = await zcodeBackupApi.exportBundle({
        requestId: id,
        ...(libraryDataRoot === undefined ? {} : { dataRoot: libraryDataRoot }),
        catalogRevision,
        profileIds,
        destination,
        password: credentials.password,
        passwordConfirmation: credentials.passwordConfirmation,
      });
      if (current(version)) applyResult(next);
    } catch (cause) {
      if (current(version)) {
        setError(safeAccountError(cause));
        setUncertain(originalRequest.current !== null);
      }
    } finally {
      credentials.password = "";
      credentials.passwordConfirmation = "";
      if (current(version)) inFlight.current = false;
    }
  };
  const query = async () => {
    const id = originalRequest.current;
    if (!alive.current || !id || readInFlight.current) return;
    readInFlight.current = true;
    setReading(true);
    setError(null);
    // A later query owns the displayed result even if the original write replies late.
    const version = ++generation.current;
    try {
      const next = await zcodeBackupApi.result(id);
      if (current(version)) applyResult(next);
    } catch (cause) {
      if (current(version)) {
        setUncertain(true);
        setError(safeAccountError(cause));
      }
    } finally {
      if (current(version)) {
        readInFlight.current = false;
        setReading(false);
      }
    }
  };
  const selectProfile = (id: string, checked: boolean) =>
    setSelected((previous) => {
      const next = new Set(previous);
      if (checked && next.size < 50) next.add(id);
      else if (!checked) next.delete(id);
      return next;
    });
  const changePassword = (value: string) => {
    secrets.current.password = value;
    setPassword(value);
  };
  const changeConfirmation = (value: string) => {
    secrets.current.passwordConfirmation = value;
    setConfirmation(value);
  };
  return {
    selectProfile,
    changePassword,
    changeConfirmation,
    t,
    copy,
    inputId,
    selected,
    password,
    confirmation,
    choosing,
    reading,
    dismissed,
    requestId,
    result,
    uncertain,
    error,
    close,
    selectedProfiles,
    canSubmit,
    submit,
    query,
  };
}
