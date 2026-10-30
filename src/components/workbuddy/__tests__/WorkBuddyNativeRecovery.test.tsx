import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { Activity, StrictMode } from "react";
import { useWorkBuddyNativeRecoveryViewModel } from "../useWorkBuddyNativeRecoveryViewModel";
import { afterEach, expect, it, vi } from "vitest";
import { WorkBuddyNativeRecovery } from "../WorkBuddyNativeRecovery";
import { workbuddyApi } from "@/lib/api/workbuddy";

afterEach(() => vi.restoreAllMocks());

it("requires explicit inspection and confirmation before restoring, and reports receipt separately from fresh journal observation", async () => {
  const capability = vi
    .spyOn(workbuddyApi, "listNativeCaptures")
    .mockResolvedValue({
      accounts: [],
      requestOutcome: "unknown",
      activationAvailable: true,
    });
  const inspect = vi
    .spyOn(workbuddyApi, "inspectNativeRecovery")
    .mockResolvedValueOnce({
      phase: "pending",
      requestOutcome: "unknown",
      canRequestActivation: false,
    })
    .mockResolvedValueOnce({
      phase: "clear",
      requestOutcome: "unknown",
      canRequestActivation: true,
    });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("restored");
  render(<WorkBuddyNativeRecovery />);
  expect(inspect).not.toHaveBeenCalled();
  expect(recover).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() => expect(screen.getByText(/日志当前待恢复/)).toBeVisible());
  expect(capability).toHaveBeenCalledTimes(1);
  expect(recover).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "恢复认证文件" }));
  expect(recover).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "确认恢复" }));
  await waitFor(() =>
    expect(
      screen.getByText(/最近一次收到的恢复回执：原文件已恢复/),
    ).toBeVisible(),
  );
  expect(
    screen.getByText(/恢复后读取的当前日志快照：无待恢复项/),
  ).toBeVisible();
  expect(screen.getByText(/不证明此前未知请求的结果/)).toBeVisible();
  expect(inspect).toHaveBeenCalledTimes(2);
  expect(recover).toHaveBeenCalledTimes(1);
});

it("does not request recovery for unsupported capability or an absent journal", async () => {
  const catalog = vi
    .spyOn(workbuddyApi, "listNativeCaptures")
    .mockResolvedValueOnce({
      accounts: [],
      requestOutcome: "unknown",
      activationAvailable: false,
    })
    .mockResolvedValueOnce({
      accounts: [],
      requestOutcome: "unknown",
      activationAvailable: true,
    });
  const inspect = vi
    .spyOn(workbuddyApi, "inspectNativeRecovery")
    .mockResolvedValue({
      phase: "absent",
      requestOutcome: "unknown",
      canRequestActivation: true,
    });
  const recover = vi.spyOn(workbuddyApi, "recoverNative");
  render(<WorkBuddyNativeRecovery />);
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent(/不支持/),
  );
  expect(inspect).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() => expect(screen.getByText(/当前没有日志/)).toBeVisible());
  expect(catalog).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeDisabled();
  expect(recover).not.toHaveBeenCalled();
});

it("cancels without dispatch and consumes a confirmation once", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery").mockResolvedValue({
    phase: "clear",
    requestOutcome: "unknown",
    canRequestActivation: true,
  });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("alreadyApplied");
  render(<WorkBuddyNativeRecovery />);
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "恢复认证文件" }));
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(recover).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "恢复认证文件" }));
  fireEvent.click(screen.getByRole("button", { name: "确认恢复" }));
  await waitFor(() =>
    expect(screen.getByText(/已提交的目标文件仍匹配/)).toBeVisible(),
  );
  expect(recover).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeDisabled();
});

it("keeps an unknown recovery outcome unconfirmed without automatic replay", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  const inspect = vi
    .spyOn(workbuddyApi, "inspectNativeRecovery")
    .mockResolvedValue({
      phase: "pending",
      requestOutcome: "unknown",
      canRequestActivation: false,
    });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockRejectedValue(new Error("private secret"));
  const view = render(<WorkBuddyNativeRecovery />);
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "恢复认证文件" }));
  fireEvent.click(screen.getByRole("button", { name: "确认恢复" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent(/结果未确认/),
  );
  expect(screen.getByRole("alert")).not.toHaveTextContent(/private secret/);
  expect(inspect).toHaveBeenCalledTimes(1);
  expect(recover).toHaveBeenCalledTimes(1);
  view.unmount();
  render(<WorkBuddyNativeRecovery />);
  expect(screen.getByRole("alert")).toHaveTextContent(/结果未确认/);
  expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeDisabled();
  expect(recover).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeEnabled(),
  );
  expect(screen.getByText(/此前恢复结果仍未确认/)).toBeVisible();
  expect(recover).toHaveBeenCalledTimes(1);
});

