import type { PropsWithChildren } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useSearchSkillsSh } from "@/hooks/useSkills";
import type {
  SkillsShDiscoverableSkill,
  SkillsShSearchResult,
} from "@/lib/api/skills";

const searchSkillsSh = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api/skills", () => ({ skillsApi: { searchSkillsSh } }));

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

function page(
  query: string,
  names: string[],
  totalCount = names.length,
  installed = false,
): SkillsShSearchResult {
  return {
    query,
    totalCount,
    skills: names.map((name) => skill(name, installed)),
  };
}

function setup(initialQuery = "agent") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: PropsWithChildren) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return {
    client,
    ...renderHook(({ query }) => useSearchSkillsSh(query, 2), {
      wrapper,
      initialProps: { query: initialQuery },
    }),
  };
}

afterEach(cleanup);
beforeEach(() => {
  searchSkillsSh.mockReset();
});

describe("skills.sh paginated query ownership", () => {
  it("replaces every loaded page after invalidation instead of appending stale results", async () => {
    let installed = false;
    searchSkillsSh.mockImplementation(
      async (query: string, _limit: number, offset: number) =>
        page(
          query,
          offset === 0 ? ["first", "second"] : ["third"],
          3,
          installed,
        ),
    );
    const { client, result } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() =>
      expect(result.current.data?.pageParams).toEqual([0, 2]),
    );
    await waitFor(() => expect(result.current.hasNextPage).toBe(false));

    installed = true;
    await act(async () => {
      await client.invalidateQueries({ queryKey: ["skills", "skillssh"] });
    });
    await waitFor(() =>
      expect(
        result.current.data?.pages
          .flatMap((p) => p.skills)
          .map((s) => [s.name, s.installed]),
      ).toEqual([
        ["first", true],
        ["second", true],
        ["third", true],
      ]),
    );
    expect(searchSkillsSh.mock.calls).toEqual([
      ["agent", 2, 0],
      ["agent", 2, 2],
      ["agent", 2, 0],
      ["agent", 2, 2],
    ]);
  });

  it("retries a failed next page at the same offset without losing loaded results", async () => {
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["first", "second"], 3));
    searchSkillsSh.mockRejectedValueOnce(new Error("temporary search failure"));
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["third"], 3));
    const { result } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() => expect(result.current.isFetchNextPageError).toBe(true));
    expect(result.current.data?.pages[0].skills.map((s) => s.name)).toEqual([
      "first",
      "second",
    ]);
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() =>
      expect(
        result.current.data?.pages.flatMap((p) => p.skills).map((s) => s.name),
      ).toEqual(["first", "second", "third"]),
    );
    expect(searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([0, 2, 2]);
  });

  it("does not display the previous query's pages while a new search is pending", async () => {
    let completeSearch!: (value: SkillsShSearchResult) => void;
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["first"], 1));
    searchSkillsSh.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          completeSearch = resolve;
        }),
    );
    const { result, rerender } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    rerender({ query: "react" });
    expect(result.current.data).toBeUndefined();
    expect(result.current.isFetching).toBe(true);
    await act(async () => {
      completeSearch(page("react", ["new-result"], 1));
    });
    await waitFor(() =>
      expect(result.current.data?.pages[0].skills[0].name).toBe("new-result"),
    );
  });

  it("advances by remote offsets when a page contains only filtered-out results", async () => {
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["first", "second"], 5));
    searchSkillsSh.mockResolvedValueOnce(page("agent", [], 5));
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["last"], 5));
    const { result } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    await act(async () => {
      await result.current.fetchNextPage();
    });
    expect(result.current.hasNextPage).toBe(true);
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() => expect(result.current.hasNextPage).toBe(false));
    await waitFor(() =>
      expect(
        result.current.data?.pages.flatMap((p) => p.skills).map((s) => s.name),
      ).toEqual(["first", "second", "last"]),
    );
    expect(searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([0, 2, 4]);
  });

  it("restarts from a freshly requested first page when a paginated search is submitted again", async () => {
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["first", "second"], 3));
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["third"], 3));
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["fresh-first"], 1));
    const { result } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await act(async () => {
      await result.current.resetSearch();
    });
    await waitFor(() =>
      expect(
        result.current.data?.pages.flatMap((p) => p.skills).map((s) => s.name),
      ).toEqual(["fresh-first"]),
    );
    expect(result.current.data?.pageParams).toEqual([0]);
    expect(searchSkillsSh.mock.calls.map((call) => call[2])).toEqual([0, 2, 0]);
  });

  it("keeps the successful first page if restarting a paginated search fails", async () => {
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["first", "second"], 3));
    searchSkillsSh.mockResolvedValueOnce(page("agent", ["third"], 3));
    searchSkillsSh.mockRejectedValueOnce(new Error("refresh unavailable"));
    const { result } = setup();
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(2));
    await act(async () => {
      await result.current.resetSearch();
    });
    await waitFor(() => expect(result.current.isRefetchError).toBe(true));
    expect(
      result.current.data?.pages.flatMap((p) => p.skills).map((s) => s.name),
    ).toEqual(["first", "second"]);
    expect(result.current.hasNextPage).toBe(true);
  });

  it("ignores a late response from a query that is no longer displayed", async () => {
    let finishOldSearch!: (value: SkillsShSearchResult) => void;
    searchSkillsSh.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOldSearch = resolve;
        }),
    );
    searchSkillsSh.mockResolvedValueOnce(page("react", ["current"], 1));
    const { result, rerender } = setup();
    await waitFor(() => expect(searchSkillsSh).toHaveBeenCalledTimes(1));
    rerender({ query: "react" });
    await waitFor(() =>
      expect(result.current.data?.pages[0].skills[0].name).toBe("current"),
    );
    await act(async () => {
      finishOldSearch(page("agent", ["obsolete"], 1));
    });
    expect(result.current.data?.pages[0].skills[0].name).toBe("current");
  });

  it("waits for a query of at least two characters before searching", async () => {
    const { result } = setup("a");
    expect(result.current.isFetching).toBe(false);
    expect(result.current.data).toBeUndefined();
    expect(searchSkillsSh).not.toHaveBeenCalled();
  });
});
