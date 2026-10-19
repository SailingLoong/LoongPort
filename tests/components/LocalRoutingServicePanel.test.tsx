import type { ComponentProps } from "react";
import type { ProxyPanel } from "@/components/proxy";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Accordion } from "@/components/ui/accordion";
import { LocalRoutingServicePanel } from "@/components/settings/LocalRoutingServicePanel";
import type { SettingsFormState } from "@/hooks/useSettings";
const runtime = vi.hoisted(() => ({
  startProxyServer: vi.fn(),
  stopWithRestore: vi.fn(),
  isRunning: false,
  isPending: false,
}));
vi.mock("@/hooks/useProxyStatus", () => ({ useProxyStatus: () => runtime }));
// Isolate the large runtime panel, preserving its actual callback/pending contract.
vi.mock("@/components/proxy", () => ({
  ProxyPanel: (
    props: Pick<
      ComponentProps<typeof ProxyPanel>,
      "isProxyPending" | "onToggleProxy"
    >,
  ) => (
    <>
      <button
        disabled={props.isProxyPending}
        onClick={() => props.onToggleProxy(true)}
      >
        Start
      </button>
      <button
        disabled={props.isProxyPending}
        onClick={() => props.onToggleProxy(false)}
      >
        Stop
      </button>
    </>
  ),
}));
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
}
function mount(
  save: (updates: Partial<SettingsFormState>) => Promise<boolean | void>,
  confirmed = false,
) {
  return render(
    <Accordion type="single" defaultValue="localRouting">
      <LocalRoutingServicePanel
        settings={{ proxyConfirmed: confirmed } as SettingsFormState}
        onAutoSave={save}
      />
    </Accordion>,
  );
}
function open() {
  fireEvent.click(screen.getByRole("button", { name: "Start", hidden: true }));
}
function confirm() {
  fireEvent.click(
    screen.getByRole("button", { name: "confirm.proxy.confirm" }),
  );
}
afterEach(() => vi.restoreAllMocks());
beforeEach(() => {
  runtime.isRunning = false;
  runtime.isPending = false;
  runtime.startProxyServer.mockReset().mockResolvedValue(undefined);
  runtime.stopWithRestore.mockReset().mockResolvedValue(undefined);
});
it("does not start when saving returns false and preserves confirmation for retry", async () => {
  const save = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  mount(save);
  open();
  confirm();
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(runtime.startProxyServer).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  confirm();
  await waitFor(() => expect(runtime.startProxyServer).toHaveBeenCalledOnce());
});
it("does not start on rejected save and allows retry", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const save = vi
    .fn()
    .mockRejectedValueOnce(new Error("synthetic save failure"))
    .mockResolvedValue(true);
  mount(save);
  open();
  confirm();
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(runtime.startProxyServer).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  confirm();
  await waitFor(() => expect(runtime.startProxyServer).toHaveBeenCalledOnce());
});
it("keeps both controls pending across save and start, admitting one chain", async () => {
  const saveResult = deferred<boolean>();
  const startResult = deferred<void>();
  runtime.startProxyServer.mockReturnValue(startResult.promise);
  const save = vi.fn(() => saveResult.promise);
  mount(save);
  open();
  const button = screen.getByRole("button", { name: "confirm.proxy.confirm" });
  act(() => {
    button.click();
    button.click();
  });
  expect(save).toHaveBeenCalledOnce();
  expect(button).toBeDisabled();
  expect(
    screen.getByRole("button", { name: "Start", hidden: true }),
  ).toBeDisabled();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  await act(async () => saveResult.resolve(true));
  expect(runtime.startProxyServer).toHaveBeenCalledOnce();
  expect(
    screen.getByRole("button", { name: "Start", hidden: true }),
  ).toBeDisabled();
  await act(async () => startResult.resolve());
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Start", hidden: true }),
  ).not.toBeDisabled();
});
it.each([true, undefined])(
  "starts once after a successful save (%s)",
  async (result) => {
    mount(vi.fn().mockResolvedValue(result));
    open();
    confirm();
    await waitFor(() =>
      expect(runtime.startProxyServer).toHaveBeenCalledOnce(),
    );
  },
);
it("retains successful confirmation after start failure without automatic retry", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  runtime.startProxyServer.mockRejectedValueOnce(
    new Error("synthetic start failure"),
  );
  const save = vi.fn().mockResolvedValue(true);
  const view = mount(save);
  open();
  confirm();
  await waitFor(() => expect(runtime.startProxyServer).toHaveBeenCalledOnce());
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(save).toHaveBeenCalledOnce();
  expect(
    screen.getByRole("button", { name: "Start", hidden: true }),
  ).not.toBeDisabled();
  view.rerender(
    <Accordion type="single" defaultValue="localRouting">
      <LocalRoutingServicePanel
        settings={{ proxyConfirmed: true } as SettingsFormState}
        onAutoSave={save}
      />
    </Accordion>,
  );
  open();
  await waitFor(() =>
    expect(runtime.startProxyServer).toHaveBeenCalledTimes(2),
  );
  expect(save).toHaveBeenCalledOnce();
});

it("serializes direct start and renders only query-owned running state", async () => {
  const startResult = deferred<void>();
  runtime.startProxyServer.mockReturnValue(startResult.promise);
  const save = vi.fn().mockResolvedValue(true);
  const view = mount(save, true);
  const button = screen.getByRole("button", { name: "Start" });
  act(() => {
    button.click();
    button.click();
  });
  expect(runtime.startProxyServer).toHaveBeenCalledOnce();
  expect(save).not.toHaveBeenCalled();
  await act(async () => startResult.resolve());
  expect(
    screen.getByText("settings.advanced.proxy.stopped"),
  ).toBeInTheDocument();
  runtime.isRunning = true;
  view.rerender(
    <Accordion type="single" defaultValue="localRouting">
      <LocalRoutingServicePanel
        settings={{ proxyConfirmed: true } as SettingsFormState}
        onAutoSave={save}
      />
    </Accordion>,
  );
  expect(
    screen.getByText("settings.advanced.proxy.running"),
  ).toBeInTheDocument();
  runtime.isRunning = false;
});

it("admits one direct stop and blocks overlapping start", async () => {
  const stopResult = deferred<void>();
  runtime.stopWithRestore.mockReturnValue(stopResult.promise);
  const save = vi.fn().mockResolvedValue(true);
  mount(save, true);
  const stop = screen.getByRole("button", { name: "Stop" });
  const start = screen.getByRole("button", { name: "Start" });
  act(() => {
    stop.click();
    stop.click();
    start.click();
  });
  expect(runtime.stopWithRestore).toHaveBeenCalledOnce();
  expect(runtime.startProxyServer).not.toHaveBeenCalled();
  expect(save).not.toHaveBeenCalled();
  expect(stop).toBeDisabled();
  await act(async () => stopResult.resolve());
  expect(stop).not.toBeDisabled();
});
