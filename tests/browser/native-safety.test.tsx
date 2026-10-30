import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeAll, afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
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
let Fixture: typeof import("./unified-accounts").NativeSafetyFixture;
beforeAll(async () => {
  history.replaceState(null, "", "/?scenario=native-safety&lang=en");
  document.body.innerHTML = '<div id="root"></div>';
  Fixture = (await import("./unified-accounts")).NativeSafetyFixture;
});
it("provides only synthetic read DTOs and still rejects every native write", () => {
  expect(boundary.invoke!("list_workbuddy_native_captures")).toMatchObject({
    accounts: [],
    activationAvailable: true,
    requestOutcome: "unknown",
  });
  expect(boundary.invoke!("inspect_workbuddy_native_recovery")).toEqual({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
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

afterEach(cleanup);
it("cancels real capture/recovery dialogs repeatedly without submitting IPC", async () => {
  const fixture = (window as any).unifiedLayout;
  fixture.rejectedCommands.length = 0;
  fixture.requestedCommands.length = 0;
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
  const t = (key: string) => i18n.t(key);
  const preview = screen.getByRole("region", {
    name: t("workbuddy.nativePreview.title"),
  });
  for (let repeat = 0; repeat < 2; repeat += 1) {
    await user.click(
      within(preview).getByRole("button", {
        name: t("workbuddy.nativeCapture.open"),
      }),
    );
    const capture = screen.getByRole("group", {
      name: t("workbuddy.nativeCapture.title"),
    });
    await user.type(within(capture).getByRole("textbox"), "Synthetic label");
    await user.click(
      within(capture).getByRole("button", {
        name: t("workbuddy.nativeCapture.cancel"),
      }),
    );
    expect(
      screen.queryByRole("group", { name: t("workbuddy.nativeCapture.title") }),
    ).not.toBeInTheDocument();
  }
  await user.click(
    within(preview).getByRole("button", {
      name: t("workbuddy.nativeCapture.open"),
    }),
  );
  await user.type(
    screen
      .getByRole("group", { name: t("workbuddy.nativeCapture.title") })
      .querySelector("input")!,
    "Synthetic interrupted capture",
  );
  await user.click(screen.getByRole("button", { name: "Leave safety view" }));
  expect(
    screen.queryByRole("group", { name: t("workbuddy.nativeCapture.title") }),
  ).not.toBeInTheDocument();
  await user.click(
    screen.getByRole("button", { name: "Return to safety view" }),
  );
  await user.click(
    within(
      screen.getByRole("group", { name: t("workbuddy.nativeCapture.title") }),
    ).getByRole("button", { name: t("workbuddy.nativeCapture.cancel") }),
  );
  const recovery = screen.getByRole("region", {
    name: t("workbuddy.nativeRecovery.title"),
  });
  await user.click(
    within(recovery).getByRole("button", {
      name: t("workbuddy.nativeRecovery.check"),
    }),
  );
  for (const dismissal of ["cancel", "escape"]) {
    await user.click(
      within(recovery).getByRole("button", {
        name: t("workbuddy.nativeRecovery.open"),
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: t("workbuddy.nativeRecovery.confirmTitle"),
    });
    if (dismissal === "cancel")
      await user.click(
        within(dialog).getByRole("button", {
          name: t("common.cancel"),
        }),
      );
    else await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  }
  expect(fixture.rejectedCommands).toEqual([]);
  expect(fixture.requestedCommands).toEqual([
    "list_workbuddy_native_captures",
    "inspect_workbuddy_native_recovery",
  ]);
});
it("forwards vendor presentation cancel/confirm without native transaction commands", async () => {
  const fixture = (window as any).unifiedLayout;
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
  const t = (key: string) => i18n.t(key);
  const before = fixture.requestedCommands.length;
  for (const dismissal of ["cancel", "escape", "confirm", "confirm"]) {
    await user.click(
      screen.getByRole("button", { name: "Review synthetic vendor" }),
    );
    const dialog = screen.getByRole("dialog", {
      name: t("applications.vendorRecovery.title"),
    });
    if (dismissal === "escape") await user.keyboard("{Escape}");
    else
      await user.click(
        within(dialog).getByRole("button", {
          name:
            dismissal === "cancel"
              ? t("common.cancel")
              : t("applications.vendorRecovery.confirm"),
        }),
      );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  }
  expect(fixture.safetyEvidence).toEqual({
    vendorConfirms: 2,
    vendorCancels: 2,
  });
  expect(fixture.requestedCommands.length).toBe(before);
});
