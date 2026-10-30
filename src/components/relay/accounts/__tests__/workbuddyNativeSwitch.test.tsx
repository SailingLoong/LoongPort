import { Activity, StrictMode } from "react";
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AccountOverviewRow } from "../AccountOverviewRow";
import { useWorkBuddyNativeSwitch } from "../useWorkBuddyNativeSwitch";
import { workbuddyApi, WorkBuddyNativeSwitchError } from "@/lib/api/workbuddy";
import type { AccountRowVM } from "../accountOverviewProjection";

const selection = {
  id: "synthetic",
  generation: 1,
  incarnation: "d54c6503-ae85-4ec5-b591-6393e9d41930",
};
const row: AccountRowVM = {
  rowKey: "row",
  nativeIdentity: { status: "ready", selection },
  groupId: "workbuddy",
  accountId: "synthetic",
  name: "Synthetic",
  facts: [],
  lastSuccess: "—",
  nextExpiry: "—",
  earliestKnownExpiryMs: null,
  balance: null,
  availability: "unknown",
  claimIssue: null,
  detailTarget: { platform: "workbuddy", id: "synthetic" },
  switchAction: { enabled: true, label: "文件切换", reason: "" },
};

it("requires explicit confirmation, refuses duplicates and reports restart independently", async () => {
  let finish!: (value: { transaction: "applied"; restart: "unknown" }) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  render(<AccountOverviewRow row={row} onOpenDetails={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  expect(call).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  expect(call).toHaveBeenCalledTimes(1);
  expect(call).toHaveBeenCalledWith(selection);
  finish({ transaction: "applied", restart: "unknown" });
  await waitFor(() => expect(screen.getByText(/启动结果未知/)).toBeVisible());
  expect(screen.queryByText(/账号已生效|当前账号/)).not.toBeInTheDocument();
  call.mockRestore();
});

it("never invokes on cancellation or when capability is false", () => {
  const call = vi.spyOn(workbuddyApi, "activateNativeWithRestart");
  const { rerender } = render(
    <AccountOverviewRow row={row} onOpenDetails={() => {}} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(call).not.toHaveBeenCalled();
  rerender(
    <AccountOverviewRow
      row={{ ...row, switchAction: { ...row.switchAction, enabled: false } }}
      onOpenDetails={() => {}}
    />,
  );
  expect(screen.getByRole("button", { name: "文件切换" })).toBeDisabled();
  call.mockRestore();
});

it("does not show an old receipt after a target changes away and back", async () => {
  let finish!: (value: {
    transaction: "applied";
    restart: "requested";
  }) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  const { rerender } = render(
    <AccountOverviewRow row={row} onOpenDetails={() => {}} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  const other = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, generation: 2 },
    },
  } as AccountRowVM;
  rerender(<AccountOverviewRow row={other} onOpenDetails={() => {}} />);
  rerender(<AccountOverviewRow row={row} onOpenDetails={() => {}} />);
  finish({ transaction: "applied", restart: "requested" });
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "文件切换" })).toBeEnabled(),
  );
  expect(screen.queryByText(/已请求启动/)).not.toBeInTheDocument();
  call.mockRestore();
});

