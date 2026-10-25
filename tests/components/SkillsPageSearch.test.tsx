import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SkillsPage } from "@/components/skills/SkillsPage";
import type {
  DiscoverableSkill,
  SkillsShDiscoverableSkill,
} from "@/lib/api/skills";

const api = vi.hoisted(() => ({
  discoverAvailable: vi.fn(),
  getRepos: vi.fn(),
  searchSkillsSh: vi.fn(),
  installUnified: vi.fn(),
}));
vi.mock("@/lib/api/skills", () => ({ skillsApi: api }));

function skill(name: string, installed = false): SkillsShDiscoverableSkill {
  return {
    key: `${name}:example:skills`,
    name,
    directory: name,
    repoOwner: "example",
    repoName: "skills",
    repoBranch: "main",
    installs: 1,
    installed,
    canInstall: !installed,
    canUninstall: installed,
  };
}

async function searchPage(firstResult: string | null = "First Skill") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <SkillsPage />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: /skills\.sh/i }));
  const input = screen.getByPlaceholderText(
    "skills.skillssh.searchPlaceholder",
  );
  await user.type(input, "agent");
  await user.click(screen.getByRole("button", { name: "skills.search" }));
  if (firstResult) await screen.findByText(firstResult);
  return { client, user, input };
}

beforeEach(() => {
  vi.resetAllMocks();
  api.discoverAvailable.mockResolvedValue([]);
  api.getRepos.mockResolvedValue([
    { owner: "example", name: "skills", branch: "main", enabled: true },
  ]);
});

