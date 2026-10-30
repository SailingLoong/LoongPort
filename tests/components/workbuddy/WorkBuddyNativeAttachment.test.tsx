import { beforeEach, expect, it, vi } from "vitest";
import {
  render as rtlRender,
  screen,
  fireEvent,
  act,
  cleanup,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const account = {
  id: "api-row",
  label: "Backend label",
  realm: "www.workbuddy.cn",
};
const ready = {
  flowId: "flow-a",
  available: true,
  state: "ready",
  account,
  saved: null,
  reason: null,
  expiresInMs: 1000,
};
const receipt = {
  ...ready,
  state: "saved",
  expiresInMs: 0,
  saved: {
    selection: {
      id: account.id,
      generation: 1,
      incarnation: "d54c6503-ae85-4ec5-b591-6393e9d41930",
    },
    label: account.label,
    realm: account.realm,
  },
};
let client: import("@tanstack/react-query").QueryClient;
let Provider: typeof import("@tanstack/react-query").QueryClientProvider;
const render = (ui: React.ReactElement) =>
  rtlRender(ui, {
    wrapper: ({ children }) => <Provider client={client}>{children}</Provider>,
  });
let Component: typeof import("@/components/workbuddy/WorkBuddyNativeAttachment").WorkBuddyNativeAttachment;
beforeEach(async () => {
  cleanup();
  vi.resetModules();
  vi.clearAllMocks();
  vi.mocked(invoke).mockReset();
  const query = await import("@tanstack/react-query");
  Provider = query.QueryClientProvider;
  client = new query.QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  Component = (await import("@/components/workbuddy/WorkBuddyNativeAttachment"))
    .WorkBuddyNativeAttachment;
});
const button = (name: string) =>
  screen.getByRole("button", { name: `workbuddy.attachment.${name}` });
it("requires backend-bound review, serializes double confirm, and reports attachment only", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    );
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  expect(screen.getByText("www.workbuddy.cn")).toBeVisible();
  fireEvent.click(button("confirm"));
  fireEvent.click(button("confirm"));
  expect(invoke).toHaveBeenCalledTimes(2);
  await act(async () => finish(receipt));
  expect(screen.getByText("workbuddy.attachment.saved")).toBeVisible();
  expect(screen.getByText("workbuddy.attachment.boundary")).toBeVisible();
});
it("retains a known flow over remount and queries it after a lost confirmation", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockRejectedValueOnce("lost")
    .mockResolvedValueOnce(receipt);
  const view = render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.unknown");
  view.unmount();
  render(<Component accountId="api-row" />);
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.saved");
  expect(invoke).toHaveBeenLastCalledWith(
    "inspect_workbuddy_native_attachment",
    { flowId: "flow-a" },
  );
});
it("does not attribute a discovered saved slot after prepare response is lost", async () => {
  const refreshed = vi.fn();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  vi.mocked(invoke)
    .mockRejectedValueOnce("lost")
    .mockResolvedValueOnce(receipt);
  render(<Component accountId="api-row" onAttached={refreshed} />);
  fireEvent.click(button("prepare"));
  await screen.findByText("workbuddy.attachment.unknown");
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.unowned");
  expect(
    screen.queryByText("workbuddy.attachment.saved"),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "workbuddy.attachment.confirm" }),
  ).not.toBeInTheDocument();
  expect(refreshed).not.toHaveBeenCalled();
  expect(invalidate).not.toHaveBeenCalled();
  expect(invoke).toHaveBeenLastCalledWith(
    "inspect_workbuddy_native_attachment",
    { flowId: null },
  );
});
it("old account callbacks cannot confirm another row and closing ready discards exact flow", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockResolvedValue({ ...ready, state: "cancelled", expiresInMs: 0 });
  const view = render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  const stale = button("confirm");
  view.rerender(<Component accountId="another-row" />);
  fireEvent.click(stale);
  expect(invoke).toHaveBeenCalledTimes(1);
  view.rerender(<Component accountId="api-row" />);
  fireEvent.click(button("close"));
  await screen.findByRole("button", { name: "workbuddy.attachment.prepare" });
  expect(invoke).toHaveBeenLastCalledWith(
    "cancel_workbuddy_native_attachment",
    { flowId: "flow-a" },
  );
});
it("closing during confirmation never calls rollback or claims cancellation", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    );
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  fireEvent.click(button("close"));
  expect(
    screen.queryByText("workbuddy.attachment.cancelled"),
  ).not.toBeInTheDocument();
  expect(invoke).toHaveBeenCalledTimes(2);
  await act(async () => finish(receipt));
  fireEvent.click(button("show"));
  expect(screen.getByText("workbuddy.attachment.saved")).toBeVisible();
});
it("can detach an unresolved confirmation and inspect the same flow without replay", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    )
    .mockResolvedValueOnce(receipt);
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  fireEvent.click(button("close"));
  fireEvent.click(button("show"));
  expect(button("inspect")).not.toBeDisabled();
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.saved");
  await act(async () => finish({ ...ready, state: "unknown", expiresInMs: 0 }));
  expect(screen.getByText("workbuddy.attachment.saved")).toBeVisible();
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("discards only the owned preparation when its response arrives after close", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    )
    .mockResolvedValueOnce({ ...ready, state: "cancelled", expiresInMs: 0 });
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  fireEvent.click(button("close"));
  await act(async () => finish(ready));
  expect(invoke).toHaveBeenLastCalledWith(
    "cancel_workbuddy_native_attachment",
    { flowId: "flow-a" },
  );
  expect(button("prepare")).toBeEnabled();
});
it("requires a new explicit review to use an unowned ready plan and preserves the lost request warning", async () => {
  vi.mocked(invoke)
    .mockRejectedValueOnce("lost")
    .mockResolvedValueOnce(ready)
    .mockResolvedValueOnce(receipt);
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("workbuddy.attachment.unknown");
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.unowned");
  expect(screen.getByText("Backend label")).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "workbuddy.attachment.confirm" }),
  ).not.toBeInTheDocument();
  fireEvent.click(button("reviewCurrent"));
  expect(
    screen.getByText("workbuddy.attachment.previousUnknown"),
  ).toBeVisible();
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.saved");
  expect(invoke).toHaveBeenLastCalledWith(
    "confirm_workbuddy_native_attachment",
    { flowId: "flow-a" },
  );
});
it("never replays confirmation when a same-flow read still reports ready after lost confirmation", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockRejectedValueOnce("lost")
    .mockResolvedValueOnce(ready);
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.unknown");
  fireEvent.click(button("inspect"));
  await act(async () => {});
  expect(
    screen.queryByRole("button", { name: "workbuddy.attachment.confirm" }),
  ).not.toBeInTheDocument();
  expect(screen.getByText("workbuddy.attachment.unknown")).toBeVisible();
  fireEvent.click(button("close"));
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("honors a newer redacted session invalidation over a late saved callback", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    )
    .mockResolvedValueOnce({
      ...ready,
      state: "unknown",
      account: null,
      reason: "sourceChanged",
      expiresInMs: 0,
    });
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  fireEvent.click(button("close"));
  fireEvent.click(button("show"));
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.unknown");
  await act(async () => finish(receipt));
  expect(
    screen.queryByText("workbuddy.attachment.saved"),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("Backend label")).not.toBeInTheDocument();
});
it("rejects a same-flow receipt bound to another account", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockResolvedValueOnce({
      ...receipt,
      account: { ...account, id: "other" },
      saved: {
        ...receipt.saved,
        selection: { ...receipt.saved.selection, id: "other" },
      },
    });
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.unknown");
  expect(
    screen.queryByText("workbuddy.attachment.saved"),
  ).not.toBeInTheDocument();
});

