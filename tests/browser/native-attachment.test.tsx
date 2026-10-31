import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeAll, afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "i18next";
const boundary = vi.hoisted(() => ({
  invoke: undefined as
    undefined | ((command: string, payload?: unknown) => unknown),
}));
vi.mock("@tauri-apps/api/mocks", () => ({
  mockIPC: (handler: typeof boundary.invoke) => {
    boundary.invoke = handler;
  },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (command: string, payload?: unknown) =>
    boundary.invoke!(command, payload),
}));
vi.mock("react-dom/client", async (original) => {
  const actual = await original<typeof import("react-dom/client")>();
  return {
    ...actual,
    createRoot: (
      container: Element | DocumentFragment,
      options?: Parameters<typeof actual.createRoot>[1],
    ) =>
      container instanceof Element && container.id === "root"
        ? { render: vi.fn() }
        : actual.createRoot(container, options),
  };
});
let Fixture: typeof import("./unified-accounts").AttachmentFixture;
beforeAll(async () => {
  history.replaceState(null, "", "/?scenario=native-attachment&lang=en");
  document.body.innerHTML = '<div id="root"></div>';
  Fixture = (await import("./unified-accounts")).AttachmentFixture;
});
afterEach(cleanup);
it("reviews explicitly, confirms once, and rejects stale saved after unknown and selection change", async () => {
  expect(
    Fixture,
    "browser harness must mount the real attachment component",
  ).toBeTypeOf("function");
  const user = userEvent.setup();
  render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <Fixture />
    </QueryClientProvider>,
  );
  const label = (key: string) => i18n.t(`workbuddy.attachment.${key}`);
  await user.click(screen.getByRole("button", { name: label("prepare") }));
  expect(await screen.findByText(label("ready"))).toBeInTheDocument();
  expect(screen.getByText("Synthetic attachment A")).toBeInTheDocument();
  await user.dblClick(screen.getByRole("button", { name: label("confirm") }));
  expect(await screen.findByText(label("saved"))).toBeInTheDocument();
  expect(screen.getByTestId("attachment-refreshed")).toHaveTextContent("1");
  const controls = (window as any).unifiedLayout.attachment;
  expect(controls.confirms).toBe(1);
  controls.defer = true;
  await user.click(
    screen.getByRole("button", {
      name: "Select synthetic attachment B",
    }),
  );
  await user.click(screen.getByRole("button", { name: label("prepare") }));
  await screen.findByText(label("ready"));
  await user.click(screen.getByRole("button", { name: label("confirm") }));
  // The first response is lost; inspection explicitly reports unknown.
  controls.loseResponse();
  await screen.findByText(label("unknown"));
  await user.click(screen.getByRole("button", { name: label("inspect") }));
  await screen.findByText(label("unknown"));
  await user.click(
    screen.getByRole("button", {
      name: "Select synthetic attachment A",
    }),
  );
  controls.releaseLateSaved();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(screen.queryByText(label("saved"))).not.toBeInTheDocument();
  expect(screen.getByTestId("attachment-refreshed")).toHaveTextContent("1");
  expect(controls.confirms).toBe(2);
  for (const command of [
    "capture_workbuddy_native_account",
    "activate_workbuddy_native_account",
    "activate_workbuddy_native_with_restart",
    "recover_workbuddy_native_switch",
  ]) {
    expect(() => boundary.invoke!(command)).toThrow(
      "Unexpected synthetic fixture command",
    );
  }
});

it("does not cancel Applying or overwrite completed receipts", async () => {
  const { attachment, attachmentIPC } = await import("./attachment-fixture");
  const call = (command: string, payload: unknown) =>
    attachmentIPC(`${command}_workbuddy_native_attachment`, payload) as any;
  attachment.defer = true;
  const ready = call("prepare", { id: "attachment-a" });
  const pending = call("confirm", { flowId: ready.flowId }) as Promise<unknown>;
  const rejected = expect(pending).rejects.toThrow("Synthetic lost response");
  const afterApplyingCancel = call("cancel", { flowId: ready.flowId });
  attachment.loseResponse();
  await rejected;
  expect(afterApplyingCancel.state).toBe("applying");
  expect(call("cancel", { flowId: ready.flowId }).state).toBe("unknown");
  attachment.defer = false;
  const next = call("prepare", { id: "attachment-b" });
  const saved = call("confirm", { flowId: next.flowId });
  expect(call("cancel", { flowId: next.flowId })).toEqual(saved);
  const cancellable = call("prepare", { id: "attachment-a" });
  expect(call("cancel", { flowId: cancellable.flowId }).state).toBe(
    "cancelled",
  );
});