describe("SkillsPage search results", () => {
  it("refreshes installation facts across loaded pages without duplicating the current page", async () => {
    let installed = false;
    api.searchSkillsSh.mockImplementation(
      async (query: string, _limit: number, offset: number) => ({
        query,
        totalCount: 21,
        skills:
          offset === 0
            ? [skill("First Skill", installed)]
            : [skill("Next Skill")],
      }),
    );
    api.installUnified.mockImplementation(
      async (selected: DiscoverableSkill) => {
        installed = true;
        return {
          id: selected.key,
          name: selected.name,
          directory: selected.directory,
          apps: { claude: true },
          installedAt: 0,
          updatedAt: 0,
        };
      },
    );
    const { user } = await searchPage();
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("Next Skill");
    const card = screen
      .getByText("First Skill")
      .closest("div.glass-card") as HTMLElement;
    await user.click(
      within(card).getByRole("button", { name: "skills.install" }),
    );
    await waitFor(() =>
      expect(
        within(card).queryByRole("button", { name: "skills.install" }),
      ).not.toBeInTheDocument(),
    );
    expect(within(card).getByText("skills.installed")).toBeInTheDocument();
    expect(screen.getAllByText("Next Skill")).toHaveLength(1);
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 0, 20,
    ]);
  });

  it("retries a failed page without skipping its offset", async () => {
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("First Skill")],
    });
    api.searchSkillsSh.mockRejectedValueOnce(new Error("temporary failure"));
    api.searchSkillsSh.mockImplementation(
      async (query: string, _limit: number, offset: number) => ({
        query,
        totalCount: 21,
        skills: offset === 20 ? [skill("Recovered Skill")] : [],
      }),
    );
    const { user } = await searchPage();
    const loadMore = screen.getByRole("button", {
      name: "skills.skillssh.loadMore",
    });
    await user.click(loadMore);
    await waitFor(() => expect(loadMore).toBeEnabled());
    await user.click(loadMore);
    await screen.findByText("Recovered Skill");
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 20,
    ]);
    expect(screen.getAllByText("First Skill")).toHaveLength(1);
  });

  it("keeps a same-query first page and restarts a same-query multi-page search", async () => {
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("First Skill")],
    });
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("Next Skill")],
    });
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 1,
      skills: [skill("Fresh Skill")],
    });
    const { user } = await searchPage();
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    expect(api.searchSkillsSh).toHaveBeenCalledTimes(1);
    expect(screen.getByText("First Skill")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("Next Skill");
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    await screen.findByText("Fresh Skill");
    expect(screen.queryByText("Next Skill")).not.toBeInTheDocument();
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 0,
    ]);
  });
  it("starts at the first page when returning to a previously paginated search", async () => {
    api.searchSkillsSh.mockImplementation(
      async (query: string, _limit: number, offset: number) => ({
        query,
        totalCount: query === "agent" ? 21 : 1,
        skills:
          query === "agent"
            ? [skill(offset === 0 ? "First Skill" : "Next Skill")]
            : [skill("Other Skill")],
      }),
    );
    const { user, input } = await searchPage();
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("Next Skill");
    await user.clear(input);
    await user.type(input, "react");
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    await screen.findByText("Other Skill");
    await user.clear(input);
    await user.type(input, "agent");
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    await screen.findByText("First Skill");
    await waitFor(() =>
      expect(screen.queryByText("Next Skill")).not.toBeInTheDocument(),
    );
    expect(
      api.searchSkillsSh.mock.calls.map((call) => [call[0], call[2]]),
    ).toEqual([
      ["agent", 0],
      ["agent", 20],
      ["react", 0],
      ["agent", 0],
    ]);
  });
  it("does not let a pre-install refresh overwrite installed facts after restarting search", async () => {
    let finishInstall!: (value: unknown) => void;
    let finishOldSearch!: (value: unknown) => void;
    api.installUnified.mockImplementation(
      () =>
        new Promise((resolve) => {
          finishInstall = resolve;
        }),
    );
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("First Skill")],
    });
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("Next Skill")],
    });
    api.searchSkillsSh.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOldSearch = resolve;
        }),
    );
    api.searchSkillsSh.mockResolvedValue({
      query: "agent",
      totalCount: 1,
      skills: [skill("First Skill", true)],
    });
    const { user } = await searchPage();
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("Next Skill");
    const card = screen
      .getByText("First Skill")
      .closest("div.glass-card") as HTMLElement;
    await user.click(
      within(card).getByRole("button", { name: "skills.install" }),
    );
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    await waitFor(() => expect(api.searchSkillsSh).toHaveBeenCalledTimes(3));
    await act(async () => {
      finishInstall({
        id: "first",
        name: "First Skill",
        directory: "first",
        apps: { claude: true },
        installedAt: 0,
        updatedAt: 0,
      });
    });
    await waitFor(() =>
      expect(within(card).getByText("skills.installed")).toBeInTheDocument(),
    );
    await act(async () => {
      finishOldSearch({
        query: "agent",
        totalCount: 21,
        skills: [skill("First Skill")],
      });
    });
    expect(
      within(card).queryByRole("button", { name: "skills.install" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Next Skill")).not.toBeInTheDocument();
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 0, 0,
    ]);
  });
  it("retries an initially failed query without requiring a different search term", async () => {
    api.searchSkillsSh.mockRejectedValueOnce(new Error("unavailable"));
    api.searchSkillsSh.mockRejectedValueOnce(new Error("still unavailable"));
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 1,
      skills: [skill("First Skill")],
    });
    const { user } = await searchPage(null);
    await screen.findByText("skills.skillssh.error");
    const search = screen.getByRole("button", { name: "skills.search" });
    expect(api.searchSkillsSh).toHaveBeenCalledTimes(1);
    await user.click(search);
    await waitFor(() => expect(api.searchSkillsSh).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(search).toBeEnabled());
    await user.click(search);
    await screen.findByText("First Skill");
    expect(
      api.searchSkillsSh.mock.calls.map((call) => [call[0], call[2]]),
    ).toEqual([
      ["agent", 0],
      ["agent", 0],
      ["agent", 0],
    ]);
  });

  it("loads later remote pages when the initial pages contain no displayable skills", async () => {
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 41,
      skills: [],
    });
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 41,
      skills: [],
    });
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 41,
      skills: [skill("Later Skill")],
    });
    const { user } = await searchPage(null);
    await screen.findByText("skills.skillssh.noResults");
    expect(
      screen.queryByRole("button", { name: "skills.install" }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await waitFor(() => expect(api.searchSkillsSh).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
      ).toBeEnabled(),
    );
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("Later Skill");
    expect(
      screen.getAllByRole("button", { name: "skills.install" }),
    ).toHaveLength(1);
    expect(
      screen.queryByRole("button", { name: "skills.skillssh.loadMore" }),
    ).not.toBeInTheDocument();
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 40,
    ]);
  });

  it("does not reload a successful empty search or offer pages beyond the remote count", async () => {
    api.searchSkillsSh.mockResolvedValue({
      query: "agent",
      totalCount: 0,
      skills: [],
    });
    const { user } = await searchPage(null);
    await screen.findByText("skills.skillssh.noResults");
    expect(
      screen.queryByRole("button", { name: "skills.skillssh.loadMore" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "skills.search" }));
    expect(api.searchSkillsSh).toHaveBeenCalledTimes(1);
  });
  it("keeps loading more available when a later request fails after an empty first page", async () => {
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [],
    });
    api.searchSkillsSh.mockRejectedValueOnce(
      new Error("temporary later-page failure"),
    );
    api.searchSkillsSh.mockResolvedValueOnce({
      query: "agent",
      totalCount: 21,
      skills: [skill("Recovered Later Skill")],
    });
    const { user } = await searchPage(null);
    await screen.findByText("skills.skillssh.noResults");
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await screen.findByText("skills.skillssh.error");
    const loadMore = screen.getByRole("button", {
      name: "skills.skillssh.loadMore",
    });
    expect(loadMore).toBeEnabled();
    await user.click(loadMore);
    await screen.findByText("Recovered Later Skill");
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20, 20,
    ]);
  });
  it("stops at the remote boundary when every page is filtered empty", async () => {
    api.searchSkillsSh.mockResolvedValue({
      query: "agent",
      totalCount: 21,
      skills: [],
    });
    const { user } = await searchPage(null);
    await screen.findByText("skills.skillssh.noResults");
    await user.click(
      screen.getByRole("button", { name: "skills.skillssh.loadMore" }),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "skills.skillssh.loadMore" }),
      ).not.toBeInTheDocument(),
    );
    expect(screen.getByText("skills.skillssh.noResults")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "skills.install" }),
    ).not.toBeInTheDocument();
    expect(api.searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([
      0, 20,
    ]);
  });
});