it("reports a refused quit distinctly without claiming a completed transaction", async () => {
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockRejectedValue(new WorkBuddyNativeSwitchError("quitRefused"));
  render(<AccountOverviewRow row={row} onOpenDetails={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent(/客户端拒绝退出/),
  );
  expect(screen.queryByText(/认证文件已切换/)).not.toBeInTheDocument();
  call.mockRestore();
});

it("does not replay an unknown switch outcome, including after remount", async () => {
  const selected = { ...selection, id: "uncertain" };
  const candidate = {
    ...row,
    nativeIdentity: { status: "ready", selection: selected },
    accountId: "uncertain",
  } as AccountRowVM;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockRejectedValue(new WorkBuddyNativeSwitchError("outcomeUnknown"));
  const view = render(
    <AccountOverviewRow row={candidate} onOpenDetails={() => {}} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent(/切换结果未知/),
  );
  expect(screen.getByRole("button", { name: "文件切换" })).toBeDisabled();
  view.unmount();
  render(<AccountOverviewRow row={candidate} onOpenDetails={() => {}} />);
  expect(screen.getByRole("button", { name: "文件切换" })).toBeDisabled();
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("distinguishes an already-applied transaction from a new write", async () => {
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockResolvedValue({ transaction: "alreadyApplied", restart: "notNeeded" });
  render(<AccountOverviewRow row={row} onOpenDetails={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: "文件切换" }));
  fireEvent.click(screen.getByRole("button", { name: "确认切换" }));
  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent(
      /认证文件已是目标内容/,
    ),
  );
  expect(screen.getByRole("status")).toHaveTextContent(/未启动/);
  call.mockRestore();
});

it("revokes an old confirmation callback synchronously on cancel, including after reopening", async () => {
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockResolvedValue({ transaction: "applied", restart: "notNeeded" });
  const { result } = renderHook(() => useWorkBuddyNativeSwitch(row));
  act(() => result.current.open());
  const cancelledConfirm = result.current.confirm;
  act(() => result.current.cancel());
  await act(async () => {
    await cancelledConfirm();
  });
  expect(call).not.toHaveBeenCalled();
  act(() => result.current.open());
  expect(result.current.confirming).toBe(true);
  await act(async () => {
    await cancelledConfirm();
  });
  expect(call).not.toHaveBeenCalled();
  expect(result.current.confirming).toBe(true);
  await act(async () => {
    await result.current.confirm();
  });
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("consumes a confirmation once even after a successful response", async () => {
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockResolvedValue({ transaction: "applied", restart: "notNeeded" });
  const { result } = renderHook(() => useWorkBuddyNativeSwitch(row));
  act(() => result.current.open());
  const confirm = result.current.confirm;
  await act(async () => {
    await confirm();
  });
  await act(async () => {
    await confirm();
  });
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("revokes a retained confirmation when React Activity is hidden", async () => {
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockResolvedValue({ transaction: "applied", restart: "notNeeded" });
  let current!: ReturnType<typeof useWorkBuddyNativeSwitch>;
  function Probe() {
    current = useWorkBuddyNativeSwitch(row);
    return null;
  }
  const view = render(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  act(() => current.open());
  const confirm = current.confirm;
  view.rerender(
    <Activity mode="hidden">
      <Probe />
    </Activity>,
  );
  await act(async () => {
    await confirm();
  });
  expect(call).not.toHaveBeenCalled();
  view.rerender(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  expect(current.confirming).toBe(false);
  call.mockRestore();
});

it("updates a replacement row when a detached pending operation finishes", async () => {
  const candidate = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "pending-remount" },
    },
  } as AccountRowVM;
  let finish!: (value: {
    transaction: "applied";
    restart: "notNeeded";
  }) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  const first = renderHook(() => useWorkBuddyNativeSwitch(candidate));
  act(() => first.result.current.open());
  act(() => {
    void first.result.current.confirm();
  });
  first.unmount();
  const second = renderHook(() => useWorkBuddyNativeSwitch(candidate));
  expect(second.result.current.canSwitch).toBe(false);
  await act(async () => {
    finish({ transaction: "applied", restart: "notNeeded" });
  });
  expect(second.result.current.canSwitch).toBe(true);
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("updates a replacement row when a detached operation becomes unknown", async () => {
  const candidate = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "unknown-remount" },
    },
  } as AccountRowVM;
  let fail!: (reason: Error) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject;
        }),
    );
  const first = renderHook(() => useWorkBuddyNativeSwitch(candidate));
  act(() => first.result.current.open());
  act(() => {
    void first.result.current.confirm();
  });
  first.unmount();
  const second = renderHook(() => useWorkBuddyNativeSwitch(candidate));
  await act(async () => {
    fail(new WorkBuddyNativeSwitchError("outcomeUnknown"));
  });
  expect(second.result.current.canSwitch).toBe(false);
  expect(second.result.current.error).toMatchObject({
    stage: "outcomeUnknown",
  });
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("does not let an old open callback revive an A-to-B-to-A confirmation", () => {
  const b = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "aba-b" },
    },
  } as AccountRowVM;
  const view = renderHook(({ value }) => useWorkBuddyNativeSwitch(value), {
    initialProps: { value: row },
  });
  const oldOpen = view.result.current.open;
  view.rerender({ value: b });
  view.rerender({ value: row });
  act(() => oldOpen());
  expect(view.result.current.confirming).toBe(false);
});

