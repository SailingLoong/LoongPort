import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import i18n from "i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MiniMaxNativeImport } from "@/components/providers/forms/MiniMaxNativeImport";
import type {
  MiniMaxCodeNativePreview,
  MiniMaxCodeAccount,
} from "@/lib/api/minimaxCode";
import en from "@/i18n/locales/en.json";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  previewNativeFile: vi.fn(),
  verifyNativeImport: vi.fn(),
  saveNativeImport: vi.fn(),
  cancelNativeImport: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("@/lib/api/minimaxCode", () => ({ minimaxCodeApi: mocks }));
const preview: MiniMaxCodeNativePreview = {
  previewId: "synthetic-preview",
  source: "publicCliOAuthV1",
  region: "cn",
  maskedSubject: "f***r",
  expiresAtMs: 1900000000000,
  stage: "preview",
  verifiedUserId: null,
  label: null,
  canSave: false,
};
const verified: MiniMaxCodeNativePreview = {
  ...preview,
  stage: "verified",
  verifiedUserId: "verified-user",
  label: "Verified account",
  canSave: true,
};
const saved = { id: "saved-account" } as MiniMaxCodeAccount;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
const button = (name: string) => screen.getByRole("button", { name });
const check = (name: string) =>
  fireEvent.click(screen.getByRole("checkbox", { name }));
