import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Check, FolderOpen, Pencil, Save, Trash2 } from "lucide-react";
import {
  orderProfilesApi,
  type OrderProfilesState,
} from "@/lib/api/orderProfiles";
import { extractErrorMessage } from "@/utils/errorUtils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

/** Named candidate snapshots. Loading is local; only Apply changes the active profile. */
export function OrderProfilesMenu({
  appType,
  state,
  targetIds,
  storedIds,
  onLoadDraft,
  onSaved,
  selectedName,
  disabled = false,
  onBusyChange,
  onProfileRenamed,
  onProfileRemoved,
}: {
  appType: string;
  /** 工作台持有的配置档状态（列表 + 当前），与本菜单共享同一个 query key。 */
  state: OrderProfilesState | undefined;
  /** 「应用此顺序」的目标（可见 ∧ 未屏蔽的显示序），另存为新档的就是它。 */
  targetIds: string[];
  /** 已知档位全集（存储序），载入时滤掉认不出的 id 用。 */
  storedIds: string[];
  onLoadDraft: (name: string, ids: string[]) => void;
  onSaved: (name: string, ids: string[]) => void;
  selectedName?: string;
  disabled?: boolean;
  onBusyChange: (busy: boolean) => void;
  onProfileRenamed: (from: string, to: string) => void;
  onProfileRemoved: (name: string) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [saveOpen, setSaveOpen] = useState(false);
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameTo, setRenameTo] = useState("");
  const profiles = state?.profiles ?? [];
  const current = selectedName ?? state?.current ?? "";
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const locked = disabled || busy;
  const run = async (operation: () => Promise<void>) => {
    if (pending.current || disabled) return;
    pending.current = true;
    setBusy(true);
    onBusyChange(true);
    try {
      await operation();
    } finally {
      pending.current = false;
      setBusy(false);
      onBusyChange(false);
    }
  };
  const refresh = () =>
    client.invalidateQueries({ queryKey: ["orderProfiles", appType] });
  const onError = (error: unknown) =>
    toast.error(t("applications.orderProfileSaveFailed"), {
      description: extractErrorMessage(error) || undefined,
    });

  const load = (profileName: string, providerIds: string[]) => {
    if (pending.current || disabled) return;
    // 载入顺序 = 配置档里认得出的档位（按档内序），不垫底：配置档是链快照，
    // 应用后链就是档内这批——垫底会把链外档位拉回链里，违背链语义。
    const known = new Set(storedIds);
    onLoadDraft(
      profileName,
      providerIds.filter((id) => known.has(id)),
    );
  };

  const save = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await orderProfilesApi.save(appType, trimmed, targetIds);
      onSaved(trimmed, targetIds);
      toast.success(t("applications.orderProfileSaved", { name: trimmed }));
      setSaveOpen(false);
      setName("");
      await refresh();
    } catch (error) {
      onError(error);
    }
  };

  const rename = async () => {
    const to = renameTo.trim();
    if (!to || renaming == null) return;
    try {
      await orderProfilesApi.rename(appType, renaming, to);
      onProfileRenamed(renaming, to);
      setRenaming(null);
      await refresh();
    } catch (error) {
      onError(error);
    }
  };

  const remove = async (profileName: string) => {
    try {
      await orderProfilesApi.remove(appType, profileName);
      onProfileRemoved(profileName);
      await refresh();
    } catch (error) {
      onError(error);
    }
  };

  const importFromFile = async () => {
    try {
      const count = await orderProfilesApi.import(appType);
      if (count != null) {
        toast.success(
          t("applications.orderProfilesImported", { count: String(count) }),
        );
        await refresh();
      }
    } catch (error) {
      onError(error);
    }
  };

  const exportToFile = async () => {
    try {
      const path = await orderProfilesApi.export(appType);
      if (path) toast.success(t("applications.orderProfileExported"));
    } catch (error) {
      onError(error);
    }
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="sm"
            disabled={locked || !state}
            variant="outline"
            className="h-7 max-w-48 gap-1.5 text-xs"
            title={t("applications.orderProfiles")}
          >
            <FolderOpen className="h-3.5 w-3.5 shrink-0" />
            <span className="truncate">
              {current || t("applications.orderProfiles")}
            </span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-96 overflow-y-auto">
          {profiles.length === 0 && (
            <p className="px-2 py-3 text-xs text-muted-foreground">
              {t("applications.orderProfilesEmpty")}
            </p>
          )}
          {profiles.map((profile) => {
            const isCurrent = profile.name === current;
            return (
              <DropdownMenuItem
                key={profile.name}
                disabled={locked}
                className="gap-2"
                onSelect={() => load(profile.name, profile.providerIds)}
              >
                {isCurrent ? (
                  <Check className="h-3.5 w-3.5 shrink-0 text-primary" />
                ) : (
                  <span className="w-3.5 shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate" title={profile.name}>
                  {profile.name}
                </span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {profile.providerIds.length}
                </span>
                <Button
                  size="icon"
                  disabled={locked}
                  variant="ghost"
                  className="h-6 w-6 shrink-0"
                  aria-label={t("applications.orderProfileRename", {
                    name: profile.name,
                  })}
                  title={t("applications.orderProfileRename", {
                    name: profile.name,
                  })}
                  // 下拉里的行内动作不能触发外层 onSelect 的载入：截断事件。
                  onClick={(event) => {
                    event.stopPropagation();
                    if (pending.current || disabled) return;
                    setRenaming(profile.name);
                    setRenameTo(profile.name);
                  }}
                >
                  <Pencil className="h-3 w-3" />
                </Button>
                <Button
                  size="icon"
                  disabled={locked}
                  variant="ghost"
                  className="h-6 w-6 shrink-0"
                  aria-label={t("applications.orderProfileDelete", {
                    name: profile.name,
                  })}
                  title={t("applications.orderProfileDelete", {
                    name: profile.name,
                  })}
                  onClick={(event) => {
                    event.stopPropagation();
                    void run(() => remove(profile.name));
                  }}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              </DropdownMenuItem>
            );
          })}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            disabled={locked}
            onSelect={() => {
              if (!pending.current && !disabled) setSaveOpen(true);
            }}
          >
            <Save className="h-3.5 w-3.5" />
            {t("applications.orderProfileSaveCurrent")}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={locked}
            onSelect={() => void run(importFromFile)}
          >
            {t("applications.orderProfileImport")}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={locked}
            onSelect={() => void run(exportToFile)}
          >
            {t("applications.orderProfileExport")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog
        open={saveOpen}
        onOpenChange={(open) => {
          if (!busy) setSaveOpen(open);
        }}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("applications.orderProfileSaveCurrent")}
            </DialogTitle>
            <DialogDescription>
              {t("applications.orderProfileSaveHint")}
            </DialogDescription>
          </DialogHeader>
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t("applications.orderProfileNamePlaceholder")}
            aria-label={t("applications.orderProfileNamePlaceholder")}
            autoFocus
          />
          <DialogFooter className="gap-2">
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setSaveOpen(false)}
            >
              {t("common.cancel")}
            </Button>
            <Button
              disabled={locked || !name.trim()}
              onClick={() => void run(save)}
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={renaming != null}
        onOpenChange={(open) => {
          if (!open && !busy) setRenaming(null);
        }}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("applications.orderProfileRenameTitle")}
            </DialogTitle>
          </DialogHeader>
          <Input
            value={renameTo}
            onChange={(event) => setRenameTo(event.target.value)}
            placeholder={t("applications.orderProfileNamePlaceholder")}
            aria-label={t("applications.orderProfileNamePlaceholder")}
            autoFocus
          />
          <DialogFooter className="gap-2">
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setRenaming(null)}
            >
              {t("common.cancel")}
            </Button>
            <Button
              disabled={
                locked || !renameTo.trim() || renameTo.trim() === renaming
              }
              onClick={() => void run(rename)}
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
