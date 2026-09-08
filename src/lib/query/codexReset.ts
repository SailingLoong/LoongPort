import { useQuery } from "@tanstack/react-query";
import { codexResetApi } from "@/lib/api/codexReset";

/**
 * 全局重置预告。数据以天计（公告不定期），staleTime 放宽到 30 分钟——
 * 后端另有 1 小时缓存 + 旧缓存兜底，双保险下网络抖动对 UI 无感。
 * 查询失败返回 undefined（组件整体不渲染，宁缺毋认）。
 */
export const codexResetKeys = {
  all: ["codexReset"] as const,
};

export function useCodexResetFeed() {
  return useQuery({
    queryKey: codexResetKeys.all,
    queryFn: codexResetApi.getFeed,
    staleTime: 30 * 60 * 1000,
    retry: 1,
  });
}
