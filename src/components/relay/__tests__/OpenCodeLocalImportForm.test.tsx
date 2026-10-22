import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
const { preview, commit, cancel } = vi.hoisted(() => ({
  preview: vi.fn(),
  commit: vi.fn(),
  cancel: vi.fn(),
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    previewOpenCodeLocal: preview,
    importOpenCodeLocal: commit,
    cancelOpenCodeLocal: cancel,
  },
}));
import { OpenCodeLocalImportForm } from "../OpenCodeLocalImportForm";
const sample = {
  previewId: "preview-a",
  entries: [{ id: "cred-a", label: "Work", isActive: true }],
  notices: [],
};
const read = () =>
  fireEvent.click(
    screen.getByRole("button", { name: "同意只读预览本机 OpenCode 凭据" }),
  );
const save = () =>
  fireEvent.click(
    screen.getByRole("button", { name: "同意加密保存所选 API key" }),
  );
describe("OpenCode local credential import", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    preview.mockResolvedValue(sample);
    commit.mockResolvedValue({ rowId: 9, configurationReady: true });
    cancel.mockResolvedValue(undefined);
  });
  it("does not read on render, reads only after consent, previews without importing", async () => {
    render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    expect(preview).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
    read();
    await screen.findByText(/Work/);
    expect(preview).toHaveBeenCalledWith(true);
    expect(commit).not.toHaveBeenCalled();
  });
  it("imports once only after the second consent and distinguishes saved from live tested", async () => {
    const saved = vi.fn();
    render(<OpenCodeLocalImportForm onSaved={saved} />);
    read();
    await screen.findByText(/Work/);
    save();
    save();
    await waitFor(() => expect(saved).toHaveBeenCalledWith(9));
    expect(commit).toHaveBeenCalledTimes(1);
    expect(commit).toHaveBeenCalledWith("preview-a", "cred-a", true);
    expect(screen.getByRole("status")).toHaveTextContent("尚未验证真实调用");
  });
  it("cancel invalidates the preview without saving", async () => {
    render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    read();
    await screen.findByText(/Work/);
    fireEvent.click(screen.getByRole("button", { name: "取消本次导入" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledWith("preview-a"));
    expect(commit).not.toHaveBeenCalled();
  });
  it("cancels a late preview after leaving without publishing stale state", async () => {
    let done!: (x: typeof sample) => void;
    preview.mockReturnValue(
      new Promise((r) => {
        done = r;
      }),
    );
    const view = render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    read();
    view.unmount();
    await act(async () => done(sample));
    expect(cancel).toHaveBeenCalledWith("preview-a");
    expect(commit).not.toHaveBeenCalled();
  });
  it("reports locked/source-changed errors safely and allows a fresh preview", async () => {
    preview.mockRejectedValueOnce("source_locked");
    render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    read();
    await screen.findByText(/数据库正忙/);
    read();
    await screen.findByText(/Work/);
    commit.mockRejectedValueOnce("source_changed");
    save();
    await screen.findByText(/本机账号或凭据已变化/);
    expect(screen.queryByText(/Work/)).not.toBeInTheDocument();
  });
  it("reports partial configuration success precisely and does not invite duplicate credentials", async () => {
    commit.mockResolvedValue({ rowId: 9, configurationReady: false });
    const saved = vi.fn();
    render(<OpenCodeLocalImportForm onSaved={saved} />);
    read();
    await screen.findByText(/Work/);
    save();
    await waitFor(() => expect(saved).toHaveBeenCalledWith(9));
    expect(screen.getByRole("status")).toHaveTextContent(
      "凭据已保存，应用配置未完成",
    );
    expect(screen.getByRole("status")).toHaveTextContent("账号行刷新重试");
  });
  it("never presents OAuth or unknown credentials as importable", async () => {
    preview.mockResolvedValue({
      previewId: "p",
      entries: [],
      notices: ["oauth_not_imported", "unsupported_credential"],
    });
    render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    read();
    await screen.findByText(/未找到可导入的完整/);
    expect(
      screen.queryByRole("button", { name: "同意加密保存所选 API key" }),
    ).not.toBeInTheDocument();
    expect(commit).not.toHaveBeenCalled();
    expect(screen.getByText(/OAuth 登录凭据未导入/)).toBeInTheDocument();
  });
  it("requires choosing among multiple saved accounts", async () => {
    preview.mockResolvedValue({
      ...sample,
      entries: [
        ...sample.entries,
        { id: "cred-b", label: "Personal", isActive: false },
      ],
    });
    render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
    read();
    await screen.findByText(/Work/);
    expect(
      screen.getByRole("button", { name: "同意加密保存所选 API key" }),
    ).toBeDisabled();
    fireEvent.click(screen.getByLabelText(/Personal/));
    save();
    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith("preview-a", "cred-b", true),
    );
  });
});

describe("OpenCode import retained-view lifecycle", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    preview.mockResolvedValue(sample);
    commit.mockResolvedValue({ rowId: 9, configurationReady: true });
    cancel.mockResolvedValue(undefined);
  });
  it.each([
    ["read", false],
    ["read", true],
    ["save", false],
    ["save", true],
  ] as const)(
    "unlocks %s after hide/resolve/show, restore-before-resolve=%s",
    async (operation, restoreFirst) => {
      const { PreservedView } = await import("@/components/ui/PreservedView");
      let done!: (value: unknown) => void;
      const saved = vi.fn();
      const viewAt = (active: boolean) => (
        <PreservedView active={active}>
          <OpenCodeLocalImportForm onSaved={saved} />
        </PreservedView>
      );
      if (operation === "read")
        preview.mockReturnValueOnce(
          new Promise((resolve) => {
            done = resolve;
          }),
        );
      else
        commit.mockReturnValueOnce(
          new Promise((resolve) => {
            done = resolve;
          }),
        );
      const view = render(viewAt(true));
      read();
      if (operation === "save") {
        await screen.findByText(/Work/);
        save();
      }
      view.rerender(viewAt(false));
      if (restoreFirst) view.rerender(viewAt(true));
      await act(async () =>
        done(
          operation === "read"
            ? sample
            : { rowId: 9, configurationReady: true },
        ),
      );
      if (!restoreFirst) view.rerender(viewAt(true));
      await waitFor(() =>
        expect(
          screen.getByRole("button", {
            name: "同意只读预览本机 OpenCode 凭据",
          }),
        ).not.toBeDisabled(),
      );
      expect(screen.queryByRole("radio")).not.toBeInTheDocument();
      expect(saved).not.toHaveBeenCalled();
    },
  );
  it("retires a preserved preview when the tab is hidden", async () => {
    const { PreservedView } = await import("@/components/ui/PreservedView");
    const viewAt = (active: boolean) => (
      <PreservedView active={active}>
        <OpenCodeLocalImportForm onSaved={vi.fn()} />
      </PreservedView>
    );
    const view = render(viewAt(true));
    read();
    await screen.findByText(/Work/);
    view.rerender(viewAt(false));
    view.rerender(viewAt(true));
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(cancel).toHaveBeenCalledWith("preview-a");
  });
});
