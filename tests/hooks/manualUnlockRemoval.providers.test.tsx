import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { providersApi } from "@/lib/api";
import { useProvidersQuery } from "@/lib/query/queries";

it("preserves an unreadable credential error instead of claiming an empty account inventory", async () => {
  const failure = new Error("secret.locked");
  const read = vi.spyOn(providersApi, "getAll").mockRejectedValue(failure);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result, unmount } = renderHook(() => useProvidersQuery("claude"), {
    wrapper,
  });
  await waitFor(() => expect(result.current.isFetched).toBe(true));
  expect(read).toHaveBeenCalledWith("claude");
  expect.soft(result.current.isError).toBe(true);
  expect.soft(result.current.error).toBe(failure);
  expect.soft(result.current.data).toBeUndefined();
  unmount();
  client.clear();
  read.mockRestore();
});

it.each([
  ["genuinely empty", {}],
  [
    "existing accounts",
    { saved: { id: "saved", name: "Existing account", settingsConfig: {} } },
  ],
] as const)(
  "keeps a successful %s inventory successful",
  async (_, providers) => {
    const read = vi.spyOn(providersApi, "getAll").mockResolvedValue(providers);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, unmount } = renderHook(() => useProvidersQuery("claude"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.error).toBeNull();
    expect(result.current.data).toEqual({ providers });
    unmount();
    client.clear();
    read.mockRestore();
  },
);

it("retains last successful rows while exposing a refetch failure", async () => {
  const providers = {
    saved: { id: "saved", name: "Existing account", settingsConfig: {} },
  };
  const failure = new Error("secret.locked");
  const read = vi
    .spyOn(providersApi, "getAll")
    .mockResolvedValueOnce(providers)
    .mockRejectedValue(failure);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result, unmount } = renderHook(() => useProvidersQuery("claude"), {
    wrapper,
  });
  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  await act(async () => {
    await result.current.refetch();
  });
  await waitFor(() => expect(result.current.isFetching).toBe(false));
  expect.soft(result.current.isError).toBe(true);
  expect.soft(result.current.error).toBe(failure);
  expect.soft(result.current.data).toEqual({ providers });
  unmount();
  client.clear();
  read.mockRestore();
});
