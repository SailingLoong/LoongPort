/**
 * 生图页「生成」视图：在 App 里直接生图，不经过任何 CLI 会话。
 *
 * 与 MCP 工具（codex / claude 对话里调用 `loongport-imagegen`）是同一个后端核心
 * （`relay::imagegen`）的两个入口 —— 档位选择、请求形状、落盘完全一致；这条链路
 * 的产物不进宿主对话，也就不消耗任何会话上下文。
 *
 * ## 档位选择没有第二份状态
 *
 * 「当前用哪个档位生图」的唯一 owner 是生图栏的启用项（`providers.is_current` +
 * 设备级 `currentProviderCodexImage`）。这里顶部的快切只是同一个动作的第二个入口
 * （复用 `relayApi.switchTier`），切换后 MCP 那条链路同步跟着变 —— 那是期望中的
 * 一致：用户心里的模型是「我现在用这家生图」。见 CLAUDE.md「前端只展示后端定义的
 * 业务事实」。
 */

import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { fmtBytes } from "@/lib/format";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  Check,
  ChevronsUpDown,
  FolderInput,
  FolderOpen,
  Loader2,
  Sparkles,
} from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { relayApi, settingsApi, type ImagegenGalleryEntry } from "@/lib/api";
import {
  imagegenKeys,
  useImageTiers,
  useImagegenGallery,
  useImagegenGenerate,
  useImagegenOutputDir,
  useImagegenSetOutputDir,
} from "@/lib/query/imagegen";

/** 尺寸档位：gpt-image 的三档（与 MCP 工具的 size 语义一致，不给「自动」）。 */
const SIZE_OPTIONS = ["1024x1024", "1536x1024", "1024x1536"] as const;

