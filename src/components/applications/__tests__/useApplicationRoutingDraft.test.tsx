import { act, renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useApplicationRoutingDraft } from "../useApplicationRoutingDraft";
import type { SwitchTierCommandResult } from "@/lib/api/relay";
const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: toastMocks }));
const changed = {
  order: { profileName: "Travel", providerIds: ["b"] },
  selection: { providerId: "b", model: "model-b" },
};
it("serializes duplicate submissions and suppresses confirmation after unmount", async () => {
  let resolve!: (value: SwitchTierCommandResult) => void;
  const apply = vi.fn(
    () =>
      new Promise<SwitchTierCommandResult>((done) => {
        resolve = done;
      }),
  );
  const { result, unmount } = renderHook(() =>
    useApplicationRoutingDraft(apply),
  );
  let request!: Promise<void>;
  act(() => {
    request = result.current.submit(changed);
  });
  expect(result.current.submitting).toBe(true);
  await act(async () => {
    await result.current.submit(changed);
  });
  expect(apply).toHaveBeenCalledTimes(1);
  unmount();
  await act(async () => {
    resolve({ status: "confirmationRequired", targetName: "Example" });
    await request;
  });
  expect(apply).toHaveBeenCalledTimes(1);
  expect(toastMocks.success).not.toHaveBeenCalled();
});
it("replays the frozen order and model intent after explicit confirmation", async () => {
  const apply = vi.fn().mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Example",
  });
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  await act(async () => {
    await result.current.submit(changed);
  });
  act(() => {
    result.current.load("Home", ["a"]);
  });
  await act(async () => {
    result.current.confirm(false);
  });
  expect(apply).toHaveBeenLastCalledWith(changed, false);
});
