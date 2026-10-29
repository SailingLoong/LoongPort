import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RelayDirectoryConnectionPage } from "@/components/relay/onboarding/RelayDirectoryConnectionPage";
import type { RelaySetupReceipt } from "@/lib/api/relay";
import { createTestQueryClient } from "../utils/testQueryClient";
import wire from "../fixtures/relay-setup-wire.json";
const api = vi.hoisted(() => ({
  importSite: vi.fn(),
  refresh: vi.fn(),
  plazaSeedFromFirstSite: vi.fn(),
  switchTier: vi.fn(),
  listRelays: vi.fn(),
  discoverTiers: vi.fn(),
  getSetupOperation: vi.fn(),
  prepareTiers: vi.fn(),
  prepareApplicationConnection: vi.fn(),
  applyPreparedTier: vi.fn(),
}));
const onboarding = vi.hoisted(() => ({
  status: vi.fn(),
  dismiss: vi.fn(),
  complete: vi.fn(),
}));
vi.mock("@/lib/api", () => ({
  PLAZA_VISIBLE_DEFAULT: false,
  relayApi: api,
  settingsApi: api,
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: onboarding,
}));
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    settings: { plazaVisible: false, crowdMetricsEnabled: false },
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en" },
  }),
}));
beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  api.importSite.mockResolvedValue({ relayId: 7, siteName: "Example" });
  api.plazaSeedFromFirstSite.mockResolvedValue(false);
  api.discoverTiers.mockResolvedValue(wire.discovery);
  api.getSetupOperation.mockResolvedValue(null);
  onboarding.status.mockResolvedValue({ completed: true, shouldPrompt: false });
  onboarding.dismiss.mockResolvedValue({
    completed: true,
    shouldPrompt: false,
  });
});
describe("RelayDirectoryConnectionPage", () => {
  it("confirms only the selected configuration after login and preserves domain on Back/Escape until explicit finish", async () => {
    const user = userEvent.setup();
    const onBack = vi.fn();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelayDirectoryConnectionPage sourceAppId="codex" onBack={onBack} />
      </QueryClientProvider>,
    );
    await user.type(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
      "panel.example",
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.firstSite.confirm" }),
    );
    const choice = await screen.findByRole("combobox", { name: "Codex" });
    expect(choice).toHaveValue("");
    expect(api.discoverTiers).toHaveBeenCalledWith(7);
    expect(api.getSetupOperation).toHaveBeenCalledWith(7);
    expect(onBack).not.toHaveBeenCalled();
    expect(api.listRelays).not.toHaveBeenCalled();
    for (const effect of [
      api.refresh,
      api.prepareTiers,
      api.switchTier,
      onboarding.complete,
      onboarding.dismiss,
    ])
      expect(effect).not.toHaveBeenCalled();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "common.back" }));
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("panel.example");
    expect(onBack).not.toHaveBeenCalled();
    expect(onboarding.dismiss).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.resume" }),
    );
    await screen.findByRole("combobox", { name: "Codex" });
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("panel.example");
    expect(
      screen.getByRole("button", { name: "loongport.onboarding.resume" }),
    ).toBeVisible();
    expect(onBack).not.toHaveBeenCalled();
    expect(onboarding.dismiss).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.resume" }),
    );
    await user.selectOptions(
      await screen.findByRole("combobox", { name: "Codex" }),
      "a",
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.setup.review" }),
    );
    const prepare = screen.getByRole("button", {
      name: "loongport.setup.confirmPrepare",
    });
    expect(prepare).toBeDisabled();
    expect(api.prepareTiers).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("checkbox", { name: "loongport.setup.prepareAck" }),
    );
    let receipt: RelaySetupReceipt;
    api.prepareTiers.mockImplementation(async (request) => {
      receipt = {
        ...(structuredClone(wire.receipt) as RelaySetupReceipt),
        operationId: request.operationId,
        status: "settled",
        createdKeyCount: 1,
        selections: request.selections,
        groups: [
          { ...wire.receipt.groups[0], state: "ready", origin: "created" },
        ],
        targets: [
          {
            ...wire.receipt.targets[0],
            app: "codex",
            configuration: "prepared",
            compatibility: "supported",
            connection: { state: "notRequested" },
          },
        ],
      };
      return receipt;
    });
    await user.click(prepare);
    await screen.findByText("loongport.setup.prepared");
    expect(api.prepareTiers).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "start",
        relayId: 7,
        discoveryRevision: wire.discovery.revision,
        selections: [{ app: "codex", groupId: "a" }],
      }),
    );
    expect(api.applyPreparedTier).not.toHaveBeenCalled();
    expect(api.switchTier).not.toHaveBeenCalled();
    api.prepareApplicationConnection.mockImplementation(async () => {
      receipt = {
        ...receipt,
        receiptRevision: 2,
        targets: receipt.targets.map((target) => ({
          ...target,
          connection: {
            state: "confirmationRequired",
            requestId: "fixture-request",
          },
        })),
      };
      return { receipt, confirmation: null };
    });
    await user.click(
      screen.getByRole("button", { name: "loongport.setup.connectApp" }),
    );
    await screen.findByRole("heading", {
      name: "loongport.setup.connectionTitle",
    });
    expect(api.prepareApplicationConnection).toHaveBeenCalledWith(
      receipt!.operationId,
      "codex",
      1,
      undefined,
    );
    const connect = screen.getByRole("button", {
      name: "loongport.setup.confirmConnect",
    });
    expect(connect).toBeDisabled();
    expect(api.applyPreparedTier).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("checkbox", { name: "loongport.setup.connectionAck" }),
    );
    api.applyPreparedTier.mockImplementation(async () => {
      receipt = {
        ...receipt,
        receiptRevision: 3,
        targets: receipt.targets.map((target) => ({
          ...target,
          connection: {
            ...target.connection,
            state: "applied",
            writeAcknowledged: true,
            observedCurrent: true,
          },
        })),
      };
      return { receipt, result: { status: "switched" } };
    });
    await user.click(connect);
    await screen.findByText("loongport.setup.applied");
    expect(api.applyPreparedTier).toHaveBeenCalledWith(
      receipt!.operationId,
      "codex",
      2,
      false,
    );
    expect(onboarding.dismiss).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("button", { name: "loongport.setup.returnAccount" }),
    );
    await waitFor(() => expect(onBack).toHaveBeenCalledTimes(1));
    expect(onboarding.dismiss).toHaveBeenCalledTimes(1);
    expect(onboarding.complete).not.toHaveBeenCalled();
    expect(api.refresh).not.toHaveBeenCalled();
    expect(api.switchTier).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "loongport.onboarding.resume" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("");
  });
});
