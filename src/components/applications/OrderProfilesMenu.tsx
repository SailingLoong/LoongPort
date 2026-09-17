import { useState } from "react";
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

/**
 * 档位顺序配置档 = 命名的链快照，其中一份是「当前配置文件」（2026-09-17 定调）：
 *
 * - **当前档**：触发器上直接显示名称；「应用此顺序」默认保存进它（工作台负责）
 * - **切换**：点某档 = 载入草稿 + 切当前指针，照常「应用/取消」
 * - **新建/另存为**：把应用目标存成新档并切换过去（后端 save 即设当前）
 * - **重命名**：行内铅笔；改当前档时指针跟着走；撞名拒绝
 * - 导入导出走 JSON 文件（跨机导入的 id 解析不了会自愈滤掉，只剩同机备份意义）
 */
export function OrderProfilesMenu({
  appType,
  state,
  targetIds,
  storedIds,
  onLoadDraft,
}: {
  appType: string;
  /** 工作台持有的配置档状态（列表 + 当前），与本菜单共享同一个 query key。 */
  state: OrderProfilesState | undefined;
  /** 「应用此顺序」的目标（可见 ∧ 未屏蔽的显示序），另存为新档的就是它。 */
  targetIds: string[];
  /** 已知档位全集（存储序），载入时滤掉认不出的 id 用。 */
  storedIds: string[];
  onLoadDraft: (ids: string[]) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [saveOpen, setSaveOpen] = useState(false);
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameTo, setRenameTo] = useState("");
  const profiles = state?.profiles ?? [];
  const current = state?.current ?? "";
  const refresh = () =>
    client.invalidateQueries({ queryKey: ["orderProfiles", appType] });
  const onError = (error: unknown) =>
    toast.error(t("applications.orderProfileSaveFailed"), {
      description: extractErrorMessage(error) || undefined,
    });

  const load = (profileName: string, providerIds: string[]) => {
    // 载入顺序 = 配置档里认得出的档位（按档内序），不垫底：配置档是链快照，
    // 应用后链就是档内这批——垫底会把链外档位拉回链里，违背链语义。
    const known = new Set(storedIds);
    onLoadDraft(providerIds.filter((id) => known.has(id)));
    void orderProfilesApi
      .setCurrent(appType, profileName)
      .then(refresh)
      .catch(onError);
  };

  const save = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await orderProfilesApi.save(appType, trimmed, targetIds);
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
      setRenaming(null);
      await refresh();
    } catch (error) {
      onError(error);
    }
  };

  const remove = async (profileName: string) => {
    try {
      await orderProfilesApi.remove(appType, profileName);
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
                    setRenaming(profile.name);
                    setRenameTo(profile.name);
                  }}
                >
                  <Pencil className="h-3 w-3" />
                </Button>
                <Button
                  size="icon"
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
                    void remove(profile.name);
                  }}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              </DropdownMenuItem>
            );
          })}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setSaveOpen(true)}>
            <Save className="h-3.5 w-3.5" />
            {t("applications.orderProfileSaveCurrent")}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void importFromFile()}>
            {t("applications.orderProfileImport")}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void exportToFile()}>
            {t("applications.orderProfileExport")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={saveOpen} onOpenChange={setSaveOpen}>
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
            <Button variant="ghost" onClick={() => setSaveOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button disabled={!name.trim()} onClick={() => void save()}>
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={renaming != null}
        onOpenChange={(open) => {
          if (!open) setRenaming(null);
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
            <Button variant="ghost" onClick={() => setRenaming(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              disabled={!renameTo.trim() || renameTo.trim() === renaming}
              onClick={() => void rename()}
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
