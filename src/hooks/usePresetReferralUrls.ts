import { useQuery } from "@tanstack/react-query";

import { providersApi } from "@/lib/api";
import { presetReferralKeys } from "@/lib/query/presetReferrals";

/**
 * 预设第三方厂商的返佣注册链接覆盖（远端配置下发，key = host）。
 *
 * 与 `relay_list_sponsors` 同一条纪律：后端同步读缓存配置（不做网络往返），
 * 拿不到就是空 map——预设自带的中性链接兜底，读路径永不因它缺席而空转。
 */
export function usePresetReferralUrls(): Record<string, string> {
  const { data } = useQuery({
    queryKey: presetReferralKeys.all,
    queryFn: () => providersApi.presetReferralUrls(),
    staleTime: Infinity,
    gcTime: Infinity,
    retry: 1,
  });
  return data ?? {};
}
