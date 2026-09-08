import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  RelayDirectoryItem,
  RelayDirectoryListing,
} from "@/lib/api/relay";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";
import { createTestQueryClient } from "../../../../../tests/utils/testQueryClient";

const {
  listDirectory,
  refreshDirectory,
  importSite,
  importDirectorySite,
  refresh,
  openInBrowser,
  toastError,
  toastSuccess,
  toastWarning,
} = vi.hoisted(() => ({
  listDirectory: vi.fn(),
  refreshDirectory: vi.fn(),
  importSite: vi.fn(),
  importDirectorySite: vi.fn(),
  refresh: vi.fn(),
  openInBrowser: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  PLAZA_VISIBLE_DEFAULT: true,
  relayApi: {
    listDirectory,
    refreshDirectory,
    importSite,
    importDirectorySite,
    refresh,
  },
}));

const useSettingsMock = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => useSettingsMock(),
}));

vi.mock("../../openInBrowser", () => ({ openInBrowser }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) =>
      options ? `${key} ${JSON.stringify(options)}` : key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

vi.mock("sonner", () => ({
  toast: {
    success: toastSuccess,
    error: toastError,
    warning: toastWarning,
    info: vi.fn(),
  },
}));

const { RelayDirectoryPage } = await import("../RelayDirectoryPage");

function renderDirectory(props: ComponentProps<typeof RelayDirectoryPage>) {
  const client = createTestQueryClient();
  const view = render(
    <QueryClientProvider client={client}>
      <RelayDirectoryPage {...props} />
    </QueryClientProvider>,
  );
  return { ...view, client };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

function item(index: number): RelayDirectoryItem {
  return {
    siteHost: index === 1 ? "bestapi.store" : `site-${index}.example`,
    siteDomain: index === 1 ? "bestapi.store" : `site-${index}.example`,
    displayName: index === 1 ? "BestAPI" : `站点 ${index}`,
    rank: index,
    entryUrl:
      index === 1 ? "https://bestapi.store" : `https://site-${index}.example`,
    // 自家实测观测：只有近 24 小时过了 k-匿的站才有（其余站整个缺席）。
    crowd:
      index === 1
        ? { ttftP50Ms: 812.5, errRate: 0.004 }
        : index === 2
          ? { ttftP50Ms: null, errRate: 0.02 }
          : undefined,
    // 站方一手 transit 摘要：只有部署了 ai-transit 公开协议的站才有
    //（New API 站与未部署的站保持 undefined，徽章不渲染）。
    transit:
      index === 1
        ? {
            minMultiplier: 0.06,
            minAvailability: 88.3,
            syncedAt: 1786633200,
            rechargeMultiplier: 1,
            minimumTopUp: 50,
            currency: "CNY",
            upstreamType: "mixed",
            isReverse: true,
            priceUrl: "https://bestapi.store/public/transit",
            supportUrl: "https://t.me/bestapi-group",
            groups: [
              {
                name: "group-a",
                platform: "openai",
                multiplier: 0.06,
                cacheHitRate7d: 79.8,
                availability: 88.3,
                avgLatencyMs: 5391,
                modelCount: 14,
              },
              {
                name: "group-b",
                platform: "anthropic",
                multiplier: 0.3,
                cacheHitRate7d: null,
                availability: null,
                avgLatencyMs: null,
                modelCount: 0,
              },
            ],
          }
        : undefined,
  };
}

function listing(
  overrides: Partial<RelayDirectoryListing> = {},
): RelayDirectoryListing {
  return {
    items: Array.from({ length: 13 }, (_, index) => item(index + 1)),
    syncedAt: 1786633200,
    ...overrides,
  };
}

describe("RelayDirectoryPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // settings=null：两个消费字段都落兜底（crowdEnabled=true、plazaVisible=true），
    // 与此前未 mock 该 hook 时的实际行为一致。
    useSettingsMock.mockReturnValue({ settings: null });
    listDirectory.mockImplementation(() => Promise.resolve(listing()));
    refreshDirectory.mockImplementation(() => Promise.resolve(listing()));
    importSite.mockResolvedValue({
      relayId: 7,
      siteOrigin: "https://bestapi.store",
      siteName: "BestAPI",
      backendKind: "sub2api",
    });
    importDirectorySite.mockResolvedValue({
      relayId: 7,
      siteOrigin: "https://bestapi.store",
      siteName: "BestAPI",
      backendKind: "sub2api",
    });
    refresh.mockResolvedValue({
      summary: {
        notice: "updated",
        refreshedAccounts: 1,
        tiers: 0,
        keysCreated: 0,
        otherPlatformTiers: 0,
        mergedProviders: 0,
        failures: [],
      },
      balances: [],
    });
  });

  it("renders measured badges from item.crowd for every visitor", async () => {
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });

    // 有实测的站：TTFT 与错误率两个徽章（行级观测公开，不依赖共建开关）。
    const bestRow = (await screen.findByText("BestAPI")).closest("article")!;
    expect(
      within(bestRow as HTMLElement).getByTitle("loongport.crowd.badgeHint"),
    ).toHaveTextContent("loongport.crowd.badgeLabel");
    expect(
      within(bestRow as HTMLElement).getByTitle("loongport.crowd.errBadgeHint"),
    ).toHaveTextContent("loongport.crowd.errBadgeLabel");

    // 只有错误率、没有 TTFT 的站：只渲染错误率徽章。
    const errOnlyRow = (await screen.findByText("站点 2")).closest("article")!;
    expect(
      within(errOnlyRow as HTMLElement).queryByTitle(
        "loongport.crowd.badgeHint",
      ),
    ).not.toBeInTheDocument();
    expect(
      within(errOnlyRow as HTMLElement).getByTitle(
        "loongport.crowd.errBadgeHint",
      ),
    ).toBeInTheDocument();

    // 没过 k-匿的站：一行实测徽章都不渲染，名次照常。
    const plainRow = (await screen.findByText("站点 3")).closest("article")!;
    expect(
      within(plainRow as HTMLElement).queryByTitle("loongport.crowd.badgeHint"),
    ).not.toBeInTheDocument();
    expect(
      within(plainRow as HTMLElement).queryByTitle(
        "loongport.crowd.errBadgeHint",
      ),
    ).not.toBeInTheDocument();
    expect(within(plainRow as HTMLElement).getByText("#3")).toBeInTheDocument();
  });

  it("renders transit badges only for sites that publish them", async () => {
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });

    // 有摘要的站：倍率（可点开详情，title 是动作提示）与可用性两个徽章都渲染。
    const bestRow = (await screen.findByText("BestAPI")).closest("article")!;
    expect(
      within(bestRow as HTMLElement).getByTitle(
        "loongport.directory.transit.openDetail",
      ),
    ).toHaveTextContent("0.06x");
    expect(
      within(bestRow as HTMLElement).getByTitle(
        "loongport.directory.transit.availabilityHint",
      ),
    ).toHaveTextContent("88%");

    // 没有摘要的站（未部署公开协议 / New API）：一行都不该有徽章。
    const plainRow = (await screen.findByText("站点 2")).closest("article")!;
    expect(
      within(plainRow as HTMLElement).queryByTitle(
        "loongport.directory.transit.openDetail",
      ),
    ).not.toBeInTheDocument();
    expect(
      within(plainRow as HTMLElement).queryByTitle(
        "loongport.directory.transit.availabilityHint",
      ),
    ).not.toBeInTheDocument();
  });

  it("opens the transit detail dialog from the multiplier badge", async () => {
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });

    const bestRow = (await screen.findByText("BestAPI")).closest("article")!;
    await userEvent.click(
      within(bestRow as HTMLElement).getByTitle(
        "loongport.directory.transit.openDetail",
      ),
    );

    // 弹窗标题：站名 + 站方公开数据。
    expect(
      screen.getByText("BestAPI · loongport.directory.transit.detailTitle"),
    ).toBeInTheDocument();
    // 充值口径与披露 meta（i18n mock 直接渲染 key，值跟在标签后面）。
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(
      "loongport.directory.transit.rechargeMultiplier",
    );
    expect(dialog).toHaveTextContent("¥50");
    expect(dialog).toHaveTextContent("mixed");
    // 分组表：两个分组、缺测值渲染为 —。
    expect(screen.getByText("group-a")).toBeInTheDocument();
    expect(screen.getByText("group-b")).toBeInTheDocument();
    expect(screen.getByText("5.4s")).toBeInTheDocument();
    expect(screen.getByText("80%")).toBeInTheDocument();
    // 站方价格页链接走外链打开。
    expect(
      screen.getByText("loongport.directory.transit.viewPricePage"),
    ).toBeInTheDocument();
    expect(openInBrowser).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByText("loongport.directory.transit.viewPricePage"),
    );
    expect(openInBrowser).toHaveBeenCalledWith(
      "https://bestapi.store/public/transit",
    );
  });

  it("authenticates every displayed row with one click", async () => {
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });

    const row = (await screen.findByText("站点 2")).closest("article");
    expect(row).not.toBeNull();
    const managedSite = within(row!);
    expect(
      managedSite.getByText("loongport.directory.actions.authenticate"),
    ).toBeInTheDocument();
    expect(
      managedSite.getByText("loongport.directory.actions.autoAddHint"),
    ).toBeInTheDocument();

    fireEvent.click(
      managedSite.getByRole("button", {
        name: "loongport.directory.actions.authenticate",
      }),
    );

    await waitFor(() =>
      expect(importDirectorySite).toHaveBeenCalledWith(
        "https://site-2.example",
      ),
    );
    expect(openInBrowser).not.toHaveBeenCalledWith("https://site-2.example");
  });

  it("adds the unmatched search text as a site via the manual import", async () => {
    const user = userEvent.setup();
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });
    await waitFor(() => expect(listDirectory).toHaveBeenCalled());

    await user.type(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      "https://my-own-relay.example",
    );
    await user.click(
      await screen.findByRole("button", {
        name: /loongport.directory.addAsSite/,
      }),
    );

    // 搜索词直连走 Manual 导入（保守打开规则），不是白名单行的目录导入。
    await waitFor(() =>
      expect(importSite).toHaveBeenCalledWith("https://my-own-relay.example"),
    );
    expect(importDirectorySite).not.toHaveBeenCalled();
  });

  it("hides only the recommended list when the plaza switch is off", async () => {
    // 广场开关关：列表为空、不发清单请求；搜索框与「搜不到就地直连」保留 ——
    // 关的是推荐列表，不是整个广场页。
    useSettingsMock.mockReturnValue({ settings: { plazaVisible: false } });
    const user = userEvent.setup();
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });

    await screen.findByText("loongport.directory.empty");
    expect(listDirectory).not.toHaveBeenCalled();
    expect(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
    ).toBeInTheDocument();

    await user.type(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      "https://my-own-relay.example",
    );
    await user.click(
      await screen.findByRole("button", {
        name: /loongport.directory.addAsSite/,
      }),
    );
    await waitFor(() =>
      expect(importSite).toHaveBeenCalledWith("https://my-own-relay.example"),
    );
  });

  it("searches and paginates twelve rows per page", async () => {
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    expect(screen.queryByText("站点 13")).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.directory.pagination.next",
      }),
    );
    expect(screen.getByText("站点 13")).toBeInTheDocument();

    fireEvent.change(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      { target: { value: "bestapi" } },
    );
    expect(screen.getByText("BestAPI")).toBeInTheDocument();
    expect(screen.queryByText("站点 13")).not.toBeInTheDocument();
  });

  it("shows the snapshot sync time only when there is a snapshot", async () => {
    listDirectory.mockResolvedValue(listing({ items: [item(1)] }));
    const first = renderDirectory({ sourceAppId: "claude", onBack: () => {} });

    expect(
      await first.findByText(/loongport\.directory\.source\.syncedAt/),
    ).toBeInTheDocument();
    first.unmount();

    // syncedAt = 0（没有实测快照）：时间戳整个不渲染，不显示假时间。
    listDirectory.mockResolvedValue(listing({ syncedAt: 0, items: [item(1)] }));
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");
    expect(
      screen.queryByText(/loongport\.directory\.source\.syncedAt/),
    ).not.toBeInTheDocument();
  });

  it("caches the listing for the whole app session", async () => {
    const { client } = renderDirectory({
      sourceAppId: "claude",
      onBack: () => {},
    });
    await screen.findByText("BestAPI");

    const query = client.getQueryCache().find({
      queryKey: relayDirectoryKeys.listing(),
    });
    expect(query?.gcTime).toBe(Infinity);
  });

  it("keeps the old list visible while a manual refresh is pending", async () => {
    const next = deferred<RelayDirectoryListing>();
    refreshDirectory.mockReturnValue(next.promise);
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.directory.actions.refresh",
      }),
    );

    await waitFor(() => expect(refreshDirectory).toHaveBeenCalled());
    expect(screen.getByText("BestAPI")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "loongport.directory.actions.refresh",
      }),
    ).toBeDisabled();

    next.resolve(listing({ items: [item(2)] }));
    expect(await screen.findByText("站点 2")).toBeInTheDocument();
  });

  it("keeps the old list and reports a manual refresh failure", async () => {
    refreshDirectory.mockRejectedValue(new Error("刷新失败"));
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.directory.actions.refresh",
      }),
    );

    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        expect.stringContaining("刷新失败"),
      ),
    );
    expect(screen.getByText("BestAPI")).toBeInTheDocument();
  });

  it("waits for authentication and backend refresh before returning", async () => {
    const onBack = vi.fn();
    const onAuthenticated = vi.fn();
    renderDirectory({
      sourceAppId: "claude",
      onBack,
      onAuthenticated,
    });
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() =>
      expect(importDirectorySite).toHaveBeenCalledWith("https://bestapi.store"),
    );
    expect(refresh).toHaveBeenCalledWith(7, "claude");
    await waitFor(() => expect(onAuthenticated).toHaveBeenCalled());
    expect(onBack).toHaveBeenCalled();
  });

  it("stays open when registration or login is cancelled", async () => {
    importDirectorySite.mockRejectedValue({
      kind: "cancelled",
      message: "注册或登录尚未完成",
    });
    const onBack = vi.fn();
    renderDirectory({ sourceAppId: "claude", onBack });
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(importDirectorySite).toHaveBeenCalled());
    expect(refresh).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("returns to the relay list when refresh fails after authentication", async () => {
    refresh.mockRejectedValue(new Error("网络不通"));
    const onBack = vi.fn();
    const onAuthenticated = vi.fn();
    renderDirectory({
      sourceAppId: "claude",
      onBack,
      onAuthenticated,
    });
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(onAuthenticated).toHaveBeenCalled());
    expect(onBack).toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledWith(
      expect.stringContaining("网络不通"),
    );
  });

  it.each([
    ["unsupported_site", "loongport.addSite.unsupportedSite"],
    ["not_in_directory", "loongport.addSite.notInDirectory"],
    ["protocol_conflict", "loongport.addSite.protocolConflict"],
  ])("maps %s to an actionable message", async (kind, key) => {
    importDirectorySite.mockRejectedValue({ kind, message: kind });
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(toastError).toHaveBeenCalledWith(key));
  });

  it("uses the localized fallback for an unknown object error", async () => {
    importDirectorySite.mockRejectedValue({ code: "unexpected" });
    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith("loongport.addSite.importFailed"),
    );
  });

  it("allows only one authentication operation at a time", async () => {
    let finishImport!: (value: {
      relayId: number;
      siteOrigin: string;
      siteName: string;
      backendKind: string;
    }) => void;
    importDirectorySite.mockReturnValue(
      new Promise((resolve) => {
        finishImport = resolve;
      }),
    );

    renderDirectory({ sourceAppId: "claude", onBack: () => {} });
    await screen.findByText("BestAPI");

    const firstRow = screen.getByText("BestAPI").closest("article");
    const secondRow = screen.getByText("站点 2").closest("article");
    fireEvent.click(
      within(firstRow!).getByText("loongport.directory.actions.authenticate"),
    );
    fireEvent.click(
      within(secondRow!).getByText("loongport.directory.actions.authenticate"),
    );

    expect(importDirectorySite).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        within(secondRow!).getByRole("button", {
          name: "loongport.directory.actions.authenticate",
        }),
      ).toBeDisabled(),
    );

    await act(async () => {
      finishImport({
        relayId: 7,
        siteOrigin: "https://bestapi.store",
        siteName: "BestAPI",
        backendKind: "sub2api",
      });
    });
  });

  it("offers the search text as a manual add only when nothing matches", async () => {
    // 手填域名合并进搜索框：搜不到 → 列表区变成「把搜索词添加为中转站」的
    // 整块虚框（白名单外的站也该能连，错误以可读 toast 呈现）；搜得到就不出现。
    const user = userEvent.setup();
    renderDirectory({ sourceAppId: "codex", onBack: () => {} });
    await screen.findByText("BestAPI");

    // 搜到白名单行：没有转添加的兜底。
    await user.type(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      "Best",
    );
    expect(
      screen.queryByRole("button", { name: /loongport.directory.addAsSite/ }),
    ).not.toBeInTheDocument();

    // 搜不到：兜底出现。
    await user.clear(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
    );
    await user.type(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      "my-own-relay.example",
    );
    expect(
      await screen.findByRole("button", {
        name: /loongport.directory.addAsSite/,
      }),
    ).toBeInTheDocument();
    expect(importSite).not.toHaveBeenCalled();
  });
});
