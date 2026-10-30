import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useWorkBuddyNativePreviewViewModel } from "@/components/workbuddy/useWorkBuddyNativePreviewViewModel";
import { useMiniMaxNativeImportViewModel } from "@/components/providers/forms/useMiniMaxNativeImportViewModel";
const api = vi.hoisted(() => ({
  previewNative: vi.fn(),
  open: vi.fn(),
  previewNativeFile: vi.fn(),
  verifyNativeImport: vi.fn(),
  saveNativeImport: vi.fn(),
  cancelNativeImport: vi.fn(),
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("@/lib/api/minimaxCode", () => ({ minimaxCodeApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: api.open }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
const preview = {
  previewId: "p",
  source: "publicCliOAuthV1",
  region: "cn",
  maskedSubject: "f***r",
  expiresAtMs: 1900000000000,
  stage: "preview",
  verifiedUserId: null,
  label: null,
  canSave: false,
};
beforeEach(() => {
  vi.resetAllMocks();
  api.cancelNativeImport.mockResolvedValue(undefined);
  api.open.mockResolvedValue("/fixture/auth.json");
  api.previewNativeFile.mockResolvedValue(preview);
  api.verifyNativeImport.mockResolvedValue({
    ...preview,
    stage: "verified",
    verifiedUserId: "fixture",
    canSave: true,
  });
});
it("WorkBuddy exposes only reviewed observation fields and fences a late preview", async () => {
  api.previewNative.mockResolvedValue({
    sourceVerification: "knownLocationOnly",
    activationAvailable: false,
    logoutMarkerPresent: false,
    authFile: "missing",
    realm: null,
    token: "must-not-retain",
  });
  const { result, unmount } = renderHook(() =>
    useWorkBuddyNativePreviewViewModel(),
  );
  await act(async () => {
    await result.current.inspect();
  });
  expect(result.current.result).toEqual({
    sourceVerification: "knownLocationOnly",
    activationAvailable: false,
    logoutMarkerPresent: false,
    authFile: "missing",
    realm: null,
  });
  const pending = deferred<unknown>();
  api.previewNative.mockReturnValue(pending.promise);
  let done!: Promise<void>;
  act(() => {
    done = result.current.inspect();
    void result.current.inspect();
  });
  unmount();
  await act(async () => {
    pending.resolve({});
    await done;
  });
  expect(api.previewNative).toHaveBeenCalledTimes(2);
});
it("MiniMax ignores chooser completion after closing and never reads implicitly", async () => {
  const pending = deferred<string>();
  api.open.mockReturnValue(pending.promise);
  const { result } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn" }),
  );
  act(() => result.current.changeOpen(true));
  let done!: Promise<void>;
  act(() => {
    done = result.current.choose();
  });
  act(() => result.current.changeOpen(false));
  await act(async () => {
    pending.resolve("/fixture/late.json");
    await done;
  });
  expect(result.current.path).toBe("");
  expect(api.previewNativeFile).not.toHaveBeenCalled();
});
it("MiniMax uncertain save is not replayed and rereads the catalog without announcing success", async () => {
  const onSaved = vi.fn();
  api.saveNativeImport.mockRejectedValue(new Error("lost"));
  const { result } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn", onSaved }),
  );
  act(() => result.current.changeOpen(true));
  await act(async () => {
    await result.current.choose();
  });
  act(() => result.current.changeReadConsent(true));
  await act(async () => {
    await result.current.perform("read");
  });
  act(() => result.current.changeVerifyConsent(true));
  await act(async () => {
    await result.current.perform("verify");
  });
  act(() => result.current.changeSaveConsent(true));
  await act(async () => {
    const command = result.current.perform;
    await command("save");
    await command("save");
  });
  expect(api.saveNativeImport).toHaveBeenCalledTimes(1);
  expect(result.current.failed).toBe(true);
  expect(result.current.canSave).toBe(false);
  expect(result.current.preview).toBeNull();
  expect(result.current.path).toBe("");
  await act(async () => {
    await result.current.perform("save");
  });
  expect(api.saveNativeImport).toHaveBeenCalledTimes(1);
  expect(onSaved).toHaveBeenCalledTimes(1);
});

it.each(["close", "unmount", "region", "disabled"])(
  "MiniMax releases a preview arriving after %s",
  async (action) => {
    const pending = deferred<typeof preview>();
    api.previewNativeFile.mockReturnValue(pending.promise);
    const { result, rerender, unmount } = renderHook(
      ({ region, disabled }: { region: "cn" | "global"; disabled: boolean }) =>
        useMiniMaxNativeImportViewModel({ region, disabled }),
      { initialProps: { region: "cn" as "cn" | "global", disabled: false } },
    );
    act(() => result.current.changeOpen(true));
    await act(async () => {
      await result.current.choose();
    });
    act(() => result.current.changeReadConsent(true));
    let done!: Promise<void>;
    act(() => {
      done = result.current.perform("read");
      void result.current.perform("read");
    });
    expect(api.previewNativeFile).toHaveBeenCalledTimes(1);
    if (action === "unmount") unmount();
    else if (action === "region")
      rerender({ region: "global", disabled: false });
    else if (action === "disabled") rerender({ region: "cn", disabled: true });
    else act(() => result.current.changeOpen(false));
    await act(async () => {
      pending.resolve(preview);
      await done;
    });
    expect(api.cancelNativeImport).toHaveBeenCalledWith("p");
    expect(result.current.preview).toBeNull();
    expect(api.verifyNativeImport).not.toHaveBeenCalled();
  },
);

it("MiniMax successful save consumes its ticket and rereads rather than publishing its receipt", async () => {
  const onSaved = vi.fn();
  api.saveNativeImport.mockResolvedValue({ id: "untrusted-command-row" });
  const { result, unmount } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn", onSaved }),
  );
  act(() => result.current.changeOpen(true));
  await act(async () => {
    await result.current.choose();
  });
  act(() => result.current.changeReadConsent(true));
  await act(async () => {
    await result.current.perform("read");
  });
  act(() => result.current.changeVerifyConsent(true));
  await act(async () => {
    await result.current.perform("verify");
  });
  act(() => result.current.changeSaveConsent(true));
  await act(async () => {
    const command = result.current.perform;
    await command("save");
    await command("save");
  });
  expect(api.saveNativeImport).toHaveBeenCalledTimes(1);
  expect(onSaved).toHaveBeenCalledExactlyOnceWith();
  expect(result.current.visible).toBe(false);
  await act(async () => {
    await result.current.choose();
  });
  expect(api.open).toHaveBeenCalledTimes(1);
  expect(result.current.path).toBe("");
  expect(result.current.preview).toBeNull();
  unmount();
  expect(api.cancelNativeImport).not.toHaveBeenCalled();
});

it("MiniMax rejects a retained read command immediately after consent scope closes", async () => {
  const { result } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn" }),
  );
  act(() => result.current.changeOpen(true));
  await act(async () => {
    await result.current.choose();
  });
  act(() => result.current.changeReadConsent(true));
  await act(async () => {
    const read = result.current.perform;
    result.current.changeOpen(false);
    await read("read");
  });
  expect(api.previewNativeFile).not.toHaveBeenCalled();
});
it("MiniMax rejects retained chooser after close and unmount", async () => {
  const { result, unmount } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn" }),
  );
  act(() => result.current.changeOpen(true));
  const choose = result.current.choose;
  act(() => result.current.changeOpen(false));
  await act(async () => {
    await choose();
  });
  unmount();
  await choose();
  expect(api.open).not.toHaveBeenCalled();
});
it("WorkBuddy rejects retained inspection after unmount", async () => {
  const { result, unmount } = renderHook(() =>
    useWorkBuddyNativePreviewViewModel(),
  );
  const inspect = result.current.inspect;
  unmount();
  await inspect();
  expect(api.previewNative).not.toHaveBeenCalled();
});

