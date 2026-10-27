import { toast } from "sonner";
import { createRef } from "react";
import {
  render,
  screen,
  waitFor,
  act,
  fireEvent,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi, beforeEach } from "vitest";

import {
  SkillsPage,
  getSkillsPageHeaderActions,
  type SkillsPageHandle,
} from "@/components/skills/SkillsPage";
import type {
  DiscoverableSkill,
  SkillRepo,
  SkillsShDiscoverableSkill,
  SkillsShSearchResult,
} from "@/lib/api/skills";

const installMutateAsyncMock = vi.fn();
const addRepoMutateAsyncMock = vi.fn();
let discoverableSkillsMock: DiscoverableSkill[] = [];
let skillReposMock: SkillRepo[] = [];
const refetchDiscoverableMock = vi.fn();

const searchCache = new Map<
  string,
  {
    data: { pages: SkillsShSearchResult[] } | undefined;
    isLoading: boolean;
    isFetching: boolean;
    hasNextPage: boolean;
    fetchNextPage: () => void;
    resetSearch: () => void;
  }
>();

const setSearchResult = (
  query: string,
  offset: number,
  result: SkillsShSearchResult | undefined,
  state: Partial<{
    isLoading: boolean;
    isFetching: boolean;
  }> = {},
) => {
  searchCache.set(`${query}:${offset}`, {
    data: result ? { pages: [result] } : undefined,
    hasNextPage: false,
    fetchNextPage: vi.fn(),
    resetSearch: vi.fn(),
    isLoading: false,
    isFetching: false,
    ...state,
  });
};

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("@/hooks/useSkills", () => ({
  useDiscoverableSkills: () => ({
    data: discoverableSkillsMock,
    isLoading: false,
    isFetching: false,
    refetch: refetchDiscoverableMock,
  }),
  useInstalledSkills: () => ({
    data: [],
    isLoading: false,
  }),
  useInstallSkill: () => ({
    mutateAsync: installMutateAsyncMock,
  }),
  useSkillRepos: () => ({
    data: skillReposMock,
    refetch: vi.fn(),
  }),
  useAddSkillRepo: () => ({
    mutateAsync: addRepoMutateAsyncMock,
  }),
  useRemoveSkillRepo: () => ({
    mutateAsync: vi.fn(),
  }),
  useSearchSkillsSh: (query: string) => {
    const cached = searchCache.get(`${query}:0`);
    if (cached) return cached;
    return {
      data: undefined,
      isLoading: false,
      isFetching: false,
      hasNextPage: false,
      fetchNextPage: vi.fn(),
      resetSearch: vi.fn(),
    };
  },
}));

const makeSkillsShSkill = (
  overrides: Partial<SkillsShDiscoverableSkill> = {},
): SkillsShDiscoverableSkill => ({
  key: "agent-browser:owner-a:repo-a",
  name: "Agent Browser",
  directory: "agent-browser",
  repoOwner: "owner-a",
  repoName: "repo-a",
  repoBranch: "main",
  installs: 100,
  readmeUrl: "https://example.com/a",
  installed: false,
  canInstall: true,
  canUninstall: false,
  ...overrides,
});

const makeDiscoverableSkill = (
  overrides: Partial<DiscoverableSkill> = {},
): DiscoverableSkill => ({
  key: "repo-skill:owner-a:repo-a",
  name: "Repo Skill",
  description: "Skill from a configured repository",
  directory: "repo-skill",
  readmeUrl: "https://example.com/repo-skill",
  repoOwner: "owner-a",
  repoName: "repo-a",
  repoBranch: "main",
  installed: false,
  canInstall: true,
  canUninstall: false,
  ...overrides,
});

const makeSkillRepo = (overrides: Partial<SkillRepo> = {}): SkillRepo => ({
  owner: "owner-a",
  name: "repo-a",
  branch: "main",
  enabled: true,
  ...overrides,
});

