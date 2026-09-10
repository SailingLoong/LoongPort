import { useQueryClient } from "@tanstack/react-query";
import { PROVIDER_MODELS_UPDATED } from "@/lib/api/events";
import type { AppId } from "@/lib/api/types";
import { useTauriEvent } from "./useTauriEvent";

/** Reflect completed backend inventory repairs in cached views. */
export function useProviderModelsCacheBridge() {
  const client = useQueryClient();
  useTauriEvent<{ appType: AppId }>(PROVIDER_MODELS_UPDATED, ({ appType }) => {
    for (const key of [
      "providers",
      "applicationOverview",
      "easyModeTierBoard",
    ]) {
      void client.invalidateQueries({ queryKey: [key, appType] });
    }
    void client.invalidateQueries({ queryKey: ["serviceAccounts"] });
  });
}
