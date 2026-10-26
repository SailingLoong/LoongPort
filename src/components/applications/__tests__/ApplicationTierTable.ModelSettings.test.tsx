import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApplicationTierTable } from "../ApplicationTierTable";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: {} }),
}));
vi.mock(
  "@/components/relay/model-verification/TierVerificationProvider",
  () => ({ useTierVerification: () => ({ isVerifying: () => false }) }),
);
vi.mock("@/components/relay/model-verification/TierVerifyButton", () => ({
  TierVerifyButton: () => null,
}));
vi.mock("@/components/relay/model-verification/TierVerdictChip", () => ({
  TierVerdictChip: () => null,
}));

const current: ApplicationConfiguration = {
  providerId: "loongport-relay-tier",
  name: "Relay tier",
  source: "relay",
  account: { kind: "relay", id: 7 },
  serviceName: "Relay",
  accountLabel: "Work",
  configurationName: "Current tier",
  model: "alias-model",
  presentation: {
    isCurrent: true,
    isInConfig: true,
    isDefaultModel: false,
  } as ApplicationConfiguration["presentation"],
  selection: { kind: "relay" },
  canSelect: true,
};
const props = {
  configurations: [current],
  tiers: [],
  orderedIds: [current.providerId],
  failoverEnabled: false,
  orderPending: false,
  search: "",
  accountFilter: null,
  modelFilter: null,
  additive: false,
  busy: false,
  orderBusy: false,
  sort: null,
  onSort: vi.fn(),
  onReorder: vi.fn(),
  onSelect: vi.fn(),
  onOpenAccount: vi.fn(),
  onBlockTier: vi.fn(),
  onResetTierErrors: vi.fn(),
};

describe("application model settings entry", () => {
  it("opens model and reasoning settings directly for the current relay tier", async () => {
    const onModelSettings = vi.fn();
    render(<ApplicationTierTable {...props} {...{ onModelSettings }} />);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.open" }),
    );
    expect(onModelSettings).toHaveBeenCalledWith(current);
    expect(props.onOpenAccount).not.toHaveBeenCalled();
  });

  it("does not enable model settings while the application owner is busy", () => {
    render(
      <ApplicationTierTable
        {...props}
        busy
        {...{ onModelSettings: vi.fn() }}
      />,
    );
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.open" }),
    ).toBeDisabled();
  });
});

// The shared DTO contains identities that the legacy numeric account route cannot open.
it.each<ApplicationConfiguration["account"]>([
  { kind: "nativeAccount", platform: "workbuddy", id: "synthetic-native" },
  {
    kind: "localProvider",
    platform: "openrouter",
    id: "synthetic-local",
    originApp: "codex",
    originProviderId: "origin",
  },
])(
  "does not send a shared source identity to the legacy account route: %j",
  (account) => {
    render(
      <ApplicationTierTable
        {...props}
        configurations={[{ ...current, account }]}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "applications.manageAccount" }),
    ).not.toBeInTheDocument();
  },
);
