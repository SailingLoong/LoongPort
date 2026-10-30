import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";

const mock = vi.hoisted(() => ({
  invoke: vi.fn(),
  apply: vi.fn(),
  complete: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { apply: mock.apply },
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: {
    status: async () => ({ completed: false }),
    complete: mock.complete,
  },
}));

it("uses the real vendor API decoder and original operation ID after a lost switch reply", async () => {
  mock.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "vendor_list_accounts")
      return {
        accounts:
          args.appId === "claude"
            ? [
                {
                  id: 7,
                  plans: [
                    {
                      planId: "a",
                      providerId: "pa",
                      planName: "A",
                      canSwitch: true,
                    },
                  ],
                },
              ]
            : [],
      };
    if (command === "vendor_query_connection")
      return args.operationId === null
        ? { status: "none" }
        : { status: "unknown", operationId: args.operationId, app: args.app };
    if (command === "vendor_prepare_connection")
      return {
        operationId: args.operationId,
        rowId: args.rowId,
        plan: args.plan,
        app: args.app,
        providerId: "pa",
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
      };
    if (command === "vendor_switch") throw new Error("response lost");
    throw new Error(`unexpected command ${command}`);
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Vendor" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("combobox", { name: "Claude Code" });
  fireEvent.click(
    await screen.findByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("loongport.onboarding.results.unverified");
  const prepares = () =>
    mock.invoke.mock.calls.filter(
      ([name]) => name === "vendor_prepare_connection",
    );
  const switches = () =>
    mock.invoke.mock.calls.filter(([name]) => name === "vendor_switch");
  const queries = () =>
    mock.invoke.mock.calls.filter(
      ([name]) => name === "vendor_query_connection",
    );
  expect(prepares()).toHaveLength(1);
  expect(switches()).toHaveLength(1);
  const operationId = prepares()[0][1].operationId;
  expect(switches()[0][1]).toMatchObject({
    rowId: 7,
    plan: "a",
    app: "claude",
    operationId,
    expectedRevision: 1,
    quitChatgpt: null,
  });
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() =>
    expect(
      queries().filter(([, args]) => args.operationId === operationId),
    ).toHaveLength(2),
  );
  expect(prepares()).toHaveLength(1);
  expect(switches()).toHaveLength(1);
  expect(mock.apply).not.toHaveBeenCalled();
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});

it("does not dispatch when the real decoder sees an unexpected prepare protection field", async () => {
  mock.invoke.mockReset();
  mock.apply.mockReset();
  mock.complete.mockReset();
  mock.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "vendor_list_accounts")
      return {
        accounts:
          args.appId === "claude"
            ? [
                {
                  id: 7,
                  plans: [
                    {
                      planId: "a",
                      providerId: "pa",
                      planName: "A",
                      canSwitch: true,
                    },
                  ],
                },
              ]
            : [],
      };
    if (command === "vendor_query_connection")
      return args.operationId === null
        ? { status: "none" }
        : { status: "unknown", operationId: args.operationId, app: args.app };
    if (command === "vendor_prepare_connection")
      return {
        operationId: args.operationId,
        rowId: args.rowId,
        plan: args.plan,
        app: args.app,
        providerId: "pa",
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
        quitRequired: true,
      };
    throw new Error(`unexpected command ${command}`);
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Vendor" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("combobox", { name: "Claude Code" });
  fireEvent.click(
    await screen.findByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("loongport.onboarding.results.unverified");
  const calls = mock.invoke.mock.calls;
  const operationId = calls.find(
    ([name]) => name === "vendor_prepare_connection",
  )![1].operationId;
  expect(
    calls.some(
      ([name, args]) =>
        name === "vendor_query_connection" && args.operationId === operationId,
    ),
  ).toBe(true);
  expect(calls.some(([name]) => name === "vendor_switch")).toBe(false);
  expect(mock.apply).not.toHaveBeenCalled();
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});

it("shows the native quit confirmation returned by the real adapter without a second write", async () => {
  mock.invoke.mockReset();
  mock.apply.mockReset();
  mock.complete.mockReset();
  mock.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "vendor_list_accounts")
      return {
        accounts:
          args.appId === "claude"
            ? [
                {
                  id: 7,
                  plans: [
                    {
                      planId: "a",
                      providerId: "pa",
                      planName: "A",
                      canSwitch: true,
                    },
                  ],
                },
              ]
            : [],
      };
    if (command === "vendor_query_connection")
      return args.operationId === null
        ? { status: "none" }
        : { status: "unknown", operationId: args.operationId, app: args.app };
    if (command === "vendor_prepare_connection")
      return {
        operationId: args.operationId,
        rowId: args.rowId,
        plan: args.plan,
        app: args.app,
        providerId: "pa",
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
      };
    if (command === "vendor_switch")
      return {
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: true,
      };
    throw new Error(`unexpected command ${command}`);
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Vendor" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("combobox", { name: "Claude Code" });
  fireEvent.click(
    await screen.findByRole("button", { name: "loongport.onboarding.finish" }),
  );
  expect(await screen.findByRole("dialog")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(
    mock.invoke.mock.calls.filter(([name]) => name === "vendor_switch"),
  ).toHaveLength(1);
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});

it("reconciles the same operation after an explicitly confirmed quit loses its reply", async () => {
  mock.invoke.mockReset();
  mock.apply.mockReset();
  mock.complete.mockReset();
  mock.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "vendor_list_accounts")
      return {
        accounts:
          args.appId === "claude"
            ? [
                {
                  id: 7,
                  plans: [
                    {
                      planId: "a",
                      providerId: "pa",
                      planName: "A",
                      canSwitch: true,
                    },
                  ],
                },
              ]
            : [],
      };
    if (command === "vendor_query_connection")
      return args.operationId === null
        ? { status: "none" }
        : { status: "unknown", operationId: args.operationId, app: args.app };
    if (command === "vendor_prepare_connection")
      return {
        operationId: args.operationId,
        rowId: args.rowId,
        plan: args.plan,
        app: args.app,
        providerId: "pa",
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
      };
    if (command === "vendor_switch" && args.quitChatgpt === true)
      throw new Error("confirmed response lost");
    if (command === "vendor_switch")
      return {
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: true,
      };
    throw new Error(`unexpected command ${command}`);
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Vendor" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("combobox", { name: "Claude Code" });
  fireEvent.click(
    await screen.findByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByRole("dialog");
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.quitConfirm.quitAndSwitch" }),
  );
  expect(
    await screen.findByText("loongport.onboarding.results.unverified"),
  ).toBeInTheDocument();
  expect(
    mock.invoke.mock.calls.filter(([name]) => name === "vendor_switch"),
  ).toHaveLength(2);
  const switches = mock.invoke.mock.calls.filter(
    ([name]) => name === "vendor_switch",
  );
  expect(switches[1][1]).toEqual({ ...switches[0][1], quitChatgpt: true });
  expect(
    mock.invoke.mock.calls.filter(
      ([name]) => name === "vendor_prepare_connection",
    ),
  ).toHaveLength(1);
  expect(mock.invoke).toHaveBeenCalledWith("vendor_query_connection", {
    app: "claude",
    operationId: switches[0][1].operationId,
  });
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});
