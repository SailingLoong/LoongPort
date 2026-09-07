import { useQueryClient } from "@tanstack/react-query";

import { RELAY_DIRECTORY_UPDATED_EVENT } from "@/config/constants";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";

import { useTauriEvent } from "./useTauriEvent";

/** 广场数据在后台被更新（快照追新 / transit 周期刷新）时作废重拉，广场关着也保持缓存新鲜。 */
export function useRelayDirectoryCacheBridge() {
  const queryClient = useQueryClient();

  useTauriEvent<unknown>(RELAY_DIRECTORY_UPDATED_EVENT, () => {
    queryClient.invalidateQueries({
      queryKey: relayDirectoryKeys.listing(),
      exact: true,
    });
  });
}
