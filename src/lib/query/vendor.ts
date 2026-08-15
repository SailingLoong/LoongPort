import { useQuery } from "@tanstack/react-query";

import { vendorApi } from "@/lib/api/vendor";
import type { AppId } from "@/lib/api/types";

export const vendorKeys = {
  supported: (appId: AppId) => ["vendorSupported", appId] as const,
};

/**
 * 当前 app 是否支持官网直连账号（厂商 preset 覆盖到哪个平台）。
 *
 * 事实 owner 是后端 —— `vendor_list_accounts` 返回的 `supported`，与
 * `VendorBlock` 整块的出现条件同一条判据，前端不自己维护厂商支持列表。
 * 该标志由编译进二进制的 preset 表决定，一次会话内不会变 ⇒ 不设失效。
 */
export function useVendorSupportedQuery(appId: AppId) {
  const { data } = useQuery({
    queryKey: vendorKeys.supported(appId),
    queryFn: () => vendorApi.list(appId),
    select: (result) => result.supported,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  return data ?? false;
}
