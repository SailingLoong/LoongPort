import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { FolderOpen, Save, Trash2 } from "lucide-react";
import { orderProfilesApi } from "@/lib/api/orderProfiles";
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
 * 档位顺序配置档（2026-09-16）：命名的顺序快照，多份共存、可覆盖/导入/导出。
 *
 * - **载入 = 进草稿**：点配置档把该顺序载入暂存（认不出的档位 id 滤掉、
 *   剩余档位按存储序垫底），之后照常「应用/取消」——不直接写库
 * - **保存 = 当前显示序**：拖过/排过序的都算，同名覆盖
 * - 导入导出走 JSON 文件（跨机导入的 id 解析不了会自愈滤掉，只剩同机备份意义）
 */
export function OrderProfilesMenu({
  appType,
  displayedIds,
  storedIds,
  onLoadDraft,
}: {
  appType: string;
  /** 当前显示序（拖拽/排序后的），保存进配置档的就是它。 */
  displayedIds: string[];
  /** 已知档位全集（存储序），载入时垫底用。 */
  storedIds: string[];
  onLoadDraft: (ids: string[]) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [saveOpen, setSaveOpen] = useState(false);
  const [name, setName] = useState("");
  const { data: profiles } = useQuery({
    queryKey: ["orderProfiles", appType],
    queryFn: () => orderProfilesApi.list(appType),
  });
  const refresh = () =>
    client.invalidateQueries({ queryKey: ["orderProfiles", appType] });

  const load = (providerIds: string[]) => {
    // 载入顺序 = 配置档里认得出的档位（按档内序）+ 不在档内的已知档位垫底
    //（垫底序 = 存储序）——与后端 set_order 的合流语义一致。
    const known = new Set(storedIds);
    const inProfile = providerIds.filter((id) => known.has(id));
    const inProfileSet = new Set(inProfile);
    onLoadDraft([
      ...inProfile,
      ...storedIds.filter((id) => !inProfileSet.has(id)),
    ]);
  };

  const save = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await orderProfilesApi.save(appType, trimmed, displayedIds);
      toast.success(t("applications.orderProfileSaved", { name: trimmed }));
      setSaveOpen(false);
      setName("");
      await refresh();
    } catch (error) {
      toast.error(t("applications.orderProfileSaveFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  const remove = async (profileName: string) => {
    try {
      await orderProfilesApi.remove(appType, profileName);
      await refresh();
    } catch (error) {
      toast.error(t("applications.orderProfileSaveFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
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
      toast.error(t("applications.orderProfileSaveFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  const exportToFile = async () => {
    try {
      const path = await orderProfilesApi.export(appType);
      if (path) toast.success(t("applications.orderProfileExported"));
    } catch (error) {
      toast.error(t("applications.orderProfileSaveFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="sm"
            variant="outline"
            className="h-7 gap-1.5 text-xs"
            title={t("applications.orderProfiles")}
          >
            <FolderOpen className="h-3.5 w-3.5" />
            {t("applications.orderProfiles")}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-96 overflow-y-auto">
          {(profiles ?? []).length === 0 && (
            <p className="px-2 py-3 text-xs text-muted-foreground">
              {t("applications.orderProfilesEmpty")}
            </p>
          )}
          {(profiles ?? []).map((profile) => (
            <DropdownMenuItem
              key={profile.name}
              className="gap-2"
              onSelect={() => load(profile.providerIds)}
            >
              <span className="min-w-0 flex-1 truncate">{profile.name}</span>
              <span className="shrink-0 text-xs text-muted-foreground">
                {profile.providerIds.length}
              </span>
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
                // 下拉里的删除不能触发外层 onSelect 的载入：截断事件。
                onClick={(event) => {
                  event.stopPropagation();
                  void remove(profile.name);
                }}
              >
                <Trash2 className="h-3 w-3" />
              </Button>
            </DropdownMenuItem>
          ))}
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
    </>
  );
}
