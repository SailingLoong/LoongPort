import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
vi.mock("@/components/providers/forms/CopilotAuthSection", () => ({
  CopilotAuthSection: () => <div>copilot-owner</div>,
}));
vi.mock("@/components/providers/forms/CodexOAuthSection", () => ({
  CodexOAuthSection: () => <div>codex-owner</div>,
}));
vi.mock("@/components/providers/forms/MiniMaxCodeSection", () => ({
  MiniMaxCodeSection: () => <div>minimax-owner</div>,
}));
vi.mock("@/components/providers/forms/XaiOAuthSection", () => ({
  XaiOAuthSection: () => <div>xai-owner</div>,
}));
vi.mock("@/components/workbuddy/WorkBuddyAccounts", () => ({
  WorkBuddyAccounts: () => <div>workbuddy-owner</div>,
}));
vi.mock("@/components/trae/TraeAccounts", () => ({
  TraeAccounts: () => <div>trae-owner</div>,
}));
vi.mock("@/components/zcode/ZCodeAccountPanel", () => ({
  ZCodeAccountPanel: () => <div>zcode-owner</div>,
}));
vi.mock("@/components/relay/RestoreOfficialLoginButton", () => ({
  RestoreOfficialLoginButton: () => <button>restore-owner</button>,
}));
vi.mock("@/components/settings/ManagedAccountProviderSection", () => ({
  ManagedAccountProviderSection: () => <div>consumer-owner</div>,
}));
vi.mock("@/components/relay/ManagedAccountBootstrapSection", () => ({
  ManagedAccountBootstrapSection: () => <div>bootstrap-owner</div>,
}));
describe("Unified account management mount", () => {
  it("mounts only the selected original manager, without application consumers", () => {
    const view = render(<AuthCenterPanel platform="github_copilot" />);
    expect(screen.getByText("copilot-owner")).toBeInTheDocument();
    for (const owner of [
      "codex",
      "minimax",
      "xai",
      "workbuddy",
      "trae",
      "zcode",
      "consumer",
      "bootstrap",
    ])
      expect(screen.queryByText(`${owner}-owner`)).not.toBeInTheDocument();
    view.rerender(<AuthCenterPanel platform="workbuddy" />);
    expect(screen.getByText("workbuddy-owner")).toBeInTheDocument();
    expect(screen.queryByText("copilot-owner")).not.toBeInTheDocument();
    view.rerender(<AuthCenterPanel platform="zcode" />);
    expect(screen.getByText("zcode-owner")).toBeInTheDocument();
    expect(screen.queryByText("workbuddy-owner")).not.toBeInTheDocument();
  });
});

import { navigationReducer } from "@/components/shell/navigation";
it("returns a manager to its exact originating application without another account owner", () => {
  const initial = {
    current: { view: "providers" as const, app: "codex" as const },
    history: [],
  };
  const opened = navigationReducer(initial, {
    type: "navigate",
    view: "services",
    platform: "codex_oauth",
  });
  expect(opened.current.app).toBe("codex");
  expect(opened.current.platform).toBe("codex_oauth");
  expect(navigationReducer(opened, { type: "back" })).toEqual(initial);
});

import { StrictMode } from "react";
import { AuthSettingsPanel } from "@/components/providers/AuthSettingsPanel";
import { OPEN_ACCOUNT_MANAGER } from "../accountPlatforms";
it("redirects each explicit management request once, including repeated account additions", () => {
  const event = vi.fn();
  const close = vi.fn();
  window.addEventListener(OPEN_ACCOUNT_MANAGER, event);
  const view = render(
    <StrictMode>
      <AuthSettingsPanel target="codex_oauth" onClose={close} />
    </StrictMode>,
  );
  expect(event).toHaveBeenCalledTimes(1);
  expect(close).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("codex-owner")).not.toBeInTheDocument();
  view.rerender(
    <StrictMode>
      <AuthSettingsPanel target={null} onClose={close} />
    </StrictMode>,
  );
  view.rerender(
    <StrictMode>
      <AuthSettingsPanel target="codex_oauth" onClose={close} />
    </StrictMode>,
  );
  expect(event).toHaveBeenCalledTimes(2);
  window.removeEventListener(OPEN_ACCOUNT_MANAGER, event);
});
