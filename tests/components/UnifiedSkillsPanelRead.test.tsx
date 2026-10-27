import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import UnifiedSkillsPanel from "@/components/skills/UnifiedSkillsPanel";
import type { InstalledSkill } from "@/lib/api/skills";

const { getInstalled } = vi.hoisted(() => ({ getInstalled: vi.fn() }));
vi.mock("@/lib/api/skills", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api/skills")>();
  return {
    ...actual,
    skillsApi: {
      ...actual.skillsApi,
      getInstalled,
      scanUnmanaged: vi.fn().mockResolvedValue([]),
    },
  };
});
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    settings: null,
    autoSaveSettings: vi.fn(),
    updateSettings: vi.fn(),
  }),
}));

const skill: InstalledSkill = {
  id: "synthetic/repo:read-test",
  name: "Synthetic Installed Skill",
  directory: "read-test",
  apps: {
    claude: false,
    codex: false,
    gemini: false,
    grokbuild: false,
    opencode: false,
    openclaw: false,
    hermes: false,
    pi: false,
  },
  installedAt: 1,
  updatedAt: 1,
};
let client: QueryClient;

function renderPanel() {
  return render(
    <QueryClientProvider client={client}>
      <UnifiedSkillsPanel onOpenDiscovery={() => {}} currentApp="claude" />
    </QueryClientProvider>,
  );
}

describe("UnifiedSkillsPanel installed read lifecycle", () => {
  beforeEach(() => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    getInstalled.mockReset();
  });
  afterEach(() => client.clear());

  it("shows a failed first read instead of empty inventory and recovers on refresh", async () => {
    getInstalled
      .mockRejectedValueOnce(new Error("synthetic installed read failed"))
      .mockResolvedValueOnce([skill]);
    renderPanel();
    expect(screen.getByText("skills.loading")).toBeInTheDocument();
    await waitFor(() =>
      expect(client.getQueryState(["skills", "installed"])?.status).toBe(
        "error",
      ),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "skills.loadFailed",
    );
    expect(screen.queryByText("skills.noInstalled")).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "skills.refresh" }),
    );
    expect(await screen.findByText(skill.name)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(getInstalled).toHaveBeenCalledTimes(2);
  });

  it("keeps a genuinely successful empty inventory distinct from failure", async () => {
    getInstalled.mockResolvedValue([]);
    renderPanel();
    expect(await screen.findByText("skills.noInstalled")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(client.getQueryState(["skills", "installed"])?.status).toBe(
      "success",
    );
  });
});
