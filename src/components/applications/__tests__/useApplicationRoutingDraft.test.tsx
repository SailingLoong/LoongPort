import { act, renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useApplicationRoutingDraft } from "../useApplicationRoutingDraft";
import type { SwitchTierCommandResult } from "@/lib/api/relay";
const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: toastMocks }));
const changed = {
  order: { profileName: "Travel", providerIds: ["b"] },
  selection: { providerId: "b", model: "model-b" },
};
it("serializes duplicate submissions and suppresses confirmation after unmount", async () => {
  let resolve!: (value: SwitchTierCommandResult) => void;
  const apply = vi.fn(
    () =>
      new Promise<SwitchTierCommandResult>((done) => {
        resolve = done;
      }),
  );
  const { result, unmount } = renderHook(() =>
    useApplicationRoutingDraft(apply),
  );
  let request!: Promise<void>;
  act(() => {
    request = result.current.submit(changed);
  });
  expect(result.current.submitting).toBe(true);
  await act(async () => {
    await result.current.submit(changed);
  });
  expect(apply).toHaveBeenCalledTimes(1);
  unmount();
  await act(async () => {
    resolve({ status: "confirmationRequired", targetName: "Example" });
    await request;
  });
  expect(apply).toHaveBeenCalledTimes(1);
  expect(toastMocks.success).not.toHaveBeenCalled();
});
it("replays the frozen order and model intent after explicit confirmation", async () => {
  const apply = vi.fn().mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Example",
  });
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  await act(async () => {
    await result.current.submit(changed);
  });
  act(() => {
    result.current.load("Home", ["a"]);
  });
  await act(async () => {
    result.current.confirm(false);
  });
  expect(apply).toHaveBeenLastCalledWith(changed, false);
});

it("rechecks reviewed facts before continuing the original native confirmation", async () => {
  let current = true;
  const apply = vi.fn().mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Example",
  });
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  await act(async () => {
    await result.current.submit(changed, undefined, () => current);
  });
  current = false;
  await act(async () => {
    result.current.confirm(false);
  });
  expect(apply).toHaveBeenCalledTimes(1);
  expect(result.current.confirmation).toBeNull();
});

const catalog = {
  ownership: "unclaimedExternal" as const,
  choice: "keep" as const,
  canManage: true,
  revision: "catalog-v1",
  beforePointer: "/external/models.json",
  afterPointer: "/external/models.json",
};

it("previews catalog ownership without committing frozen order/model and uses that preview on apply", async () => {
  const decision = { choice: "manage" as const, revision: "catalog-v1" };
  const managed = {
    ...catalog,
    choice: "manage",
    afterPointer: "/managed/models.json",
  };
  const apply = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Example",
      catalog,
      quitRequired: false,
    })
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Example",
      catalog: managed,
      quitRequired: false,
    })
    .mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      providerName: "Example",
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    });
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  act(() => {
    result.current.load("Travel", ["b"]);
  });
  await act(async () => {
    await result.current.submit(changed);
  });
  expect(result.current.confirmation?.catalog).toEqual(catalog);
  await act(async () => {
    await result.current.previewCatalog(decision);
  });
  expect(apply).toHaveBeenNthCalledWith(
    2,
    { ...changed, catalogDecision: decision, catalogPreviewOnly: true },
    undefined,
  );
  expect(result.current.stagedIds).toEqual(["b"]);
  expect(toastMocks.success).not.toHaveBeenCalled();
  expect(result.current.confirmation?.catalog).toEqual(managed);
  await act(async () => {
    await result.current.confirm(false, decision);
  });
  expect(apply).toHaveBeenNthCalledWith(
    3,
    { ...changed, catalogDecision: decision, catalogPreviewOnly: false },
    false,
  );
  expect(toastMocks.success).toHaveBeenCalledOnce();
  expect(result.current.stagedIds).toBeNull();
});

it("invalidates a catalog preview on cancellation while its request is pending", async () => {
  let finish!: (value: SwitchTierCommandResult) => void;
  const apply = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Example",
      catalog,
    })
    .mockImplementationOnce(
      () =>
        new Promise<SwitchTierCommandResult>((resolve) => {
          finish = resolve;
        }),
    );
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  await act(async () => {
    await result.current.submit(changed);
  });
  let request!: Promise<void>;
  act(() => {
    request = result.current.previewCatalog({
      choice: "manage",
      revision: "catalog-v1",
    });
  });
  act(() => {
    result.current.cancel();
  });
  await act(async () => {
    finish({
      status: "confirmationRequired",
      targetName: "Example",
      catalog: { ...catalog, choice: "manage" },
    });
    await request;
  });
  expect(result.current.confirmation).toBeNull();
  expect(toastMocks.success).not.toHaveBeenCalled();
});

it("drops catalog approval on sourceChanged and preserves local order for a fresh review", async () => {
  const apply = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Example",
      catalog,
    })
    .mockRejectedValueOnce(new Error("sourceChanged"));
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  act(() => {
    result.current.load("Travel", ["b"]);
  });
  await act(async () => {
    await result.current.submit(changed);
  });
  await act(async () => {
    await result.current.confirm(false, {
      choice: "keep",
      revision: "catalog-v1",
    });
  });
  expect(result.current.confirmation).toBeNull();
  expect(result.current.stagedIds).toEqual(["b"]);
  await act(async () => {
    await result.current.confirm(false, {
      choice: "keep",
      revision: "catalog-v1",
    });
  });
  expect(apply).toHaveBeenCalledTimes(2);
  expect(toastMocks.success).not.toHaveBeenCalled();
});

it("keeps a newer draft when the submitted intent completes", async () => {
  let finish!: (value: SwitchTierCommandResult) => void;
  const apply = vi.fn(
    () =>
      new Promise<SwitchTierCommandResult>((resolve) => {
        finish = resolve;
      }),
  );
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  act(() => result.current.load("Travel", ["b"]));
  let submitted!: Promise<void>;
  act(() => {
    submitted = result.current.submit(changed);
  });
  act(() => {
    result.current.load("New draft", ["a"]);
    result.current.setModelFilter("model-d");
  });
  await act(async () => {
    finish({
      status: "switched",
      providerName: "Synthetic",
      warnings: [],
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    });
    await submitted;
  });
  act(() => result.current.discardSubmitted());
  expect(result.current.stagedIds).toEqual(["a"]);
  expect(result.current.modelFilter).toEqual("model-d");
});

it("preserves staged order and filters when a selection-only request completes", async () => {
  const apply = vi.fn().mockResolvedValue({
    status: "switched",
    providerName: "Synthetic",
    warnings: [],
    chatgptWasRunning: false,
    chatgptRelaunched: false,
  });
  const { result } = renderHook(() => useApplicationRoutingDraft(apply));
  act(() => {
    result.current.load("Travel", ["b", "a"]);
    result.current.setSearch("Synthetic");
    result.current.setAccountFilter("relay:1");
    result.current.setModelFilter("staged-model");
  });
  await act(async () => {
    await result.current.submit({ selection: { providerId: "b" } });
  });
  // Workspace calls this for the backend's completed original-request receipt.
  act(() => result.current.discardSubmitted());
  expect(result.current.stagedIds).toEqual(["b", "a"]);
  expect(result.current.profileName).toBe("Travel");
  expect(result.current.search).toBe("Synthetic");
  expect(result.current.accountFilter).toBe("relay:1");
  expect(result.current.modelFilter).toBe("staged-model");
});
