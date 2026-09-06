/**
 * 生图页「生成」视图的数据层：画廊查询、生成 mutation、MCP 注册开关 mutation。
 *
 * 档位列表不走这里 —— 它就是 `relayApi.listRelays("codex-image")`（与「档位」视图
 * 同一条命令、同一个事实），在组件里就地查询；启用切换复用 `relayApi.switchTier`。
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { relayApi } from "@/lib/api";

export const imagegenKeys = {
  all: ["imagegen"] as const,
  gallery: ["imagegen", "gallery"] as const,
  /** 生图档位列表（与「档位」视图同一条 `listRelays` 命令，一个缓存两处用）。 */
  tiers: ["imagegen", "tiers"] as const,
};

/** 生图页共用一份的档位列表：生成视图的快切、页级的空态判定都读它。 */
export const useImageTiers = () =>
  useQuery({
    queryKey: imagegenKeys.tiers,
    queryFn: () => relayApi.listRelays("codex-image"),
  });

/** 画廊清单。staleTime 短：生成成功后会主动失效，平时进入页面拿一次就够。 */
export const useImagegenGallery = () =>
  useQuery({
    queryKey: imagegenKeys.gallery,
    queryFn: () => relayApi.imagegenListImages(),
    staleTime: 30_000,
  });

/** App 内直接生图。成功后失效画廊（新图出现在最前面）。 */
export const useImagegenGenerate = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ prompt, size }: { prompt: string; size: string | null }) =>
      relayApi.imagegenGenerate(prompt, size),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: imagegenKeys.gallery });
    },
  });
};

/** 切「在 CLI 对话中提供生图工具（MCP）」。后端落设置并对齐注册，前端失效设置缓存。 */
export const useSetImagegenMcpEnabled = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) => relayApi.setImagegenMcpEnabled(enabled),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
};
