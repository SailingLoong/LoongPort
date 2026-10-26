import type { ReactNode } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { useSettingsQuery } from "@/lib/query/queries";
import { settingsApi } from "@/lib/api";
import type { FrontendSettings } from "@/lib/api/settings";
import { getSettings } from "../msw/state";
it("does not reuse a read-only bootstrap projection as complete editable settings after readiness", async () => {
  const bootstrap: FrontendSettings = {
    showInTray: true,
    minimizeToTrayOnClose: true,
    visibleApps: getSettings().visibleApps!,
  };
  const complete: FrontendSettings = {
    ...getSettings(),
    visibleApps: getSettings().visibleApps!,
    language: "en",
  };
  const bootstrapRead = vi
    .spyOn(settingsApi, "getBootstrap")
    .mockResolvedValueOnce(bootstrap);
  let resolve!: (value: typeof complete) => void;
  const read = vi.spyOn(settingsApi, "get").mockImplementationOnce(
    () =>
      new Promise((yes) => {
        resolve = yes;
      }),
  );
  const save = vi.spyOn(settingsApi, "save");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result, rerender, unmount } = renderHook(
    ({ ready }) => useSettingsQuery({ bootstrap: !ready }),
    { initialProps: { ready: false }, wrapper },
  );
  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect.soft(client.getQueryData(["settings"])).toBeUndefined();
  expect.soft(client.getQueryData(["bootstrapSettings"])).toEqual(bootstrap);
  rerender({ ready: true });
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  expect(result.current.data).toBeUndefined();
  expect(client.getQueryData(["settings"])).toBeUndefined();
  expect(save).not.toHaveBeenCalled();
  resolve(complete);
  await waitFor(() => expect(result.current.data?.language).toBe("en"));
  expect(read).toHaveBeenCalledTimes(1);
  expect(bootstrapRead).toHaveBeenCalledTimes(1);
  expect(client.getQueryData(["settings"])).toEqual(complete);
  expect(save).not.toHaveBeenCalled();
  unmount();
  client.clear();
  read.mockRestore();
  bootstrapRead.mockRestore();
  save.mockRestore();
});
