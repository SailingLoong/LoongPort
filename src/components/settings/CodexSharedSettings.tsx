import {
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type Ref,
} from "react";
import { useTranslation } from "react-i18next";
import JsonEditor from "@/components/JsonEditor";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  getSharedSettingsApi,
  type SharedSettingsApp,
  type SharedSettingsPreview,
  type SharedSettingsImport,
  type SharedSettingsRequest,
  type SharedSettingsResult,
} from "@/lib/api/config";

export interface CodexSharedSettingsHandle {
  hasPendingChanges: boolean;
  submitting: boolean;
  discard: () => void;
}
interface Props {
  app?: SharedSettingsApp;
  ref?: Ref<CodexSharedSettingsHandle>;
  onClose: () => void;
  beforeLeave: (proceed: () => void) => void;
  onManageMcp?: () => void;
}
const errorCodes = [
  "readOnlyFields",
  "invalidFormat",
  "sourceChanged",
  "ownerUnavailable",
  "vaultLocked",
  "pendingOperation",
  "legacyDisabled",
];
const resultStatuses = [
  "completed",
  "pending",
  "verificationRequired",
  "discarded",
  "abandoned",
  "notRecorded",
  "unknown",
  "conflict",
  "stale",
  "blocked",
];
const unresolvedStatuses = ["pending", "verificationRequired", "unknown"];