export function ImagegenPlayground() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const gallery = useImagegenGallery();
  const generate = useImagegenGenerate();
  const outputDir = useImagegenOutputDir();
  const setOutputDir = useImagegenSetOutputDir();
  const [prompt, setPrompt] = useState("");
  const [size, setSize] = useState<string>("1024x1024");
  const [count, setCount] = useState<string>("1");
  // 并发提交是本次会话的生成偏好（默认开），不落设置 —— 折叠态这类 UI 偏好归前端。
  const [parallel, setParallel] = useState(true);
  const [preview, setPreview] = useState<ImagegenGalleryEntry | null>(null);
  const [tierPickerOpen, setTierPickerOpen] = useState(false);
  // 存储位置迁移确认弹窗的待定目标（null = 不显示）。
  const [dirConfirm, setDirConfirm] = useState<{
    from: string;
    to: string;
  } | null>(null);

  // 选目录 → 弹迁移确认（搬迁并切换 / 直接切换）。取消选择（null）什么都不做。
  const handlePickOutputDir = async () => {
    const picked = await settingsApi.pickDirectory(outputDir.data?.path);
    if (!picked) return;
    if (outputDir.data && picked === outputDir.data.path) return;
    setDirConfirm({ from: outputDir.data?.path ?? "", to: picked });
  };

  const applyOutputDir = (migrate: boolean) => {
    if (!dirConfirm) return;
    const target = dirConfirm;
    setDirConfirm(null);
    setOutputDir.mutate(
      { path: target.to, migrate },
      {
        onSuccess: (result) =>
          toast.success(
            migrate
              ? t("loongport.imagegenPlayground.storageMovedToast", {
                  count: result.moved,
                })
              : t("loongport.imagegenPlayground.storageSwitchedToast"),
          ),
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  // 档位列表与「档位」视图同一条命令（listRelays 按栏查，结果天然同质）。
  const tiersQuery = useImageTiers();

  const tiers = useMemo(
    () => (tiersQuery.data ?? []).flatMap((row) => row.tiers),
    [tiersQuery.data],
  );
  const currentTier = tiers.find((tier) => tier.isCurrent);

  const switchTier = async (providerId: string, name: string) => {
    try {
      const result = await relayApi.switchTier(providerId, "codex-image");
      // confirmationRequired 是 codex 聊天档位独有的分支（要不要退 ChatGPT），
      // 生图档位不会走到；真走到也不该在这儿弹确认，交给用户去「档位」视图操作。
      if (result.status !== "confirmationRequired") {
        toast.success(t("loongport.switch.done", { name }));
        await queryClient.invalidateQueries({ queryKey: imagegenKeys.tiers });
      }
    } catch (e) {
      toast.error(String(e));
    }
  };

  const onGenerate = () => {
    // 输入框里的值可能还在半截（空串/越界），提交时夹回合法域 —— 真正的闸在后端。
    const parsed = Number.parseInt(count, 10);
    const safe = Number.isNaN(parsed) ? 1 : Math.min(50, Math.max(1, parsed));
    generate.mutate(
      { prompt: prompt.trim(), size, count: safe, parallel },
      {
        onSuccess: (result) => {
          // 部分失败：成功的图已落盘画廊，warning 说明有几张没成，别当整体失败。
          if (result.failed > 0) {
            toast.warning(
              t("loongport.imagegenPlayground.partialFailureToast", {
                count: result.images.length,
                failed: result.failed,
                model: result.model,
              }),
            );
          } else {
            toast.success(
              t("loongport.imagegenPlayground.generatedToast", {
                count: result.images.length,
                model: result.model,
              }),
            );
          }
        },
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  const entries = gallery.data ?? [];
  const busy = generate.isPending;

  return (
    <div className="space-y-4">
      {/* 档位快切：可输入筛选的 Combobox（形状抄 ModelPicker 的 Command-in-Popover）。
          行内带模型与倍率 —— 倍率是后端算好的事实，null（未知）不显示，绝不当 0。 */}
      <div className="flex flex-wrap items-center gap-2">
        <Label className="text-xs text-muted-foreground">
          {t("loongport.imagegenPlayground.tierLabel")}
        </Label>
        <Popover open={tierPickerOpen} onOpenChange={setTierPickerOpen}>
          <PopoverTrigger asChild>
            <Button
              variant="outline"
              role="combobox"
              aria-expanded={tierPickerOpen}
              disabled={tiers.length === 0}
              className="w-[280px] justify-between font-normal"
            >
              <span className="truncate">
                {currentTier?.displayName ??
                  t("loongport.imagegenPlayground.noTier")}
              </span>
              <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-[320px] p-0" align="start">
            <Command label={t("loongport.imagegenPlayground.tierLabel")}>
              <CommandInput
                placeholder={t(
                  "loongport.imagegenPlayground.tierFilterPlaceholder",
                )}
              />
              <CommandList>
                <CommandEmpty>
                  {t("loongport.imagegenPlayground.tierFilterEmpty")}
                </CommandEmpty>
                <CommandGroup>
                  {tiers.map((tier) => (
                    <CommandItem
                      key={tier.providerId}
                      value={tier.providerId}
                      keywords={[tier.displayName, tier.model]}
                      onSelect={() => {
                        setTierPickerOpen(false);
                        void switchTier(tier.providerId, tier.displayName);
                      }}
                    >
                      <Check
                        className={cn(
                          "mr-2 h-4 w-4 shrink-0",
                          currentTier?.providerId === tier.providerId
                            ? "opacity-100"
                            : "opacity-0",
                        )}
                      />
                      <span className="min-w-0 flex-1 truncate">
                        {tier.displayName}
                      </span>
                      <span className="ml-2 shrink-0 text-xs text-muted-foreground">
                        {tier.model}
                        {tier.rateMultiplier !== null &&
                          ` · ${t("loongport.tier.rate", { value: tier.rateMultiplier })}`}
                      </span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </div>

      {tiers.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {t("loongport.imagegenPlayground.noTierHint")}
        </p>
      ) : (
        <div className="space-y-2">
          <Textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder={t("loongport.imagegenPlayground.promptPlaceholder")}
            rows={3}
            className="resize-none"
          />
          <div className="flex flex-wrap items-center gap-2">
            <Select value={size} onValueChange={setSize}>
              <SelectTrigger
                className="w-[150px]"
                aria-label={t("loongport.imagegenPlayground.sizeLabel")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SIZE_OPTIONS.map((option) => (
                  <SelectItem key={option} value={option}>
                    {option}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {/* 批量张数：自由输入（1-50，后端闸）。一次点下去就是 n 张的钱，
                hint 把后果写在点上；提交节奏（并发/串行）由旁边的勾选框决定。 */}
            <Input
              type="number"
              inputMode="numeric"
              min={1}
              max={50}
              value={count}
              onChange={(e) => setCount(e.target.value)}
              onBlur={() => {
                // 失焦时夹回合法域：空/非法回落 1，越界夹到边界 ——
                // 真正的闸在后端，这里只是不把显然非法的值发出去。
                const parsed = Number.parseInt(count, 10);
                if (Number.isNaN(parsed)) setCount("1");
                else setCount(String(Math.min(50, Math.max(1, parsed))));
              }}
              aria-label={t("loongport.imagegenPlayground.countLabel")}
              title={t("loongport.imagegenPlayground.countHint")}
              className="w-[80px]"
            />
            {/* 并发提交：勾=多张拆并发单张（快）；不勾=小批量合并一条、大批量逐张
                （慢而稳，对站点最友好）。取舍归用户，语义唯源在后端 split_batch。 */}
            <label
              className="flex cursor-pointer select-none items-center gap-1.5 text-sm"
              title={t("loongport.imagegenPlayground.parallelHint")}
            >
              <Checkbox
                checked={parallel}
                onCheckedChange={(checked) => setParallel(checked === true)}
                aria-label={t("loongport.imagegenPlayground.parallelLabel")}
              />
              {t("loongport.imagegenPlayground.parallelLabel")}
            </label>
            <Button
              type="button"
              onClick={onGenerate}
              disabled={busy || !prompt.trim() || !currentTier}
              title={t("loongport.imagegenPlayground.generateHint")}
            >
              {busy ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Sparkles className="h-4 w-4" />
              )}
              {busy
                ? t("loongport.imagegenPlayground.generating")
                : t("loongport.imagegenPlayground.generate")}
            </Button>
          </div>
        </div>
      )}

      {/* 画廊：MCP 生成的图也落在这里 —— 两个入口的产物汇成同一份记录。
          存储位置就近展示与更改（路径由后端给出，前端只展示）。 */}
      <section className="space-y-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-medium">
            {t("loongport.imagegenPlayground.galleryTitle")}
          </h3>
          <div className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
            <span className="shrink-0">
              {t("loongport.imagegenPlayground.storageLabel")}
            </span>
            <span className="truncate font-mono" title={outputDir.data?.path}>
              {outputDir.data?.path ?? "…"}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-xs"
              onClick={handlePickOutputDir}
            >
              {t("loongport.imagegenPlayground.storageChange")}
            </Button>
          </div>
        </div>
        {gallery.isLoading ? (
          <p className="text-sm text-muted-foreground">
            {t("loongport.imagegenPlayground.galleryLoading")}
          </p>
        ) : entries.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {t("loongport.imagegenPlayground.galleryEmpty")}
          </p>
        ) : (
          <div className="grid grid-cols-3 gap-2 sm:grid-cols-4 md:grid-cols-6">
            {entries.map((entry) => (
              <button
                key={entry.path}
                type="button"
                className="group relative aspect-square overflow-hidden rounded-md border border-border bg-muted"
                onClick={() => setPreview(entry)}
              >
                <img
                  src={convertFileSrc(entry.path)}
                  alt={entry.name}
                  loading="lazy"
                  className="h-full w-full object-cover transition-transform group-hover:scale-105"
                />
              </button>
            ))}
          </div>
        )}
      </section>

      {/* 大图预览：形状与 SwitchTierConfirmDialog 同族（Header → 内容 → Footer）。 */}
      <Dialog
        open={preview != null}
        onOpenChange={(open) => {
          if (!open) setPreview(null);
        }}
      >
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle className="break-all text-sm">
              {preview?.name}
            </DialogTitle>
            <DialogDescription>
              {preview &&
                `${fmtBytes(preview.sizeBytes)} · ${new Date(
                  preview.modifiedAt * 1000,
                ).toLocaleString()}`}
            </DialogDescription>
          </DialogHeader>
          {preview && (
            <img
              src={convertFileSrc(preview.path)}
              alt={preview.name}
              className="max-h-[60vh] w-full rounded-md object-contain"
            />
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                if (preview) {
                  relayApi
                    .imagegenRevealImage(preview.path)
                    .catch((e) => toast.error(String(e)));
                }
              }}
            >
              <FolderOpen className="h-4 w-4" />
              {t("loongport.imagegenPlayground.reveal")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 存储位置迁移确认：形状与 SwitchTierConfirmDialog 同族（ghost→outline→主梯度）。
          不做每图索引 —— 文件系统是唯一事实源，历史靠这一次性搬迁保住。 */}
      <Dialog
        open={dirConfirm != null}
        onOpenChange={(open) => {
          if (!open) setDirConfirm(null);
        }}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("loongport.imagegenPlayground.storageDialog.title")}
            </DialogTitle>
            <DialogDescription>
              {t("loongport.imagegenPlayground.storageDialog.body")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-1 break-all text-xs">
            <p className="font-mono text-muted-foreground">
              {dirConfirm?.from}
            </p>
            <p className="font-mono">{dirConfirm?.to}</p>
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              onClick={() => setDirConfirm(null)}
            >
              {t("loongport.imagegenPlayground.storageDialog.cancel")}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={setOutputDir.isPending}
              onClick={() => applyOutputDir(false)}
            >
              {t("loongport.imagegenPlayground.storageDialog.switchOnly")}
            </Button>
            <Button
              type="button"
              disabled={setOutputDir.isPending}
              onClick={() => applyOutputDir(true)}
            >
              <FolderInput className="h-4 w-4" />
              {t("loongport.imagegenPlayground.storageDialog.moveAndSwitch")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