it("retires an unresolved prepare before another row starts without cancelling the backend", async () => {
  let finish!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          finish = r;
        }),
    )
    .mockResolvedValueOnce({
      ...ready,
      flowId: null,
      state: "unknown",
      account: null,
      expiresInMs: 0,
    })
    .mockResolvedValueOnce({
      ...ready,
      flowId: "flow-b",
      account: { ...account, id: "next", label: "Next" },
    });
  const view = render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  fireEvent.click(button("close"));
  fireEvent.click(button("show"));
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.unowned");
  expect(
    screen.getByText("workbuddy.attachment.stopTrackingHint"),
  ).toBeVisible();
  fireEvent.click(button("stopTracking"));
  view.rerender(<Component accountId="next" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Next");
  await act(async () => finish(ready));
  expect(screen.getByText("Next")).toBeVisible();
  expect(screen.queryByText("Backend label")).not.toBeInTheDocument();
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("refreshes the existing data owners once for an owned saved receipt, never discovery", async () => {
  const refreshed = vi.fn();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockResolvedValueOnce(receipt)
    .mockResolvedValueOnce(receipt);
  render(<Component accountId="api-row" onAttached={refreshed} />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.saved");
  expect(refreshed).toHaveBeenCalledTimes(1);
  fireEvent.click(button("inspect"));
  await act(async () => {});
  expect(refreshed).toHaveBeenCalledTimes(1);
  expect(invalidate).toHaveBeenCalledTimes(2);
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["accountPlatformPresence"],
  });
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["workbuddyNativeCaptures"],
  });
});