async function choose() {
  fireEvent.click(button("Import local MiniMax CLI account"));
  fireEvent.click(button("Choose auth.json"));
  await screen.findByDisplayValue(
    "/fixture/.minimax/auth/prod/cn/mcode-public/auth.json",
  );
}
async function read() {
  await choose();
  check("Allow a local read of the selected file");
  fireEvent.click(button("Read local preview"));
  await screen.findByText("Stored access expiry", { exact: false });
}
async function verify() {
  await read();
  check(
    "Send this account’s access token to official MiniMax in the selected region to verify its profile",
  );
  fireEvent.click(button("Verify account online"));
  await screen.findByText("verified-user");
}
beforeEach(async () => {
  vi.restoreAllMocks();
  await i18n.changeLanguage("en");
  i18n.addResourceBundle("en", "translation", en, true, true);
  mocks.open
    .mockReset()
    .mockResolvedValue("/fixture/.minimax/auth/prod/cn/mcode-public/auth.json");
  mocks.previewNativeFile.mockReset().mockResolvedValue(preview);
  mocks.verifyNativeImport.mockReset().mockResolvedValue(verified);
  mocks.saveNativeImport.mockReset().mockResolvedValue(saved);
  mocks.cancelNativeImport.mockReset().mockResolvedValue(undefined);
});
describe("MiniMax access-only import", () => {
  it("separates selection, local read, identity verification and encrypted save", async () => {
    const onSaved = vi.fn();
    render(<MiniMaxNativeImport region="cn" onSaved={onSaved} />);
    await choose();
    expect(mocks.previewNativeFile).not.toHaveBeenCalled();
    expect(button("Read local preview")).toBeDisabled();
    check("Allow a local read of the selected file");
    fireEvent.click(button("Read local preview"));
    await screen.findByText("f***r");
    expect(mocks.previewNativeFile).toHaveBeenCalledExactlyOnceWith(
      "/fixture/.minimax/auth/prod/cn/mcode-public/auth.json",
      "cn",
      true,
    );
    expect(mocks.verifyNativeImport).not.toHaveBeenCalled();
    expect(mocks.saveNativeImport).not.toHaveBeenCalled();
    expect(button("Verify account online")).toBeDisabled();
    expect(button("Save encrypted access-only account")).toBeDisabled();
    check(
      "Send this account’s access token to official MiniMax in the selected region to verify its profile",
    );
    fireEvent.click(button("Verify account online"));
    await screen.findByText("verified-user");
    expect(mocks.verifyNativeImport).toHaveBeenCalledExactlyOnceWith(
      "synthetic-preview",
      true,
    );
    expect(mocks.saveNativeImport).not.toHaveBeenCalled();
    expect(button("Save encrypted access-only account")).toBeDisabled();
    check(
      "Save a copy of this access token in LoongPort’s encrypted account vault",
    );
    fireEvent.click(button("Save encrypted access-only account"));
    await waitFor(() => expect(onSaved).toHaveBeenCalledExactlyOnceWith());
    expect(mocks.saveNativeImport).toHaveBeenCalledExactlyOnceWith(
      "synthetic-preview",
      true,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("accepts a missing local subject without fabricating identity", async () => {
    mocks.previewNativeFile.mockResolvedValueOnce({
      ...preview,
      maskedSubject: null,
    });
    render(<MiniMaxNativeImport region="cn" />);
    await read();
    expect(screen.queryByText("f***r")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    check(
      "Send this account’s access token to official MiniMax in the selected region to verify its profile",
    );
    fireEvent.click(button("Verify account online"));
    await screen.findByText("verified-user");
  });
  it.each(["close", "unmount", "region", "disabled"])(
    "cleans up a preview arriving after %s",
    async (action) => {
      const pending = deferred<MiniMaxCodeNativePreview>();
      mocks.previewNativeFile.mockReturnValueOnce(pending.promise);
      const view = render(<MiniMaxNativeImport region="cn" />);
      await choose();
      check("Allow a local read of the selected file");
      fireEvent.click(button("Read local preview"));
      fireEvent.click(button("Read local preview"));
      expect(mocks.previewNativeFile).toHaveBeenCalledTimes(1);
      if (action === "unmount") view.unmount();
      else if (action === "region")
        view.rerender(<MiniMaxNativeImport region="global" />);
      else if (action === "disabled")
        view.rerender(<MiniMaxNativeImport region="cn" disabled />);
      else fireEvent.click(button("Cancel import"));
      await act(async () => pending.resolve(preview));
      expect(mocks.cancelNativeImport).toHaveBeenCalledWith(
        "synthetic-preview",
      );
      expect(screen.queryByText("f***r")).not.toBeInTheDocument();
      expect(mocks.verifyNativeImport).not.toHaveBeenCalled();
    },
  );
  it("cancels the old preview and resets every permission when selecting another file", async () => {
    render(<MiniMaxNativeImport region="cn" />);
    await verify();
    mocks.open.mockResolvedValueOnce(
      "/fixture/other/auth/prod/cn/mcode-public/auth.json",
    );
    fireEvent.click(button("Choose auth.json"));
    await screen.findByDisplayValue(
      "/fixture/other/auth/prod/cn/mcode-public/auth.json",
    );
    expect(mocks.cancelNativeImport).toHaveBeenCalledWith("synthetic-preview");
    expect(screen.queryByText("verified-user")).not.toBeInTheDocument();
    for (const checkbox of screen.getAllByRole("checkbox"))
      expect(checkbox).not.toBeChecked();
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
  it.each(["verify", "save"])(
    "does not display a late %s result under a newly opened selection",
    async (step) => {
      const pending = deferred<MiniMaxCodeNativePreview | MiniMaxCodeAccount>();
      const onSaved = vi.fn();
      render(<MiniMaxNativeImport region="cn" onSaved={onSaved} />);
      if (step === "verify") {
        await read();
        mocks.verifyNativeImport.mockReturnValueOnce(pending.promise);
        check(
          "Send this account’s access token to official MiniMax in the selected region to verify its profile",
        );
        fireEvent.click(button("Verify account online"));
      } else {
        await verify();
        mocks.saveNativeImport.mockReturnValueOnce(pending.promise);
        check(
          "Save a copy of this access token in LoongPort’s encrypted account vault",
        );
        fireEvent.click(button("Save encrypted access-only account"));
      }
      fireEvent.click(button("Cancel import"));
      await choose();
      await act(async () =>
        pending.resolve(step === "verify" ? verified : saved),
      );
      expect(screen.queryByText("verified-user")).not.toBeInTheDocument();
      expect(button("Save encrypted access-only account")).toBeDisabled();
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    },
  );
  it("blocks saving after the source expiry without extending it", async () => {
    render(<MiniMaxNativeImport region="cn" />);
    await verify();
    check(
      "Save a copy of this access token in LoongPort’s encrypted account vault",
    );
    vi.spyOn(Date, "now").mockReturnValue(preview.expiresAtMs + 1);
    fireEvent.click(button("Save encrypted access-only account"));
    expect(mocks.saveNativeImport).not.toHaveBeenCalled();
    expect(button("Save encrypted access-only account")).toBeDisabled();
    expect(
      screen.getByText(/Sign in again in the CLI and explicitly reimport/),
    ).toBeInTheDocument();
  });
  it("does not expose backend errors or retain malformed identity responses", async () => {
    mocks.verifyNativeImport.mockRejectedValueOnce(
      new Error("synthetic-secret-path-and-token"),
    );
    render(<MiniMaxNativeImport region="cn" />);
    await read();
    check(
      "Send this account’s access token to official MiniMax in the selected region to verify its profile",
    );
    fireEvent.click(button("Verify account online"));
    await screen.findByRole("alert");
    expect(screen.queryByText(/synthetic-secret/)).not.toBeInTheDocument();
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
  it("does not read when the file picker is canceled or returns after close", async () => {
    const pending = deferred<string | null>();
    mocks.open.mockReturnValueOnce(pending.promise);
    render(<MiniMaxNativeImport region="cn" />);
    fireEvent.click(button("Import local MiniMax CLI account"));
    fireEvent.click(button("Choose auth.json"));
    fireEvent.click(button("Cancel import"));
    await act(async () =>
      pending.resolve("/fixture/late/auth/prod/cn/mcode-public/auth.json"),
    );
    fireEvent.click(button("Import local MiniMax CLI account"));
    expect(
      screen.getByRole("textbox", { name: "Selected source file" }),
    ).toHaveValue("");
    expect(mocks.previewNativeFile).not.toHaveBeenCalled();
  });
  it.each([
    { canSave: true },
    { source: "unsupportedDesktop" },
    { region: "global" },
    { stage: "verified" },
    { maskedSubject: "too-long-to-be-a-masked-subject" },
  ])("discards a malformed local projection %j", async (patch) => {
    mocks.previewNativeFile.mockResolvedValueOnce({ ...preview, ...patch });
    render(<MiniMaxNativeImport region="cn" />);
    await choose();
    check("Allow a local read of the selected file");
    fireEvent.click(button("Read local preview"));
    await screen.findByRole("alert");
    expect(mocks.cancelNativeImport).toHaveBeenCalledWith("synthetic-preview");
    expect(button("Verify account online")).toBeDisabled();
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
  it("rejects a non-boolean verified capability rather than enabling save", async () => {
    mocks.verifyNativeImport.mockResolvedValueOnce({
      ...verified,
      canSave: "true",
    });
    render(<MiniMaxNativeImport region="cn" />);
    await read();
    check(
      "Send this account’s access token to official MiniMax in the selected region to verify its profile",
    );
    fireEvent.click(button("Verify account online"));
    await screen.findByRole("alert");
    expect(button("Save encrypted access-only account")).toBeDisabled();
    expect(mocks.saveNativeImport).not.toHaveBeenCalled();
  });
  it("discards verification that extends the original source expiry", async () => {
    mocks.verifyNativeImport.mockResolvedValueOnce({
      ...verified,
      expiresAtMs: preview.expiresAtMs + 1,
    });
    render(<MiniMaxNativeImport region="cn" />);
    await read();
    check(
      "Send this account’s access token to official MiniMax in the selected region to verify its profile",
    );
    fireEvent.click(button("Verify account online"));
    await screen.findByRole("alert");
    expect(mocks.cancelNativeImport).toHaveBeenCalledWith("synthetic-preview");
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
  it("revoking local read consent clears verified identity and later permissions", async () => {
    render(<MiniMaxNativeImport region="cn" />);
    await verify();
    check(
      "Save a copy of this access token in LoongPort’s encrypted account vault",
    );
    check("Allow a local read of the selected file");
    expect(screen.queryByText("verified-user")).not.toBeInTheDocument();
    expect(mocks.cancelNativeImport).toHaveBeenCalledWith("synthetic-preview");
    for (const checkbox of screen.getAllByRole("checkbox"))
      expect(checkbox).not.toBeChecked();
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
  it("cancelling the native picker does not grant read consent", async () => {
    mocks.open.mockResolvedValueOnce(null);
    render(<MiniMaxNativeImport region="cn" />);
    fireEvent.click(button("Import local MiniMax CLI account"));
    fireEvent.click(button("Choose auth.json"));
    await waitFor(() => expect(button("Choose auth.json")).toBeEnabled());
    expect(button("Read local preview")).toBeDisabled();
    expect(mocks.previewNativeFile).not.toHaveBeenCalled();
  });
  it("never refreshes or publishes a late save after the selected region changes", async () => {
    const pending = deferred<MiniMaxCodeAccount>();
    const onSaved = vi.fn();
    mocks.saveNativeImport.mockReturnValueOnce(pending.promise);
    const view = render(<MiniMaxNativeImport region="cn" onSaved={onSaved} />);
    await verify();
    check(
      "Save a copy of this access token in LoongPort’s encrypted account vault",
    );
    fireEvent.click(button("Save encrypted access-only account"));
    view.rerender(<MiniMaxNativeImport region="global" onSaved={onSaved} />);
    await act(async () => pending.resolve(saved));
    fireEvent.click(button("Import local MiniMax CLI account"));
    expect(onSaved).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toHaveTextContent(
      "Account region: Global",
    );
    expect(screen.queryByText("verified-user")).not.toBeInTheDocument();
    expect(button("Save encrypted access-only account")).toBeDisabled();
  });
});
