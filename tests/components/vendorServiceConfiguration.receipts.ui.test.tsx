import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";

const mock = vi.hoisted(() => ({
  prepare: vi.fn(),
  query: vi.fn(),
  switchPrepared: vi.fn(),
  legacy: vi.fn(),
  previewNew: vi.fn(),
  confirmNew: vi.fn(),
  apply: vi.fn(),
  complete: vi.fn(),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    list: async (app: string) => ({
      accounts:
        app === "claude"
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
    }),
    prepareConnection: mock.prepare,
    queryConnection: mock.query,
    switchPreparedConnection: mock.switchPrepared,
    switch: mock.legacy,
    previewNewConnectionIntent: mock.previewNew,
    confirmNewConnectionIntent: mock.confirmNew,
  },
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

beforeEach(() => {
  vi.clearAllMocks();
  mock.query.mockImplementation(async (_app: string, id: string | null) =>
    id === null ? { status: "none" } : { status: "unknown" },
  );
  mock.prepare.mockImplementation(async (input: any) => ({
    ...input,
    providerId: "pa",
    receiptRevision: 1,
    status: "prepared",
    operationState: "prepared",
    completedConfigurationRevision: null,
  }));
  mock.switchPrepared.mockRejectedValue(new Error("response lost"));
  mock.previewNew.mockResolvedValue({ status: "unknown" });
});

it("shows an uncertain result and never dispatches a second write from the current UI", async () => {
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
  expect(mock.switchPrepared).toHaveBeenCalledOnce();
  const originalId = mock.prepare.mock.calls[0][0].operationId;
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.reapply" }),
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", {
        name: "loongport.onboarding.continueSetup",
      }),
    ).toBeEnabled(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() =>
    expect(
      mock.query.mock.calls.filter(([, id]) => id === originalId),
    ).toHaveLength(3),
  );
  expect(mock.prepare).toHaveBeenCalledOnce();
  expect(mock.switchPrepared).toHaveBeenCalledOnce();
  expect(mock.legacy).not.toHaveBeenCalled();
  expect(mock.apply).not.toHaveBeenCalled();
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});

it("shows the review before a new operation and preserves unknown after its lost response", async () => {
  mock.previewNew.mockImplementation(async (input: any) => ({
    ...input,
    status: "newIntentReviewRequired",
    receiptRevision: 1,
    originalRowId: 7,
    originalPlan: "a",
    targetProviderId: "pa",
    reviewToken: "a".repeat(64),
  }));
  mock.confirmNew.mockResolvedValue({
    status: "newIntentPermitted",
    app: "claude",
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
  const old = mock.prepare.mock.calls[0][0].operationId;
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.reapply" }),
  );
  expect(await screen.findByRole("dialog")).toBeInTheDocument();
  expect(mock.prepare).toHaveBeenCalledOnce();
  fireEvent.click(
    screen.getByRole("button", { name: "applications.vendorRecovery.confirm" }),
  );
  await waitFor(() => expect(mock.prepare).toHaveBeenCalledTimes(2));
  expect(mock.prepare.mock.calls[1][0].operationId).not.toBe(old);
  await screen.findByText("loongport.onboarding.results.unverified");
  expect(mock.switchPrepared).toHaveBeenCalledTimes(2);
  expect(mock.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});
