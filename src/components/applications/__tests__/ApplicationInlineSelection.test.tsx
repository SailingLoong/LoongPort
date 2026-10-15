import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type { ApplicationRoutingTier } from "@/lib/api/applicationRouting";
import { ApplicationInlineSelection } from "../ApplicationInlineSelection";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const configuration = (
  id: string,
  current = false,
): ApplicationConfiguration => ({
  providerId: id,
  name: id,
  source: "relay",
  account: { kind: "relay", id: 1 },
  serviceName: "Synthetic source",
  accountLabel: "Synthetic account",
  configurationName: id,
  model: null,
  presentation: {
    isCurrent: current,
    isInConfig: true,
    isDefaultModel: false,
    isOfficial: false,
    isManaged: true,
    isReadOnly: true,
    canDelete: false,
    canEdit: false,
    canTestConnectivity: false,
    canConfigureUsage: false,
    usesOfficialSubscriptionUsage: false,
    canSetAsDefault: false,
    routingBadge: null,
    routingReason: null,
    switchBlockedReason: null,
  },
  selection: { kind: "relay" },
  canSelect: true,
});
const configurations = [configuration("first", true), configuration("second")];
const tiers = [
  {
    providerId: "first",
    models: ["first-model"],
    effectiveModel: "first-model",
  },
  {
    providerId: "second",
    models: ["second-model", "alternate"],
    effectiveModel: "second-model",
  },
] as ApplicationRoutingTier[];

describe("ApplicationInlineSelection", () => {
  it("stages only an existing binding and its model before requesting review", async () => {
    const user = userEvent.setup();
    const onReview = vi.fn();
    render(
      <ApplicationInlineSelection
        configurations={configurations}
        tiers={tiers}
        currentProviderId="first"
        currentModel="first-model"
        disabled={false}
        onReview={onReview}
      />,
    );
    await user.click(screen.getByLabelText("applications.source"));
    await user.click(screen.getByRole("option", { name: /second/ }));
    expect(
      screen.queryByRole("option", { name: "first-model" }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByLabelText(
        "applications.currentConfiguration · applications.modelFilter",
      ),
    );
    await user.click(screen.getByRole("option", { name: "alternate" }));
    expect(onReview).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("button", { name: "applications.reviewChanges" }),
    );
    expect(onReview).toHaveBeenCalledWith({
      providerId: "second",
      model: "alternate",
    });
  });

  it("blocks stale models, removed bindings and owner-blocked state without applying", async () => {
    const user = userEvent.setup();
    const onReview = vi.fn();
    const props = {
      configurations,
      tiers,
      currentProviderId: "first",
      currentModel: "first-model",
      disabled: false,
      onReview,
    };
    const view = render(<ApplicationInlineSelection {...props} />);
    await user.click(screen.getByLabelText("applications.source"));
    await user.click(screen.getByRole("option", { name: /second/ }));
    await user.click(
      screen.getByLabelText(
        "applications.currentConfiguration · applications.modelFilter",
      ),
    );
    await user.click(screen.getByRole("option", { name: "alternate" }));
    view.rerender(
      <ApplicationInlineSelection
        {...props}
        tiers={[tiers[0], { ...tiers[1], models: ["second-model"] }]}
      />,
    );
    expect(screen.getByRole("button")).toBeDisabled();
    view.rerender(
      <ApplicationInlineSelection
        {...props}
        configurations={[configurations[0]]}
      />,
    );
    expect(screen.getByRole("button")).toBeDisabled();
    view.rerender(<ApplicationInlineSelection {...props} disabled />);
    expect(screen.getByLabelText("applications.source")).toBeDisabled();
    expect(
      screen.getByLabelText(
        "applications.currentConfiguration · applications.modelFilter",
      ),
    ).toBeDisabled();
    expect(onReview).not.toHaveBeenCalled();
  });

  it("excludes unavailable bindings and avoids a no-op review", () => {
    render(
      <ApplicationInlineSelection
        configurations={[
          configurations[0],
          { ...configurations[1], canSelect: false },
        ]}
        tiers={tiers}
        currentProviderId="first"
        currentModel="first-model"
        disabled={false}
        onReview={vi.fn()}
      />,
    );
    expect(
      screen.queryByRole("option", { name: /second/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button")).toBeDisabled();
  });
});

it("allows clearing the current explicit model through the default choice", async () => {
  const user = userEvent.setup();
  const onReview = vi.fn();
  render(
    <ApplicationInlineSelection
      configurations={configurations}
      tiers={tiers}
      currentProviderId="first"
      currentModel="first-model"
      disabled={false}
      onReview={onReview}
    />,
  );
  await user.click(
    screen.getByLabelText(
      "applications.currentConfiguration · applications.modelFilter",
    ),
  );
  await user.click(
    screen.getByRole("option", { name: "applications.default" }),
  );
  await user.click(
    screen.getByRole("button", { name: "applications.reviewChanges" }),
  );
  expect(onReview).toHaveBeenCalledWith({ providerId: "first" });
});