it("REVIEW: newer sourceChanged inspection must retire an older pending saved inspection", async () => {
  let older!: (x: unknown) => void;
  const refreshed = vi.fn();
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockRejectedValueOnce("confirmation transport lost")
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          older = r;
        }),
    )
    .mockResolvedValueOnce({
      ...ready,
      state: "unknown",
      account: null,
      reason: "sourceChanged",
      expiresInMs: 0,
    });
  render(<Component accountId="api-row" onAttached={refreshed} />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  fireEvent.click(button("confirm"));
  await screen.findByText("workbuddy.attachment.unknown");
  fireEvent.click(button("inspect"));
  fireEvent.click(button("close"));
  fireEvent.click(button("show"));
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.reason.sourceChanged");
  await act(async () => older(receipt));
  expect(
    screen.queryByText("workbuddy.attachment.saved"),
  ).not.toBeInTheDocument();
  expect(
    screen.getByText("workbuddy.attachment.reason.sourceChanged"),
  ).toBeVisible();
  expect(refreshed).not.toHaveBeenCalled();
});

it("REVIEW: retained same-account close callback cannot cancel the next reviewed flow", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce(ready)
    .mockResolvedValueOnce({ ...ready, state: "cancelled", expiresInMs: 0 })
    .mockResolvedValueOnce({ ...ready, flowId: "flow-b" })
    .mockResolvedValue({
      ...ready,
      flowId: "flow-b",
      state: "cancelled",
      expiresInMs: 0,
    });
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  const el = button("close") as unknown as Record<string, any>;
  const propsKey = Object.keys(el).find((k) => k.startsWith("__reactProps$"))!;
  const oldClose = el[propsKey].onClick;
  fireEvent.click(button("close"));
  await screen.findByRole("button", { name: "workbuddy.attachment.prepare" });
  fireEvent.click(button("prepare"));
  await screen.findByText("Backend label");
  await act(async () => oldClose());
  expect(invoke).toHaveBeenCalledTimes(3);
  expect(button("confirm")).toBeEnabled();
});
it("REVIEW: active sourceChanged discovery retires pending prepare capability", async () => {
  let older!: (x: unknown) => void;
  vi.mocked(invoke)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          older = r;
        }),
    )
    .mockResolvedValueOnce({
      ...ready,
      flowId: null,
      state: "unknown",
      account: null,
      reason: "sourceChanged",
      expiresInMs: 0,
    })
    .mockResolvedValueOnce({ ...ready, state: "cancelled", expiresInMs: 0 });
  render(<Component accountId="api-row" />);
  fireEvent.click(button("prepare"));
  fireEvent.click(button("close"));
  fireEvent.click(button("show"));
  fireEvent.click(button("inspect"));
  await screen.findByText("workbuddy.attachment.reason.sourceChanged");
  await act(async () => older(ready));
  expect(
    screen.getByText("workbuddy.attachment.reason.sourceChanged"),
  ).toBeVisible();
  expect(screen.queryByText("Backend label")).not.toBeInTheDocument();
});
