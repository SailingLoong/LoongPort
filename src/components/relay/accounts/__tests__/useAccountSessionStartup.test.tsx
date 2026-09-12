import { StrictMode } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { useAccountSessionStartup } from "../useAccountSessionStartup";
const api = vi.hoisted(() => ({ checkSession: vi.fn(), info: vi.fn() }));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
vi.mock("sonner", () => ({ toast: { info: api.info } }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
beforeEach(() => vi.clearAllMocks());
it("probes once at startup under StrictMode and not again on rerender", async () => {
  api.checkSession.mockResolvedValue([1]);
  const client = new QueryClient();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const { rerender } = renderHook(() => useAccountSessionStartup(), {
    wrapper: ({ children }) => (
      <StrictMode>
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      </StrictMode>
    ),
  });
  await waitFor(() =>
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["serviceAccounts"] }),
  );
  rerender();
  expect(api.checkSession).toHaveBeenCalledTimes(1);
  expect(api.info).toHaveBeenCalledTimes(1);
});
it("keeps a failed probe quiet without repeating maintenance", async () => {
  api.checkSession.mockRejectedValue(new Error("offline"));
  const client = new QueryClient();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const { rerender } = renderHook(() => useAccountSessionStartup(), {
    wrapper: ({ children }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
  await waitFor(() => expect(api.checkSession).toHaveBeenCalledTimes(1));
  rerender();
  expect(api.info).not.toHaveBeenCalled();
  expect(invalidate).not.toHaveBeenCalled();
  expect(api.checkSession).toHaveBeenCalledTimes(1);
});
