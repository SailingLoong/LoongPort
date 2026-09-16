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

  it("shows the first pending announcement and confirms it away", async () => {
    pending.value = [
      {
        id: "a1",
        type: "dialog",
        title: "维护公告",
        body: "今晚 2 点维护\n预计 30 分钟",
      },
      { id: "a2", type: "dialog", title: "第二条", body: "排队展示" },
    ];
    renderDialog();
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("维护公告");
    expect(dialog).toHaveTextContent("今晚 2 点维护");
    await userEvent.click(
      screen.getByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() => expect(ack).toHaveBeenCalledWith("a1"));
    // 队列推进：下一条顶上。
    await waitFor(() =>
      expect(screen.getByRole("dialog")).toHaveTextContent("第二条"),
    );
  });
});