it("does not show a detached lifetime receipt after Activity hide and reveal", async () => {
  const candidate = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "activity-receipt" },
    },
  } as AccountRowVM;
  let finish!: (value: {
    transaction: "applied";
    restart: "requested";
  }) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  let current!: ReturnType<typeof useWorkBuddyNativeSwitch>;
  function Probe() {
    current = useWorkBuddyNativeSwitch(candidate);
    return null;
  }
  const view = render(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  act(() => current.open());
  act(() => {
    void current.confirm();
  });
  view.rerender(
    <Activity mode="hidden">
      <Probe />
    </Activity>,
  );
  view.rerender(
    <Activity mode="visible">
      <Probe />
    </Activity>,
  );
  await act(async () => {
    finish({ transaction: "applied", restart: "requested" });
  });
  expect(current.report).toBeNull();
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("admits a fresh explicit confirmation under React StrictMode", async () => {
  const candidate = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "strict-mode" },
    },
  } as AccountRowVM;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockResolvedValue({ transaction: "applied", restart: "notNeeded" });
  const view = renderHook(() => useWorkBuddyNativeSwitch(candidate), {
    wrapper: StrictMode,
  });
  act(() => view.result.current.open());
  expect(view.result.current.confirming).toBe(true);
  await act(async () => {
    await view.result.current.confirm();
  });
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});

it("does not queue another target while a switch is pending", async () => {
  const a = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "concurrent-a" },
    },
  } as AccountRowVM;
  const b = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "concurrent-b" },
    },
  } as AccountRowVM;
  let finish!: (value: {
    transaction: "applied";
    restart: "notNeeded";
  }) => void;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  const first = renderHook(() => useWorkBuddyNativeSwitch(a));
  const second = renderHook(() => useWorkBuddyNativeSwitch(b));
  act(() => first.result.current.open());
  act(() => {
    void first.result.current.confirm();
  });
  expect(second.result.current.canSwitch).toBe(false);
  act(() => second.result.current.open());
  await act(async () => {
    await second.result.current.confirm();
  });
  expect(call).toHaveBeenCalledTimes(1);
  await act(async () => {
    finish({ transaction: "applied", restart: "notNeeded" });
  });
  expect(second.result.current.canSwitch).toBe(true);
  call.mockRestore();
});

it("does not let an old cancel dismiss a later confirmation", () => {
  const view = renderHook(() => useWorkBuddyNativeSwitch(row));
  act(() => view.result.current.open());
  const oldCancel = view.result.current.cancel;
  act(() => oldCancel());
  act(() => view.result.current.open());
  expect(view.result.current.confirming).toBe(true);
  act(() => oldCancel());
  expect(view.result.current.confirming).toBe(true);
});

it("fences new switches after a transaction requires recovery", async () => {
  const a = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "recovery-a" },
    },
  } as AccountRowVM;
  const b = {
    ...row,
    nativeIdentity: {
      status: "ready",
      selection: { ...selection, id: "recovery-b" },
    },
  } as AccountRowVM;
  const call = vi
    .spyOn(workbuddyApi, "activateNativeWithRestart")
    .mockRejectedValue(
      new WorkBuddyNativeSwitchError("transaction", "recoveryRequired"),
    );
  const first = renderHook(() => useWorkBuddyNativeSwitch(a));
  const second = renderHook(() => useWorkBuddyNativeSwitch(b));
  act(() => first.result.current.open());
  await act(async () => {
    await first.result.current.confirm();
  });
  expect(first.result.current.canSwitch).toBe(false);
  expect(second.result.current.canSwitch).toBe(false);
  expect(second.result.current.error).toMatchObject({
    stage: "transaction",
    reason: "recoveryRequired",
  });
  render(<AccountOverviewRow row={b} onOpenDetails={() => {}} />);
  expect(screen.getByRole("alert")).toHaveTextContent(/报告异常/);
  expect(screen.getByRole("alert")).not.toHaveTextContent(/事务未完成/);
  const unrelated = renderHook(() =>
    useWorkBuddyNativeSwitch({
      ...row,
      groupId: "zcode",
      nativeIdentity: { status: "unavailable", selection: null },
    } as AccountRowVM),
  );
  expect(unrelated.result.current.error).toBeNull();
  act(() => second.result.current.open());
  await act(async () => {
    await second.result.current.confirm();
  });
  expect(call).toHaveBeenCalledTimes(1);
  call.mockRestore();
});