it("retains a known receipt if the separate post-recovery inspection fails", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery")
    .mockResolvedValueOnce({
      phase: "pending",
      requestOutcome: "unknown",
      canRequestActivation: false,
    })
    .mockRejectedValueOnce(new Error("private"));
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("restored");
  render(<WorkBuddyNativeRecovery />);
  fireEvent.click(screen.getByRole("button", { name: "检查恢复状态" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "恢复认证文件" }));
  fireEvent.click(screen.getByRole("button", { name: "确认恢复" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent(/后续日志检查失败/),
  );
  expect(
    screen.getByText(/最近一次收到的恢复回执：原文件已恢复/),
  ).toBeVisible();
  expect(recover).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "恢复认证文件" })).toBeDisabled();
});

it("retired confirmation callbacks cannot cancel or dispatch a later confirmation", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery").mockResolvedValue({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("restored");
  const view = renderHook(() => useWorkBuddyNativeRecoveryViewModel());
  await act(async () => {
    await view.result.current.check();
  });
  act(() => view.result.current.open());
  const oldRecover = view.result.current.recover;
  const oldCancel = view.result.current.cancel;
  act(() => oldCancel());
  act(() => view.result.current.open());
  act(() => oldCancel());
  expect(view.result.current.confirming).toBe(true);
  await act(async () => {
    await oldRecover();
  });
  expect(recover).not.toHaveBeenCalled();
  await act(async () => {
    await view.result.current.recover();
  });
  await act(async () => {
    await oldRecover();
  });
  expect(recover).toHaveBeenCalledTimes(1);
});

it("hiding retires the confirmation and requires a fresh explicit check when shown", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery").mockResolvedValue({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("restored");
  let current!: ReturnType<typeof useWorkBuddyNativeRecoveryViewModel>;
  function Probe() {
    current = useWorkBuddyNativeRecoveryViewModel();
    return null;
  }
  const view = render(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  await act(async () => {
    await current.check();
  });
  act(() => current.open());
  const old = current.recover;
  view.rerender(
    <Activity mode="hidden">
      <Probe />
    </Activity>,
  );
  await act(async () => {
    await old();
  });
  expect(recover).not.toHaveBeenCalled();
  view.rerender(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  expect(current.confirming).toBe(false);
  expect(current.canRecover).toBe(false);
  await act(async () => {
    await current.check();
  });
  expect(current.canRecover).toBe(true);
});

it("a remounted view cannot dispatch while recovery runs and receives the worker receipt", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery").mockResolvedValue({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
  let finish!: (value: "restored") => void;
  const recover = vi.spyOn(workbuddyApi, "recoverNative").mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const first = renderHook(() => useWorkBuddyNativeRecoveryViewModel());
  await act(async () => {
    await first.result.current.check();
  });
  act(() => first.result.current.open());
  act(() => {
    void first.result.current.recover();
  });
  first.unmount();
  const second = renderHook(() => useWorkBuddyNativeRecoveryViewModel());
  expect(second.result.current.busy).toBe(true);
  await act(async () => {
    await second.result.current.check();
  });
  expect(recover).toHaveBeenCalledTimes(1);
  await act(async () => {
    finish("restored");
  });
  expect(second.result.current.receipt).toBe("restored");
  expect(second.result.current.canRecover).toBe(false);
  expect(recover).toHaveBeenCalledTimes(1);
});

it("accepts a fresh confirmation under React StrictMode", async () => {
  vi.spyOn(workbuddyApi, "listNativeCaptures").mockResolvedValue({
    accounts: [],
    requestOutcome: "unknown",
    activationAvailable: true,
  });
  vi.spyOn(workbuddyApi, "inspectNativeRecovery").mockResolvedValue({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
  const recover = vi
    .spyOn(workbuddyApi, "recoverNative")
    .mockResolvedValue("restored");
  const view = renderHook(() => useWorkBuddyNativeRecoveryViewModel(), {
    wrapper: StrictMode,
  });
  await act(async () => {
    await view.result.current.check();
  });
  act(() => view.result.current.open());
  expect(view.result.current.confirming).toBe(true);
  await act(async () => {
    await view.result.current.recover();
  });
  expect(recover).toHaveBeenCalledTimes(1);
});