/** Independent live-config projection; never sends the provider or settings form. */
export function CodexSharedSettings({
  app = "codex",
  ref,
  onClose,
  onManageMcp,
  beforeLeave,
}: Props) {
  const { t } = useTranslation();
  const api = getSharedSettingsApi(app);
  const appText = `${app}SharedSettings`;
  const [draft, setDraft] = useState("");
  const [baseline, setBaseline] = useState("");
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const dirtyRef = useRef(false);
  const [preview, setPreview] = useState<SharedSettingsPreview | null>(null);
  const [result, setResult] = useState<SharedSettingsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [importOptions, setImportOptions] =
    useState<SharedSettingsImport | null>(null);
  const [selectedFields, setSelectedFields] = useState<number[]>([]);
  const submittedDraft = useRef<string | null>(null);
  const [darkMode, setDarkMode] = useState(() =>
    document.documentElement.classList.contains("dark"),
  );
  const unresolved = !!result && unresolvedStatuses.includes(result.status);
  dirtyRef.current = draft !== baseline;

  function setOperation(value: boolean) {
    busyRef.current = value;
    setBusy(value);
  }
  function showError(code?: string) {
    setError(errorCodes.includes(code ?? "") ? code! : "generic");
  }
  function discard() {
    dirtyRef.current = false;
    setDraft(baseline);
    setPreview(null);
    setImportOptions(null);
    setSelectedFields([]);
  }
  useImperativeHandle(ref, () => ({
    get hasPendingChanges() {
      return dirtyRef.current;
    },
    get submitting() {
      return busyRef.current;
    },
    discard,
  }));

  async function load() {
    if (busyRef.current) return;
    setOperation(true);
    setReady(false);
    setPreview(null);
    setImportOptions(null);
    setSelectedFields([]);
    setError(null);
    setResult(null);
    submittedDraft.current = null;
    try {
      const value = await api.get();
      setResult(value.originalSave);
      if (value.status !== "ready") {
        showError(value.code);
        return;
      }
      setDraft(value.config);
      setBaseline(value.config);
      setReady(true);
    } catch {
      showError();
    } finally {
      setOperation(false);
    }
  }
  useEffect(() => {
    void load();
  }, []);
  useEffect(() => {
    const observer = new MutationObserver(() =>
      setDarkMode(document.documentElement.classList.contains("dark")),
    );
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  function acceptResult(value: SharedSettingsResult) {
    setResult(value);
    if (value.status === "completed" && submittedDraft.current !== null) {
      setBaseline(submittedDraft.current);
      dirtyRef.current = false;
    }
    if (value.status === "completed" && submittedDraft.current === null)
      setReady(false);
    if (value.code) showError(value.code);
  }
  async function lookup(request: SharedSettingsRequest) {
    try {
      acceptResult(await api.result(request));
    } catch {
      setResult({ app, request, status: "unknown" });
      showError();
    }
  }
  async function checkResult() {
    if (busyRef.current || !result) return;
    setOperation(true);
    setError(null);
    await lookup(result.request);
    setOperation(false);
  }
  async function loadImportOptions() {
    if (busyRef.current || !ready || unresolved) return;
    setOperation(true);
    setPreview(null);
    setImportOptions(null);
    setSelectedFields([]);
    setError(null);
    try {
      const value = await api.getImport();
      if (value.status === "ready") setImportOptions(value);
      else showError(value.code);
    } catch {
      showError();
    } finally {
      setOperation(false);
    }
  }
  async function previewImport(fields: string[][]) {
    if (busyRef.current || !ready || unresolved || fields.length === 0) return;
    setOperation(true);
    setPreview(null);
    setResult(null);
    setError(null);
    try {
      const value = await api.previewImport(fields, crypto.randomUUID());
      if (value.status === "ready") {
        // The backend seeds selected missing fields into the latest live projection.
        setDraft(value.config);
        setPreview(value);
        setImportOptions(null);
        setSelectedFields([]);
      } else showError(value.code);
    } catch {
      showError();
    } finally {
      setOperation(false);
    }
  }
  async function previewChanges() {
    if (busyRef.current || !ready || unresolved || !dirtyRef.current) return;
    setOperation(true);
    setPreview(null);
    setResult(null);
    setError(null);
    try {
      const value = await api.preview(draft, crypto.randomUUID());
      if (value.status === "ready") setPreview(value);
      else showError(value.code);
    } catch {
      showError();
    } finally {
      setOperation(false);
    }
  }
  async function save() {
    if (busyRef.current || !ready || !preview || unresolved) return;
    const request = preview.request;
    submittedDraft.current = draft;
    setOperation(true);
    setPreview(null);
    setError(null);
    try {
      const value = await api.save(draft, request);
      acceptResult(value);
      if (value.status === "unknown") await lookup(request);
    } catch {
      // The write may have completed. Ask for this exact receipt, never repeat it.
      await lookup(request);
    } finally {
      setOperation(false);
    }
  }

  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) beforeLeave(onClose);
      }}
    >
      <SheetContent width={720} closeLabel={t("common.close")}>
        <SheetHeader>
          <SheetTitle>{t(`${appText}.title`)}</SheetTitle>
          <SheetDescription>{t(`${appText}.description`)}</SheetDescription>
        </SheetHeader>
        <SheetBody className="space-y-4">
          <Notice title={t("codexSharedSettings.readOnlyFields")}>
            {t("codexSharedSettings.scope")}
          </Notice>
          {onManageMcp && (
            <Button
              variant="link"
              className="h-auto px-0"
              disabled={busy}
              onClick={() =>
                beforeLeave(() => {
                  onClose();
                  onManageMcp();
                })
              }
            >
              {t(`${appText}.manageMcp`)}
            </Button>
          )}
          {importOptions ? (
            <Notice title={t("codexSharedSettings.import.title")}>
              <p>{t("codexSharedSettings.import.description")}</p>
              {importOptions.fields.length === 0 ? (
                <p className="mt-2">{t("codexSharedSettings.import.empty")}</p>
              ) : (
                <ul className="mt-3 space-y-3">
                  {importOptions.fields.map((field, index) => (
                    <li key={JSON.stringify(field.path)}>
                      <label className="flex items-start gap-2">
                        <Checkbox
                          aria-label={field.field}
                          checked={selectedFields.includes(index)}
                          disabled={busy || unresolved}
                          onCheckedChange={(checked) =>
                            setSelectedFields((selected) =>
                              checked
                                ? [...selected, index]
                                : selected.filter((value) => value !== index),
                            )
                          }
                        />
                        <span className="min-w-0">
                          <span className="block break-all font-medium">
                            {field.field}
                          </span>
                          <span className="block whitespace-pre-wrap break-all font-mono select-text">
                            {field.value}
                          </span>
                        </span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}
              <div className="mt-3 flex gap-2">
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setImportOptions(null);
                    setSelectedFields([]);
                  }}
                >
                  {t("codexSharedSettings.import.cancel")}
                </Button>
                <Button
                  variant="outline"
                  disabled={busy || unresolved || selectedFields.length === 0}
                  onClick={() => {
                    const fields = selectedFields.map(
                      (index) => importOptions.fields[index].path,
                    );
                    beforeLeave(() => void previewImport(fields));
                  }}
                >
                  {t("codexSharedSettings.import.preview")}
                </Button>
              </div>
            </Notice>
          ) : (
            <Button
              variant="outline"
              disabled={!ready || busy || unresolved}
              onClick={() => void loadImportOptions()}
            >
              {t("codexSharedSettings.import.open")}
            </Button>
          )}
          <JsonEditor
            ariaLabel={t(`${appText}.editor`)}
            value={draft}
            onChange={(value) => {
              if (busyRef.current || !ready || unresolved) return;
              setDraft(value);
              setPreview(null);
              setResult(null);
              setError(null);
            }}
            language={app === "claude" ? "json" : "javascript"}
            showFormat={false}
            showValidation={false}
            redactErrors
            darkMode={darkMode}
            rows={16}
            readOnly={!ready || busy || unresolved}
          />
          <NoticeSlot>
            {busy && <Notice title={t("common.loading")} />}
            {error && (
              <Notice
                tone="warning"
                title={t(
                  app === "claude" && error === "invalidFormat"
                    ? "claudeSharedSettings.invalidFormat"
                    : `codexSharedSettings.error.${error}`,
                )}
              />
            )}
            {result && (
              <Notice
                tone={result.status === "completed" ? "neutral" : "warning"}
                title={t(
                  resultStatuses.includes(result.status)
                    ? `codexSharedSettings.result.${result.status}`
                    : "codexSharedSettings.error.generic",
                )}
              />
            )}
            {preview && (
              <Notice
                title={t("codexSharedSettings.previewTitle", {
                  count: preview.changes.length,
                })}
              >
                <ul className="space-y-3">
                  {preview.changes.map((change) => (
                    <li key={change.field}>
                      <p>
                        <span className="font-medium">{change.field}</span>
                        <span className="ms-2">
                          {t(
                            `codexSharedSettings.${!("before" in change) ? "added" : !("after" in change) ? "removed" : "modified"}`,
                          )}
                        </span>
                      </p>
                      <dl className="mt-1 space-y-1">
                        {(["before", "after"] as const).map(
                          (side) =>
                            side in change && (
                              <div key={side}>
                                <dt>{t(`codexSharedSettings.${side}`)}</dt>
                                <dd className="whitespace-pre-wrap break-all font-mono select-text">
                                  {change[side]}
                                </dd>
                              </div>
                            ),
                        )}
                      </dl>
                    </li>
                  ))}
                </ul>
                <p className="mt-2">{t("codexSharedSettings.versionCheck")}</p>
                <p>{t("codexSharedSettings.currentUnchanged")}</p>
              </Notice>
            )}
          </NoticeSlot>
          <p className="text-caption text-fg-2">
            {t("codexSharedSettings.saveScope")}
          </p>
          <div className="flex gap-2">
            {result && unresolved && (
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => void checkResult()}
              >
                {t("codexSharedSettings.checkResult")}
              </Button>
            )}
            <Button
              variant="outline"
              disabled={busy || unresolved}
              onClick={() => beforeLeave(() => void load())}
            >
              {t("codexSharedSettings.reload")}
            </Button>
          </div>
        </SheetBody>
        <SheetFooter>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => beforeLeave(onClose)}
          >
            {t("common.cancel")}
          </Button>
          <Button
            variant="outline"
            disabled={!ready || busy || unresolved || draft === baseline}
            onClick={() => void previewChanges()}
          >
            {t("codexSharedSettings.preview")}
          </Button>
          <Button
            disabled={!ready || busy || unresolved || !preview}
            onClick={() => void save()}
          >
            {t("codexSharedSettings.save")}
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