describe("SkillsPage - skills.sh install (regression)", () => {
  beforeEach(() => {
    addRepoMutateAsyncMock.mockReset();
    vi.mocked(toast.error).mockClear();
    vi.mocked(toast.success).mockClear();
    installMutateAsyncMock.mockReset();
    installMutateAsyncMock.mockResolvedValue({});
    discoverableSkillsMock = [];
    skillReposMock = [];
    refetchDiscoverableMock.mockReset();
    searchCache.clear();
  });

  it("retains the repository draft after an add failure and clears it only after a successful retry", async () => {
    const user = userEvent.setup();
    const ref = createRef<SkillsPageHandle>();
    addRepoMutateAsyncMock
      .mockRejectedValueOnce(new Error("synthetic repository save failed"))
      .mockResolvedValueOnce(undefined);
    refetchDiscoverableMock.mockResolvedValue({ data: [] });
    render(<SkillsPage ref={ref} />);

    // Use the same registered action as App's discovery-page header. Neither
    // SkillsPage's handler nor its actual RepoManagerPanel child is mocked.
    act(() => {
      getSkillsPageHeaderActions("skillssh")
        .find((action) => action.key === "manage-repos")!
        .execute(ref.current);
    });
    const url = screen.getByLabelText("skills.repo.url");
    const branch = screen.getByLabelText("skills.repo.branch");
    await user.type(url, "https://github.com/synthetic-owner/synthetic-repo");
    await user.type(branch, "review-branch");
    await user.click(screen.getByRole("button", { name: "skills.repo.add" }));

    await waitFor(() => {
      expect(
        screen.getByText("synthetic repository save failed"),
      ).toBeInTheDocument();
    });
    expect(url).toHaveValue(
      "https://github.com/synthetic-owner/synthetic-repo",
    );
    expect(branch).toHaveValue("review-branch");
    expect(refetchDiscoverableMock).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled(); // Inline error is the single owner.

    await user.click(screen.getByRole("button", { name: "skills.repo.add" }));
    await waitFor(() => expect(url).toHaveValue(""));
    expect(branch).toHaveValue("");
    expect(
      screen.queryByText("synthetic repository save failed"),
    ).not.toBeInTheDocument();
    expect(addRepoMutateAsyncMock).toHaveBeenCalledTimes(2);
    expect(addRepoMutateAsyncMock).toHaveBeenLastCalledWith({
      owner: "synthetic-owner",
      name: "synthetic-repo",
      branch: "review-branch",
      enabled: true,
    });
    expect(toast.success).toHaveBeenCalledTimes(1);
  });

  const openRepoManager = () => {
    const ref = createRef<SkillsPageHandle>();
    render(<SkillsPage ref={ref} />);
    act(() => {
      getSkillsPageHeaderActions("skillssh")
        .find((action) => action.key === "manage-repos")!
        .execute(ref.current);
    });
    return {
      url: screen.getByLabelText("skills.repo.url"),
      branch: screen.getByLabelText("skills.repo.branch"),
      add: screen.getByRole("button", { name: "skills.repo.add" }),
    };
  };

  const deferredSave = () => {
    let resolve!: () => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<void>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    return { promise, resolve, reject };
  };

  it.each([false, true])(
    "reports a saved repository with failed discovery without a stale count (cached=%s)",
    async (cached) => {
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } },
      });
      const observer = new QueryObserver<DiscoverableSkill[]>(client, {
        queryKey: ["synthetic-discovery"],
        enabled: false,
        initialData: cached ? [makeDiscoverableSkill()] : undefined,
        queryFn: async () => {
          throw new Error("synthetic discovery unavailable");
        },
      });
      // Run the installed QueryObserver's real refetch contract, not a mock
      // that rejects regardless of whether production opts into throwOnError.
      refetchDiscoverableMock.mockImplementation((options) =>
        observer.refetch(options),
      );
      addRepoMutateAsyncMock.mockResolvedValue(undefined);
      const { url, branch, add } = openRepoManager();
      fireEvent.change(url, { target: { value: "owner-a/repo-a" } });
      fireEvent.change(branch, { target: { value: "main" } });
      fireEvent.click(add);
      await waitFor(() => expect(url).toHaveValue(""));
      expect(branch).toHaveValue("");
      expect(addRepoMutateAsyncMock).toHaveBeenCalledTimes(1);
      expect(toast.success).not.toHaveBeenCalled();
      expect(toast.error).not.toHaveBeenCalled();
      expect(toast.warning).toHaveBeenCalledWith(
        "skills.repo.savedRefreshFailed",
        expect.objectContaining({ closeButton: true }),
      );
      expect(refetchDiscoverableMock).toHaveBeenCalledWith({
        throwOnError: true,
      });
      expect(
        screen.queryByText("skills.repo.addFailed"),
      ).not.toBeInTheDocument();
      client.clear();
    },
  );

  it("accepts a successful discovery with zero skills", async () => {
    addRepoMutateAsyncMock.mockResolvedValue(undefined);
    refetchDiscoverableMock.mockResolvedValue({ data: [] });
    const { url, add } = openRepoManager();
    fireEvent.change(url, { target: { value: "owner-a/empty-repo" } });
    fireEvent.click(add);
    await waitFor(() => expect(url).toHaveValue(""));
    expect(toast.success).toHaveBeenCalledWith("skills.repo.addSuccess", {
      closeButton: true,
    });
    expect(toast.warning).not.toHaveBeenCalled();
  });

  it("admits only one pending add even for repeated synchronous clicks", async () => {
    const save = deferredSave();
    addRepoMutateAsyncMock.mockReturnValue(save.promise);
    refetchDiscoverableMock.mockResolvedValue({ data: [] });
    const { url, add } = openRepoManager();
    fireEvent.change(url, { target: { value: "owner-a/repo-a" } });
    act(() => {
      fireEvent.click(add);
      fireEvent.click(add);
    });
    expect(addRepoMutateAsyncMock).toHaveBeenCalledTimes(1);
    expect(add).toBeDisabled();
    await act(async () => save.resolve());
    await waitFor(() => expect(url).toHaveValue(""));
    expect(add).not.toBeDisabled();
  });

  it.each(["success", "failure"])(
    "does not let an earlier save %s overwrite a newer draft",
    async (outcome) => {
      const save = deferredSave();
      addRepoMutateAsyncMock.mockReturnValueOnce(save.promise);
      refetchDiscoverableMock.mockResolvedValue({ data: [] });
      const { url, branch, add } = openRepoManager();
      fireEvent.change(url, { target: { value: "owner-a/repo-a" } });
      fireEvent.change(branch, { target: { value: "old-branch" } });
      fireEvent.click(add);
      fireEvent.change(url, { target: { value: "owner-b/repo-b" } });
      fireEvent.change(branch, { target: { value: "new-branch" } });
      await act(async () => {
        if (outcome === "success") save.resolve();
        else save.reject(new Error("old request failed"));
      });
      expect(url).toHaveValue("owner-b/repo-b");
      expect(branch).toHaveValue("new-branch");
      expect(screen.queryByText("old request failed")).not.toBeInTheDocument();
      expect(add).not.toBeDisabled();
      addRepoMutateAsyncMock.mockResolvedValueOnce(undefined);
      fireEvent.click(add);
      await waitFor(() => expect(url).toHaveValue(""));
      expect(addRepoMutateAsyncMock).toHaveBeenLastCalledWith({
        owner: "owner-b",
        name: "repo-b",
        branch: "new-branch",
        enabled: true,
      });
    },
  );

  it("retains a draft edited away and back while its earlier save is pending", async () => {
    const save = deferredSave();
    addRepoMutateAsyncMock.mockReturnValueOnce(save.promise);
    refetchDiscoverableMock.mockResolvedValue({ data: [] });
    const { url, add } = openRepoManager();
    fireEvent.change(url, { target: { value: "owner-a/repo-a" } });
    fireEvent.click(add);
    fireEvent.change(url, { target: { value: "owner-b/repo-b" } });
    fireEvent.change(url, { target: { value: "owner-a/repo-a" } });
    await act(async () => save.resolve());
    expect(url).toHaveValue("owner-a/repo-a");
  });

  it.each([
    ["panel reopen", "success"],
    ["panel reopen", "failure"],
    ["page remount", "success"],
    ["page remount", "failure"],
  ])(
    "preserves the new draft after %s and an old save %s",
    async (lifecycle, outcome) => {
      const save = deferredSave();
      addRepoMutateAsyncMock.mockReturnValueOnce(save.promise);
      refetchDiscoverableMock.mockResolvedValue({ data: [] });
      let ref = createRef<SkillsPageHandle>();
      const view = render(<SkillsPage ref={ref} />);
      const open = () =>
        act(() => {
          getSkillsPageHeaderActions("skillssh")
            .find((action) => action.key === "manage-repos")!
            .execute(ref.current);
        });
      open();
      fireEvent.change(screen.getByLabelText("skills.repo.url"), {
        target: { value: "owner-old/repo-old" },
      });
      fireEvent.click(screen.getByRole("button", { name: "skills.repo.add" }));
      expect(addRepoMutateAsyncMock).toHaveBeenCalledTimes(1);
      if (lifecycle === "panel reopen") {
        fireEvent.keyDown(window, { key: "Escape" });
      } else {
        view.unmount();
        ref = createRef<SkillsPageHandle>();
        render(<SkillsPage ref={ref} />);
      }
      expect(
        screen.queryByLabelText("skills.repo.url"),
      ).not.toBeInTheDocument();
      open();
      const url = screen.getByLabelText("skills.repo.url");
      const branch = screen.getByLabelText("skills.repo.branch");
      const add = screen.getByRole("button", { name: "skills.repo.add" });
      fireEvent.change(url, { target: { value: "owner-new/repo-new" } });
      fireEvent.change(branch, { target: { value: "new-branch" } });
      await act(async () => {
        if (outcome === "success") save.resolve();
        else save.reject(new Error("old instance save failed"));
      });
      expect(url).toHaveValue("owner-new/repo-new");
      expect(branch).toHaveValue("new-branch");
      expect(
        screen.queryByText("old instance save failed"),
      ).not.toBeInTheDocument();
      expect(add).not.toBeDisabled();
      // The admission guard belongs to one panel instance, not a global lock.
      addRepoMutateAsyncMock.mockResolvedValueOnce(undefined);
      fireEvent.click(add);
      await waitFor(() => expect(url).toHaveValue(""));
      expect(addRepoMutateAsyncMock).toHaveBeenLastCalledWith({
        owner: "owner-new",
        name: "repo-new",
        branch: "new-branch",
        enabled: true,
      });
    },
  );

  it("installs the second skill when two results share the same directory", async () => {
    const first = makeSkillsShSkill({
      key: "agent-browser:owner-a:repo-a",
      name: "Agent Browser A",
      repoOwner: "owner-a",
      repoName: "repo-a",
    });
    const second = makeSkillsShSkill({
      key: "agent-browser:owner-b:repo-b",
      name: "Agent Browser B",
      repoOwner: "owner-b",
      repoName: "repo-b",
    });

    setSearchResult("agent", 0, {
      skills: [first, second],
      totalCount: 2,
      query: "agent",
    });

    const ref = createRef<SkillsPageHandle>();
    render(<SkillsPage ref={ref} initialApp="claude" />);

    const user = userEvent.setup();

    // Switch to skills.sh source
    await user.click(screen.getByRole("button", { name: /skills\.sh/i }));

    // Type a query and submit
    const input = screen.getByPlaceholderText(
      "skills.skillssh.searchPlaceholder",
    );
    await user.type(input, "agent");
    await user.click(screen.getByRole("button", { name: "skills.search" }));

    // Wait for both cards to render
    await waitFor(() => {
      expect(screen.getByText("Agent Browser A")).toBeInTheDocument();
      expect(screen.getByText("Agent Browser B")).toBeInTheDocument();
    });

    // Click install on the SECOND card (Agent Browser B)
    const secondCard = screen
      .getByText("Agent Browser B")
      .closest("div.glass-card");
    expect(secondCard).not.toBeNull();
    const installButton = secondCard!.querySelector(
      "button:last-of-type",
    ) as HTMLButtonElement;
    expect(installButton).not.toBeNull();
    await user.click(installButton);

    // Verify the SECOND skill was passed to the install mutation, not the first
    await waitFor(() => {
      expect(installMutateAsyncMock).toHaveBeenCalledTimes(1);
    });
    const callArgs = installMutateAsyncMock.mock.calls[0][0];
    expect(callArgs.skill.repoOwner).toBe("owner-b");
    expect(callArgs.skill.repoName).toBe("repo-b");
    expect(callArgs.skill.name).toBe("Agent Browser B");
  });

  it("renders repository installation status and actions from backend facts", () => {
    discoverableSkillsMock = [
      makeDiscoverableSkill({
        installed: true,
        canInstall: false,
        canUninstall: false,
      }),
    ];
    skillReposMock = [makeSkillRepo()];

    render(<SkillsPage initialApp="claude" />);

    expect(screen.getByText("skills.installed")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "skills.install" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "skills.uninstall" }),
    ).not.toBeInTheDocument();
  });

  it("does not invent an install action for a blocked skills.sh result", async () => {
    setSearchResult("agent", 0, {
      skills: [
        makeSkillsShSkill({
          installed: false,
          canInstall: false,
          canUninstall: false,
        }),
      ],
      totalCount: 1,
      query: "agent",
    });

    render(<SkillsPage initialApp="claude" />);
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: /skills\.sh/i }));
    const input = screen.getByPlaceholderText(
      "skills.skillssh.searchPlaceholder",
    );
    await user.type(input, "agent");
    await user.click(screen.getByRole("button", { name: "skills.search" }));

    await waitFor(() => {
      expect(screen.getByText("Agent Browser")).toBeInTheDocument();
    });
    expect(
      screen.queryByRole("button", { name: "skills.install" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "skills.uninstall" }),
    ).not.toBeInTheDocument();
  });

  it("keeps skills.sh results when submitting the same query again", async () => {
    const figmaSkill = makeSkillsShSkill({
      key: "figma-use:figma:mcp-server-guide",
      name: "figma-use",
      directory: "figma-use",
      repoOwner: "figma",
      repoName: "mcp-server-guide",
    });

    setSearchResult("figma", 0, {
      skills: [figmaSkill],
      totalCount: 1,
      query: "figma",
    });

    render(<SkillsPage initialApp="claude" />);
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: /skills\.sh/i }));
    const input = screen.getByPlaceholderText(
      "skills.skillssh.searchPlaceholder",
    );
    await user.type(input, "figma");

    const searchButton = screen.getByRole("button", {
      name: "skills.search",
    });
    await user.click(searchButton);

    await waitFor(() => {
      expect(screen.getByText("figma-use")).toBeInTheDocument();
    });

    await user.click(searchButton);

    expect(screen.getByText("figma-use")).toBeInTheDocument();
  });

  it("shows the skills.sh loading state while a new query is fetching", async () => {
    const figmaSkill = makeSkillsShSkill({
      key: "figma-use:figma:mcp-server-guide",
      name: "figma-use",
      directory: "figma-use",
      repoOwner: "figma",
      repoName: "mcp-server-guide",
    });

    setSearchResult("figma", 0, {
      skills: [figmaSkill],
      totalCount: 1,
      query: "figma",
    });
    setSearchResult("react", 0, undefined, { isFetching: true });

    render(<SkillsPage initialApp="claude" />);
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: /skills\.sh/i }));
    const input = screen.getByPlaceholderText(
      "skills.skillssh.searchPlaceholder",
    );
    await user.type(input, "figma");

    const searchButton = screen.getByRole("button", {
      name: "skills.search",
    });
    await user.click(searchButton);

    await waitFor(() => {
      expect(screen.getByText("figma-use")).toBeInTheDocument();
    });

    await user.clear(input);
    await user.type(input, "react");
    await user.click(searchButton);

    expect(screen.getByText("skills.skillssh.loading")).toBeInTheDocument();
  });

  it("reports the effective skills.sh source to parent chrome", async () => {
    const onSourceChange = vi.fn();

    render(<SkillsPage initialApp="claude" onSourceChange={onSourceChange} />);

    await waitFor(() => {
      expect(onSourceChange).toHaveBeenCalledWith("skillssh");
    });
  });

  it("keeps the repository source when configured repositories return no discoverable skills", async () => {
    skillReposMock = [makeSkillRepo()];
    const onSourceChange = vi.fn();

    render(<SkillsPage initialApp="claude" onSourceChange={onSourceChange} />);

    await waitFor(() => {
      expect(onSourceChange).toHaveBeenCalledWith("repos");
    });
    expect(
      screen.getByPlaceholderText("skills.searchPlaceholder"),
    ).toBeVisible();
  });

  it("can switch back to repository results after discoverable skills refresh", async () => {
    const onSourceChange = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      <SkillsPage initialApp="claude" onSourceChange={onSourceChange} />,
    );

    await waitFor(() => {
      expect(onSourceChange).toHaveBeenCalledWith("skillssh");
    });

    await user.click(screen.getByRole("button", { name: /skills\.sh/i }));

    discoverableSkillsMock = [makeDiscoverableSkill()];
    skillReposMock = [makeSkillRepo()];
    rerender(
      <SkillsPage initialApp="claude" onSourceChange={onSourceChange} />,
    );

    await user.click(
      screen.getByRole("button", { name: "skills.searchSource.repos" }),
    );

    expect(screen.getByText("Repo Skill")).toBeInTheDocument();
    expect(onSourceChange).toHaveBeenCalledWith("repos");
  });

  it("exposes repository-only header actions for the parent chrome", () => {
    expect(
      getSkillsPageHeaderActions("repos").map((action) => action.key),
    ).toEqual(["refresh-repos", "manage-repos"]);
    expect(
      getSkillsPageHeaderActions("skillssh").map((action) => action.key),
    ).toEqual(["manage-repos"]);
  });
});