it("MiniMax does not reuse a retained read command under a reopened permission scope", async () => {
  const { result } = renderHook(() =>
    useMiniMaxNativeImportViewModel({ region: "cn" }),
  );
  act(() => result.current.changeOpen(true));
  await act(async () => {
    await result.current.choose();
  });
  act(() => result.current.changeReadConsent(true));
  const read = result.current.perform;
  act(() => result.current.changeOpen(false));
  act(() => result.current.changeOpen(true));
  api.open.mockResolvedValue("/fixture/new.json");
  await act(async () => {
    await result.current.choose();
  });
  act(() => result.current.changeReadConsent(true));
  await act(async () => {
    await read("read");
  });
  expect(api.previewNativeFile).not.toHaveBeenCalled();
});

it.each(["verify", "save"] as const)(
  "MiniMax honors immediate withdrawal of %s consent",
  async (step) => {
    const { result } = renderHook(() =>
      useMiniMaxNativeImportViewModel({ region: "cn" }),
    );
    act(() => result.current.changeOpen(true));
    await act(async () => {
      await result.current.choose();
    });
    act(() => result.current.changeReadConsent(true));
    await act(async () => {
      await result.current.perform("read");
    });
    act(() => result.current.changeVerifyConsent(true));
    if (step === "save") {
      await act(async () => {
        await result.current.perform("verify");
      });
      act(() => result.current.changeSaveConsent(true));
    }
    await act(async () => {
      const command = result.current.perform;
      if (step === "verify") result.current.changeVerifyConsent(false);
      else result.current.changeSaveConsent(false);
      await command(step);
    });
    if (step === "verify")
      expect(api.verifyNativeImport).not.toHaveBeenCalled();
    else expect(api.saveNativeImport).not.toHaveBeenCalled();
  },
);
