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
import { convertFileSrc } from "@tauri-apps/api/core";
import { FolderOpen, Loader2, Sparkles } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { relayApi, type ImagegenGalleryEntry } from "@/lib/api";
import {
  imagegenKeys,
  useImageTiers,
  useImagegenGallery,
  useImagegenGenerate,
} from "@/lib/query/imagegen";

/** 尺寸档位：gpt-image 的三档（与 MCP 工具的 size 语义一致，不给「自动」）。 */
const SIZE_OPTIONS = ["1024x1024", "1536x1024", "1024x1536"] as const;

export function ImagegenPlayground() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const gallery = useImagegenGallery();
  const generate = useImagegenGenerate();
  const [prompt, setPrompt] = useState("");
  const [size, setSize] = useState<string>("1024x1024");
  const [preview, setPreview] = useState<ImagegenGalleryEntry | null>(null);

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
    generate.mutate(
      { prompt: prompt.trim(), size },
      {
        onSuccess: (result) =>
          toast.success(
            t("loongport.imagegenPlayground.generatedToast", {
              count: result.images.length,
              model: result.model,
            }),
          ),
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  const entries = gallery.data ?? [];
  const busy = generate.isPending;

  return (
    <div className="space-y-4">
      {/* 档位快切：显示名是「站点 · 分组」，模型随档位走，不提供独立模型下拉。 */}
      <div className="flex flex-wrap items-center gap-2">
        <Label
          htmlFor="imagegen-tier-select"
          className="text-xs text-muted-foreground"
        >
          {t("loongport.imagegenPlayground.tierLabel")}
        </Label>
        <Select
          value={currentTier?.providerId ?? ""}
          onValueChange={(providerId) => {
            const tier = tiers.find(
              (candidate) => candidate.providerId === providerId,
            );
            if (tier) void switchTier(tier.providerId, tier.displayName);
          }}
          disabled={tiers.length === 0}
        >
          <SelectTrigger id="imagegen-tier-select" className="w-[280px]">
            <SelectValue
              placeholder={t("loongport.imagegenPlayground.noTier")}
            />
          </SelectTrigger>
          <SelectContent>
            {tiers.map((tier) => (
              <SelectItem key={tier.providerId} value={tier.providerId}>
                {tier.displayName}
                <span className="ml-1.5 text-xs text-muted-foreground">
                  {tier.model}
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
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

      {/* 画廊：MCP 生成的图也落在这里 —— 两个入口的产物汇成同一份记录。 */}
      <section className="space-y-2">
        <h3 className="text-sm font-medium">
          {t("loongport.imagegenPlayground.galleryTitle")}
        </h3>
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
    </div>
  );
}

/** 字节数的展示格式化（纯展示，本地就够了）。 */
function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
