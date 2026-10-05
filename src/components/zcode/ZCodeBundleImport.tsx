import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  accountErrorText,
  safeAccountError,
  zcodeAccountsApi,
  type BundlePreview,
  type ContextSelection,
} from "@/lib/api/zcodeAccounts";

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
type Review = {
  preview: BundlePreview;
  source: ContextSelection;
  contextRevision: string;
  catalogRevision: string;
};
export function ZCodeBundleImport({
  source,
  contextRevision,
  catalogRevision,
  enabled,
  onActiveChange,
  onImported,
}: {
  source: ContextSelection;
  contextRevision: string;
  catalogRevision: string;
  enabled: boolean;
  onActiveChange: (active: boolean) => void;
  onImported: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.${key}`, { defaultValue, ...values });
  const [file, setFile] = useState<File | null>(null);
  const [password, setPassword] = useState("");
  const [review, setReview] = useState<Review | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [updates, setUpdates] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [resultRows, setResultRows] = useState<
    {
      row: BundlePreview["rows"][number];
      outcome: "saved" | "refreshed" | "kept" | "unknown";
    }[]
  >([]);
  const inFlight = useRef(false);
  const alive = useRef(true);
  const lease = useRef<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (lease.current)
        void zcodeAccountsApi.cancelBundle(lease.current).catch(() => {});
    };
  }, []);
  const resetFile = () => {
    setFile(null);
    setPassword("");
    if (fileInput.current) fileInput.current.value = "";
  };
  const fail = (cause: unknown) => {
    const failure = safeAccountError(cause);
    const key = failure.code.slice("zcode.account.".length);
    setError(
      t(`zcode.accounts.errors.${key}`, {
        defaultValue:
          accountErrorText[key as keyof typeof accountErrorText] ??
          "The account operation could not be verified. Refresh the source and review again.",
      }),
    );
  };
  const preview = async () => {
    if (inFlight.current || !enabled || !file || !password.trim()) return;
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
    setBusy(true);
    onActiveChange(true);
    setError(null);
    setNotice(null);
    setResultRows([]);
    const inputPassword = password;
    setPassword("");
    const binding = { source: { ...source }, contextRevision, catalogRevision };
    let bytes: Uint8Array | null = null;
    try {
      bytes = new Uint8Array(await readFile(file));
      const result = await zcodeAccountsApi.previewBundle(
        binding.source,
        binding.contextRevision,
        binding.catalogRevision,
        Array.from(bytes),
        inputPassword,
      );
      if (!alive.current) {
        await zcodeAccountsApi.cancelBundle(result.previewId);
        return;
      }
      lease.current = result.previewId;
      setReview({ ...binding, preview: result });
      // Conflicting records have no established freshness: choose none implicitly.
      setSelected(
        new Set(
          result.rows
            .filter((row) => !row.error && !row.ambiguous)
            .map((row) => row.index),
        ),
      );
      setUpdates(new Set());
    } catch (cause) {
      if (alive.current) {
        fail(cause);
        onActiveChange(false);
      }
    } finally {
      bytes?.fill(0);
      inFlight.current = false;
      if (alive.current) {
        setBusy(false);
        resetFile();
      }
    }
  };
  const cancel = async () => {
    if (inFlight.current || !review) return;
    inFlight.current = true;
    setBusy(true);
    try {
      await zcodeAccountsApi.cancelBundle(review.preview.previewId);
    } catch (cause) {
      if (alive.current) fail(cause);
    } finally {
      lease.current = null;
      inFlight.current = false;
      if (alive.current) {
        setReview(null);
        setBusy(false);
        resetFile();
        onActiveChange(false);
      }
    }
  };
  const commit = async () => {
    if (inFlight.current || !review || selected.size === 0) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    let submittedRows: BundlePreview["rows"] = [];
    try {
      if (
        contextRevision !== review.contextRevision ||
        catalogRevision !== review.catalogRevision ||
        JSON.stringify(source) !== JSON.stringify(review.source)
      ) {
        await zcodeAccountsApi.cancelBundle(review.preview.previewId);
        throw {
          code: "zcode.account.context_changed",
          remedy: "refreshContext",
          committed: false,
        };
      }
      const choices = review.preview.rows
        .filter((row) => selected.has(row.index) && !row.error)
        .map((row) => ({
          index: row.index,
          updateDuplicate: updates.has(row.index),
        }));
      const rows = review.preview.rows.filter((row) =>
        choices.some((choice) => choice.index === row.index),
      );
      submittedRows = rows;
      const results = await zcodeAccountsApi.importBundle(
        review.source,
        review.contextRevision,
        review.catalogRevision,
        review.preview.previewId,
        choices,
      );
      if (
        !Array.isArray(results) ||
        results.length !== rows.length ||
        !results.every((result) =>
          ["saved", "refreshed", "kept"].includes(result),
        )
      )
        throw {
          code: "zcode.account.saved_data_invalid",
          remedy: "reviewSavedData",
          committed: false,
        };
      if (alive.current) {
        setResultRows(
          rows.map((row, index) => ({ row, outcome: results[index] })),
        );
        setNotice(
          copy(
            "bundleImported",
            "Imported into the encrypted vault: {{saved}} saved, {{updated}} updated, {{kept}} kept. Imported credentials require official local sign-in verification before switching.",
            {
              saved: results.filter((x) => x === "saved").length,
              updated: results.filter((x) => x === "refreshed").length,
              kept: results.filter((x) => x === "kept").length,
            },
          ),
        );
        try {
          await onImported();
        } catch {
          if (alive.current) {
            setError(
              copy(
                "bundleRefreshFailed",
                "Import completed, but the saved account list could not be refreshed. Refresh or inspect the source again before another action.",
              ),
            );
          }
        }
      }
    } catch (cause) {
      if (alive.current) {
        setResultRows(
          submittedRows.map((row) => ({ row, outcome: "unknown" })),
        );
        fail(cause);
      }
    } finally {
      lease.current = null;
      inFlight.current = false;
      if (alive.current) {
        setReview(null);
        setBusy(false);
        resetFile();
        onActiveChange(false);
      }
    }
  };
  return (
    <div className="space-y-3 rounded-md border p-3">
      <p className="text-sm">
        {copy(
          "bundleIntro",
          "Import a fixed .zsb account bundle into the encrypted local vault. Imported source scope is unverified; importing never switches ZCode.",
        )}
      </p>
      <Label htmlFor="zcode-bundle-file">
        {copy("bundleFile", "Account bundle (.zsb)")}
      </Label>
      <Input
        ref={fileInput}
        id="zcode-bundle-file"
        type="file"
        accept=".zsb"
        disabled={!enabled || busy || !!review}
        onChange={(event) => {
          setFile(event.target.files?.[0] ?? null);
          setError(null);
        }}
      />
      <Label htmlFor="zcode-bundle-password">
        {copy("bundlePassword", "Bundle password")}
      </Label>
      <Input
        id="zcode-bundle-password"
        type="password"
        value={password}
        autoComplete="off"
        spellCheck={false}
        disabled={!enabled || busy || !!review}
        onChange={(event) => setPassword(event.target.value)}
      />
      <Button
        type="button"
        disabled={!enabled || busy || !!review || !file || !password.trim()}
        onClick={() => void preview()}
      >
        {copy("bundlePreview", "Preview account bundle")}
      </Button>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="text-sm">
          {notice}
        </p>
      )}
      {resultRows.length > 0 && (
        <ul
          aria-label={copy("bundleResults", "Last import results by account")}
          className="max-h-48 space-y-1 overflow-y-auto text-sm"
        >
          {resultRows.map(({ row, outcome }) => (
            <li key={row.index}>
              {row.index + 1}.{" "}
              {row.label || copy("bundleMaskedAccount", "Masked account")} ·{" "}
              {row.family === "bigmodel" ? "BigModel" : "Z.ai"} ·{" "}
              {outcome === "saved"
                ? copy("bundleResultSaved", "Saved")
                : outcome === "refreshed"
                  ? copy("bundleResultUpdated", "Updated")
                  : outcome === "kept"
                    ? copy("bundleResultKept", "Kept existing")
                    : copy(
                        "bundleResultUnknown",
                        "Unconfirmed — inspect saved accounts",
                      )}
            </li>
          ))}
        </ul>
      )}
      <Dialog
        open={!!review}
        onOpenChange={(open) => {
          if (!open && !busy) void cancel();
        }}
      >
        <DialogContent className="flex max-h-[85vh] flex-col">
          <DialogHeader>
            <DialogTitle>
              {copy("bundleReviewTitle", "Review bundle accounts")}
            </DialogTitle>
            <DialogDescription>
              {copy(
                "bundleReviewMessage",
                "Choose accounts to store. Existing accounts are kept unless explicitly updated. Updates replace the saved credentials and reset source verification. Conflicting records require choosing one; file order does not establish freshness.",
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 space-y-3 overflow-y-auto">
            {review?.preview.rows.map((row) => (
              <div key={row.index} className="space-y-2 rounded border p-3">
                <label className="flex items-start gap-2">
                  <Checkbox
                    aria-label={copy(
                      "bundleSelect",
                      "Select account {{number}}",
                      { number: row.index + 1 },
                    )}
                    checked={selected.has(row.index)}
                    disabled={busy || !!row.error}
                    onCheckedChange={(checked) =>
                      setSelected((previous) => {
                        const next = new Set(previous);
                        if (checked === true) {
                          for (const peer of review.preview.rows)
                            if (peer.id === row.id) next.delete(peer.index);
                          next.add(row.index);
                        } else next.delete(row.index);
                        return next;
                      })
                    }
                  />
                  <span className="break-all text-sm">
                    {row.index + 1}.{" "}
                    {row.label ??
                      row.id ??
                      copy("bundleInvalidEntry", "Unusable account entry")}{" "}
                    {row.family}
                  </span>
                </label>
                {row.error && (
                  <p className="text-sm">
                    {row.error === "incompatibleCredentials"
                      ? copy(
                          "bundleIncompatible",
                          "Credentials cannot be verified with this target's standard key. Sign in with the account in official ZCode and save it locally instead.",
                        )
                      : copy("bundleInvalidEntry", "Unusable account entry")}
                  </p>
                )}
                {row.ambiguous && (
                  <p className="text-sm">
                    {copy(
                      "bundleAmbiguous",
                      "Same identity appears more than once. Explicitly choose one record; none is selected by default.",
                    )}
                  </p>
                )}
                {row.duplicate && (
                  <label className="flex items-start gap-2">
                    <Checkbox
                      aria-label={copy(
                        "bundleUpdate",
                        "Update existing account {{number}}",
                        { number: row.index + 1 },
                      )}
                      checked={updates.has(row.index)}
                      disabled={busy || !selected.has(row.index)}
                      onCheckedChange={(checked) =>
                        setUpdates((previous) => {
                          const next = new Set(previous);
                          checked === true
                            ? next.add(row.index)
                            : next.delete(row.index);
                          return next;
                        })
                      }
                    />
                    <span className="text-sm">
                      {copy(
                        "updateDuplicate",
                        "Explicitly update this existing saved account",
                      )}
                    </span>
                  </label>
                )}
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => void cancel()}
            >
              {t("common.cancel", { defaultValue: "Cancel" })}
            </Button>
            <Button
              disabled={busy || selected.size === 0}
              onClick={() => void commit()}
            >
              {copy("bundleImport", "Import selected into encrypted vault")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
