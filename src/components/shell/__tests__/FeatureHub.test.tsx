import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AppId } from "@/lib/api";
import { FeatureHub } from "../FeatureHub";

function setup(appId: AppId, kind: "records" | "resources" = "resources") {
  const onNavigate = vi.fn();
  const onUsage = vi.fn();
  const onLaunchDashboard = vi.fn();
  render(
    <FeatureHub
      appId={appId}
      kind={kind}
      onNavigate={onNavigate}
      onUsage={onUsage}
      onLaunchDashboard={onLaunchDashboard}
    />,
  );
  return { onNavigate, onUsage, onLaunchDashboard, user: userEvent.setup() };
}

const entry = (name: string) =>
  screen.getByRole("button", { name: new RegExp(`client.features.${name}`) });

describe("FeatureHub", () => {
  it("keeps common extension management destinations reachable", async () => {
    const { user, onNavigate } = setup("claude");
    const destinations = [
      "mcp",
      "skills",
      "prompts",
      "agents",
      "universal",
      "workspace",
    ];
    for (const destination of destinations)
      await user.click(entry(destination));
    expect(onNavigate.mock.calls.map(([view]) => view)).toEqual(destinations);
  });

  it("includes all OpenClaw configuration destinations", async () => {
    const { user, onNavigate } = setup("openclaw");
    const destinations = [
      "workspace",
      "openclawEnv",
      "openclawTools",
      "openclawAgents",
    ];
    for (const destination of destinations)
      await user.click(entry(destination));
    expect(onNavigate.mock.calls.map(([view]) => view)).toEqual(destinations);
    expect(
      screen.queryByRole("button", { name: /client.features.hermesMemory/ }),
    ).not.toBeInTheDocument();
  });

  it("opens Hermes memory and its real dashboard callback", async () => {
    const { user, onNavigate, onLaunchDashboard } = setup("hermes");
    await user.click(entry("hermesMemory"));
    await user.click(entry("hermesDashboard"));
    expect(onNavigate).toHaveBeenCalledWith("hermesMemory");
    expect(onLaunchDashboard).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("button", { name: /client.features.openclawEnv/ }),
    ).not.toBeInTheDocument();
  });

  it("does not offer native MCP for Pi while retaining its extension entries", async () => {
    const { user, onNavigate } = setup("pi");
    expect(
      screen.queryByRole("button", { name: /client.features.mcp/ }),
    ).not.toBeInTheDocument();
    await user.click(entry("skills"));
    await user.click(entry("prompts"));
    expect(onNavigate.mock.calls.map(([view]) => view)).toEqual([
      "skills",
      "prompts",
    ]);
  });

  it("keeps usage statistics and application sessions as distinct actions", async () => {
    const { user, onNavigate, onUsage } = setup("codex", "records");
    await user.click(entry("usage"));
    await user.click(entry("sessions"));
    expect(onUsage).toHaveBeenCalledOnce();
    expect(onNavigate).toHaveBeenCalledWith("sessions");
  });

  it("keeps usage accessible for image generation without inventing a sessions view", async () => {
    const { user, onUsage } = setup("codex-image", "records");
    expect(
      screen.queryByRole("button", { name: /client.features.sessions/ }),
    ).not.toBeInTheDocument();
    await user.click(entry("usage"));
    expect(onUsage).toHaveBeenCalledOnce();
  });
});
