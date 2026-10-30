import {
  act,
  render,
  screen,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
const boundary = vi.hoisted(() => ({
  preview: vi.fn(),
  commit: vi.fn(),
  cancel: vi.fn(),
  key: vi.fn(),
  login: vi.fn(),
  buttons: new Map<string, () => void>(),
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    previewOpenCodeLocal: boundary.preview,
    importOpenCodeLocal: boundary.commit,
    cancelOpenCodeLocal: boundary.cancel,
    importOpenCodeKey: boundary.key,
    openLogin: boundary.login,
  },
  VENDOR_CATALOG: [
    { id: "opencode", displayName: "opencode", descriptionKey: "description" },
  ],
}));
vi.mock("@/components/ui/button", () => ({
  Button: (p: any) => {
    boundary.buttons.set(p.children, p.onClick);
    return <button {...p} />;
  },
}));
vi.mock("@/components/ui/input", () => ({
  Input: (p: any) => <input {...p} />,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (x: string) => x }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
import { OpenCodeLocalImportForm } from "../OpenCodeLocalImportForm";
import { OpenCodePersonalKeyForm } from "../OpenCodePersonalKeyForm";
import { OfficialApiPage } from "../OfficialApiPage";
const sample = {
  previewId: "ticket",
  entries: [{ id: "entry", label: "Work", isActive: true }],
  notices: [],
};
const deferred = () => {
  let resolve!: (v: any) => void;
  const promise = new Promise((r) => (resolve = r));
  return { promise, resolve };
};
beforeEach(() => {
  vi.clearAllMocks();
  boundary.buttons.clear();
  boundary.preview.mockResolvedValue(sample);
  boundary.cancel.mockResolvedValue(undefined);
  boundary.commit.mockResolvedValue({ rowId: 9, configurationReady: true });
  boundary.key.mockResolvedValue({ rowId: 9, configurationReady: true });
});
async function localReady() {
  render(<OpenCodeLocalImportForm onSaved={vi.fn()} />);
  fireEvent.click(screen.getByText("同意只读预览本机 OpenCode 凭据"));
  await screen.findByText(/Work/);
  return boundary.buttons.get("同意加密保存所选 API key")!;
}
it("retained save is revoked synchronously by cancel", async () => {
  const save = await localReady();
  act(() => {
    boundary.buttons.get("取消本次导入")!();
    save();
  });
  expect(boundary.commit).not.toHaveBeenCalled();
});
it("consumed ticket cannot be replayed through retained save", async () => {
  const save = await localReady();
  await act(async () => save());
  await act(async () => save());
  expect(boundary.commit).toHaveBeenCalledTimes(1);
});
it("unmounted personal key command cannot persist secret", () => {
  const { unmount } = render(<OpenCodePersonalKeyForm onSaved={vi.fn()} />);
  fireEvent.change(screen.getByLabelText("OpenCode Personal API key"), {
    target: { value: "secret" },
  });
  const save = boundary.buttons.get("同意加密保存并添加 OpenCode")!;
  unmount();
  act(() => save());
  expect(boundary.key).not.toHaveBeenCalled();
});
it("official login is singleflight even before a rerender", async () => {
  const pending = deferred();
  boundary.login.mockReturnValue(pending.promise);
  render(
    <OfficialApiPage
      sourceAppId="codex"
      vendorId="opencode"
      onBack={vi.fn()}
    />,
  );
  const button = screen.getByRole("button", { name: /官网账户登录/ });
  act(() => {
    button.click();
    button.click();
  });
  expect(boundary.login).toHaveBeenCalledTimes(1);
  await act(async () => pending.resolve(null));
});
it("source replacement rejects old official login completion", async () => {
  const pending = deferred();
  boundary.login.mockReturnValue(pending.promise);
  const connected = vi.fn();
  const { rerender } = render(
    <OfficialApiPage
      sourceAppId="codex"
      vendorId="opencode"
      onBack={vi.fn()}
      onConnected={connected}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: /官网账户登录/ }));
  rerender(
    <OfficialApiPage
      sourceAppId="gemini"
      vendorId="opencode"
      onBack={vi.fn()}
      onConnected={connected}
    />,
  );
  await act(async () => pending.resolve({ rowId: 8 }));
  expect(connected).not.toHaveBeenCalled();
});
import { Activity } from "react";
it("Activity hide retires pending preview and reveal permits a fresh read", async () => {
  const pending = deferred();
  boundary.preview.mockReturnValueOnce(pending.promise);
  const view = (mode: "visible" | "hidden") => (
    <Activity mode={mode}>
      <OpenCodeLocalImportForm onSaved={vi.fn()} />
    </Activity>
  );
  const { rerender } = render(view("visible"));
  fireEvent.click(screen.getByText("同意只读预览本机 OpenCode 凭据"));
  rerender(view("hidden"));
  await act(async () => pending.resolve(sample));
  expect(boundary.cancel).toHaveBeenCalledWith("ticket");
  rerender(view("visible"));
  expect(screen.queryByText(/Work/)).toBeNull();
  fireEvent.click(screen.getByText("同意只读预览本机 OpenCode 凭据"));
  await screen.findByText(/Work/);
  expect(boundary.preview).toHaveBeenCalledTimes(2);
});
it("Activity reveal of idle owner permits first read", async () => {
  const view = (mode: "visible" | "hidden") => (
    <Activity mode={mode}>
      <OpenCodeLocalImportForm onSaved={vi.fn()} />
    </Activity>
  );
  const { rerender } = render(view("visible"));
  rerender(view("hidden"));
  rerender(view("visible"));
  fireEvent.click(screen.getByText("同意只读预览本机 OpenCode 凭据"));
  await screen.findByText(/Work/);
});
it("personal key is wiped on Activity hide and old success cannot notify restored owner", async () => {
  const pending = deferred();
  boundary.key.mockReturnValueOnce(pending.promise);
  const saved = vi.fn();
  const view = (mode: "visible" | "hidden") => (
    <Activity mode={mode}>
      <OpenCodePersonalKeyForm onSaved={saved} />
    </Activity>
  );
  const { rerender } = render(view("visible"));
  fireEvent.change(screen.getByLabelText("OpenCode Personal API key"), {
    target: { value: "secret" },
  });
  fireEvent.click(screen.getByText("同意加密保存并添加 OpenCode"));
  rerender(view("hidden"));
  rerender(view("visible"));
  expect(
    (screen.getByLabelText("OpenCode Personal API key") as HTMLInputElement)
      .value,
  ).toBe("");
  await act(async () =>
    pending.resolve({ rowId: 9, configurationReady: true }),
  );
  expect(saved).not.toHaveBeenCalled();
});
it("local failure consumes ticket and does not leak raw error", async () => {
  boundary.commit.mockRejectedValueOnce("secret backend detail");
  const save = await localReady();
  await act(async () => save());
  await act(async () => save());
  expect(boundary.commit).toHaveBeenCalledTimes(1);
  expect(screen.queryByText(/secret backend detail/)).toBeNull();
});
it("official null cancellation offers no recovery or navigation", async () => {
  boundary.login.mockResolvedValue(null);
  const back = vi.fn();
  render(
    <OfficialApiPage sourceAppId="codex" vendorId="opencode" onBack={back} />,
  );
  fireEvent.click(screen.getByRole("button", { name: /官网账户登录/ }));
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: /官网账户登录/ })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  expect(back).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).toBeNull();
});
it("official error offers recovery without reading or saving keys", async () => {
  boundary.login.mockRejectedValue(new Error("login failed"));
  render(
    <OfficialApiPage
      sourceAppId="codex"
      vendorId="opencode"
      onBack={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: /官网账户登录/ }));
  await screen.findByRole("alert");
  expect(boundary.preview).not.toHaveBeenCalled();
  expect(boundary.key).not.toHaveBeenCalled();
  expect(boundary.commit).not.toHaveBeenCalled();
});
import { renderHook } from "@testing-library/react";
import { useOpenCodeLocalImportViewModel } from "../useOpenCodeLocalImportViewModel";
import { useOpenCodePersonalKeyViewModel } from "../useOpenCodePersonalKeyViewModel";
import { useOfficialApiPageViewModel } from "../useOfficialApiPageViewModel";
it("changing reviewed entry revokes the previous save command", async () => {
  boundary.preview.mockResolvedValue({
    ...sample,
    entries: [
      ...sample.entries,
      { id: "second", label: "Second", isActive: false },
    ],
  });
  const { result } = renderHook(() => useOpenCodeLocalImportViewModel(vi.fn()));
  await act(async () => result.current.read());
  act(() => result.current.setSelected("entry"));
  const old = result.current.save;
  act(() => result.current.setSelected("second"));
  await act(async () => old());
  expect(boundary.commit).not.toHaveBeenCalled();
  await act(async () => result.current.save());
  expect(boundary.commit).toHaveBeenCalledWith("ticket", "second", true);
});
it("editing key revokes retained save intent rather than saving a newer secret", async () => {
  const { result } = renderHook(() => useOpenCodePersonalKeyViewModel(vi.fn()));
  act(() => result.current.setKey("old"));
  const old = result.current.save;
  act(() => result.current.setKey("new"));
  await act(async () => old());
  expect(boundary.key).not.toHaveBeenCalled();
  await act(async () => result.current.save());
  expect(boundary.key).toHaveBeenCalledWith("new", "");
});
it("completed official handoff retires retained login command", async () => {
  boundary.login.mockResolvedValue({ rowId: 9 });
  const connected = vi.fn();
  const { result } = renderHook(() =>
    useOfficialApiPageViewModel({
      sourceAppId: "codex",
      onBack: vi.fn(),
      onConnected: connected,
    }),
  );
  const pick = result.current.handlePick;
  await act(async () => pick("opencode"));
  await act(async () => pick("opencode"));
  expect(boundary.login).toHaveBeenCalledTimes(1);
  expect(connected).toHaveBeenCalledTimes(1);
});
it("replacement source stays visibly busy until previous login settles", async () => {
  const pending = deferred();
  boundary.login.mockReturnValueOnce(pending.promise);
  const props = { onBack: vi.fn(), vendorId: "opencode" };
  const { rerender } = render(
    <OfficialApiPage {...props} sourceAppId="codex" />,
  );
  fireEvent.click(screen.getByRole("button", { name: /官网账户登录/ }));
  rerender(<OfficialApiPage {...props} sourceAppId="gemini" />);
  expect(screen.getByRole("button", { name: /官网账户登录/ })).toBeDisabled();
  await act(async () => pending.resolve(null));
  expect(screen.getByRole("button", { name: /官网账户登录/ })).toBeEnabled();
});
it("Back revokes in-flight login before navigation callback runs", async () => {
  const pending = deferred();
  boundary.login.mockReturnValueOnce(pending.promise);
  const connected = vi.fn();
  const back = vi.fn();
  render(
    <OfficialApiPage
      sourceAppId="codex"
      vendorId="opencode"
      onBack={back}
      onConnected={connected}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: /官网账户登录/ }));
  fireEvent.click(screen.getByRole("button", { name: "common.back" }));
  expect(back).toHaveBeenCalledTimes(1);
  await act(async () => pending.resolve({ rowId: 9 }));
  expect(connected).not.toHaveBeenCalled();
});
