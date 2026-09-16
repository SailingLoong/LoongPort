import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AnnouncementDialog } from "../AnnouncementDialog";

const pending = vi.hoisted(() => ({
  value: [] as { id: string; type: string; title: string; body: string }[],
}));
const ack = vi.hoisted(() => vi.fn());

vi.mock("@/lib/api/announcements", () => ({
  announcementsApi: {
    getPending: vi.fn(() => Promise.resolve(pending.value)),
    acknowledge: (id: string) => {
      ack(id);
      pending.value = pending.value.filter((item) => item.id !== id);
      return Promise.resolve();
    },
  },
}));
vi.mock("@/lib/api", () => ({
  settingsApi: { openExternal: vi.fn() },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const renderDialog = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AnnouncementDialog />
    </QueryClientProvider>,
  );

describe("AnnouncementDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    pending.value = [];
  });

  it("stays silent without pending announcements", async () => {
    renderDialog();
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("shows one announcement per session: the rest wait for next launch", async () => {
    pending.value = [
      { id: "a1", type: "dialog", title: "第一条", body: "内容一" },
      { id: "a2", type: "dialog", title: "第二条", body: "内容二" },
    ];
    renderDialog();
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("第一条");
    await userEvent.click(
      screen.getByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() => expect(ack).toHaveBeenCalledWith("a1"));
    // 会话闩：确认后第二条不连环弹出（下次启动才轮到它）。
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(screen.queryByText("第二条")).not.toBeInTheDocument();
  });

  it("renders markdown (bold, https links and images) and strips raw html / non-https", async () => {
    pending.value = [
      {
        id: "a1",
        type: "dialog",
        title: "富文本公告",
        body: "**加粗** 正文\n![横幅](https://example.com/banner.png)\n详情见 [官网](https://example.com)、[内网](http://insecure.local) 与 ![坏图](http://insecure.local/x.png)\n<script>alert(1)</script>",
      },
    ];
    renderDialog();
    const dialog = await screen.findByRole("dialog");
    // markdown 加粗生效；单换行按公告习惯渲染为换行（硬换行预处理）。
    expect(dialog.querySelector("strong")?.textContent).toBe("加粗");
    expect(dialog.querySelectorAll("br").length).toBeGreaterThan(0);
    // https 链接渲染为 <a>，http 链接降级为纯文本。
    expect(dialog.querySelector("a")?.textContent).toBe("官网");
    expect(dialog).toHaveTextContent("内网");
    // https 图片渲染且限宽；http 图片整块不渲染。
    const img = dialog.querySelector("img");
    expect(img?.getAttribute("src")).toBe("https://example.com/banner.png");
    expect(img?.className).toContain("max-w-full");
    expect(dialog.querySelectorAll("img")).toHaveLength(1);
    // 原始 HTML 被剥离，不出现可执行的 script 节点。
    expect(dialog.querySelector("script")).toBeNull();
    expect(dialog).toHaveTextContent("alert(1)");
  });
});
