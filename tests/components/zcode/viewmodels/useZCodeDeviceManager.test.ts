import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useZCodeDeviceManager } from "@/components/zcode/viewmodels/useZCodeDeviceManager";
import { zcodeDeviceApi } from "@/lib/api/zcodeDevices";
vi.mock("@/lib/api/zcodeDevices", () => ({
  zcodeDeviceApi: { state: vi.fn(), import: vi.fn(), select: vi.fn() },
}));
const initial = {
  devices: [],
  selectedDeviceMid: null,
  effectiveDeviceMid: null,
  restartRequired: false,
  revision: 3,
};
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(zcodeDeviceApi.state).mockResolvedValue(initial);
});
it("owns revision recovery without replaying a rejected write", async () => {
  const { result } = renderHook(() =>
    useZCodeDeviceManager("/synthetic/library"),
  );
  await waitFor(() => expect(result.current.state?.revision).toBe(3));
  act(() => result.current.setInput("draft"));
  vi.mocked(zcodeDeviceApi.import).mockRejectedValue(
    new Error("lost response"),
  );
  vi.mocked(zcodeDeviceApi.state).mockResolvedValue({
    ...initial,
    revision: 4,
  });
  await act(() => result.current.importDevices());
  expect(result.current.state?.revision).toBe(4);
  expect(result.current.input).toBe("draft");
  expect(result.current.error).toBe(true);
  expect(zcodeDeviceApi.import).toHaveBeenCalledTimes(1);
  expect(zcodeDeviceApi.import).toHaveBeenCalledWith(
    "/synthetic/library",
    "draft",
    3,
  );
});
it("retires a write when the source changes", async () => {
  let finish!: (value: typeof initial) => void;
  vi.mocked(zcodeDeviceApi.import).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { result, rerender } = renderHook(
    ({ root }) => useZCodeDeviceManager(root),
    { initialProps: { root: "/synthetic/one" } },
  );
  await waitFor(() => expect(result.current.state).not.toBeNull());
  act(() => result.current.setInput("draft"));
  let pending!: Promise<void>;
  act(() => {
    pending = result.current.importDevices();
  });
  rerender({ root: "/synthetic/two" });
  await act(async () => {
    finish({ ...initial, revision: 99 });
    await pending;
  });
  expect(result.current.state?.revision).toBe(3);
  expect(result.current.input).toBe("");
  expect(result.current.pending).toBe(false);
});
