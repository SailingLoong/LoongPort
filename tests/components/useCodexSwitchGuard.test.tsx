import { act, render, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useCodexSwitchGuard } from "@/components/relay/useCodexSwitchGuard";
import type { Provider } from "@/types";
import type { SwitchTierConfirmDialogProps } from "@/components/relay/SwitchTierConfirmDialog";

const dialogProps = vi.hoisted(() => ({
  current: null as null | SwitchTierConfirmDialogProps,
}));

vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: (props: typeof dialogProps.current) => {
    dialogProps.current = props;
    return null;
  },
}));

const provider: Provider = {
  id: "provider-1",
  name: "Backend-decided provider",
  settingsConfig: {},
  category: "custom",
};

describe("useCodexSwitchGuard", () => {
  beforeEach(() => {
    dialogProps.current = null;
  });

  it("opens confirmation only when the switch command requests it", async () => {
    const switchProvider = vi
      .fn()
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: provider.name,
      })
      .mockResolvedValueOnce({
        status: "switched",
        warnings: [],
        routingNotice: null,
      });
    const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
    const dialog = render(result.current.switchDialog);

    await act(async () => {
      await result.current.guardedSwitch(provider);
    });
    dialog.rerender(result.current.switchDialog);

    expect(switchProvider).toHaveBeenNthCalledWith(1, provider);
    expect(dialogProps.current?.targetName).toBe(provider.name);

    await act(async () => {
      dialogProps.current?.onSwitch(false);
    });

    expect(switchProvider).toHaveBeenNthCalledWith(2, provider, false);
  });

  it("does not reconstruct confirmation from app or cached status", async () => {
    const switchProvider = vi.fn().mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      routingNotice: null,
    });
    const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
    const dialog = render(result.current.switchDialog);

    await act(async () => {
      await result.current.guardedSwitch(provider);
    });
    dialog.rerender(result.current.switchDialog);

    expect(dialogProps.current?.targetName).toBeNull();
  });
});

it("refuses an already open confirmation after write admission is revoked", async () => {
  const switchProvider = vi.fn().mockResolvedValue({
    status: "confirmationRequired",
    targetName: provider.name,
  });
  const { result, rerender } = renderHook(
    ({ blocked }) => useCodexSwitchGuard(switchProvider, blocked),
    { initialProps: { blocked: false } },
  );
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  rerender({ blocked: true });
  render(result.current.switchDialog);
  await act(async () => {
    dialogProps.current?.onSwitch(false);
  });
  expect(switchProvider).toHaveBeenCalledTimes(1);
});

const catalog = {
  ownership: "unclaimedExternal" as const,
  choice: "keep" as const,
  canManage: true,
  revision: "catalog-v1",
  beforePointer: "/external/models.json",
  afterPointer: "/external/models.json",
};

it("reopens backend confirmation from final switch instead of reporting success", async () => {
  const switchProvider = vi.fn().mockResolvedValue({
    status: "confirmationRequired",
    targetName: provider.name,
    catalog,
    quitRequired: false,
  });
  const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
  const dialog = render(result.current.switchDialog);
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  dialog.rerender(result.current.switchDialog);
  await act(async () => {
    await dialogProps.current?.onSwitch(false, {
      choice: "keep",
      revision: "catalog-v1",
    });
  });
  dialog.rerender(result.current.switchDialog);
  expect(dialogProps.current?.targetName).toBe(provider.name);
  expect(dialogProps.current?.catalog).toEqual(catalog);
  expect(dialogProps.current?.quitRequired).toBe(false);
});

it("previews a catalog choice without applying and confirms with the last preview revision", async () => {
  const managed = {
    ...catalog,
    choice: "manage",
    afterPointer: "/managed/models.json",
  };
  const switchProvider = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog,
      quitRequired: false,
    })
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog: managed,
      quitRequired: false,
    })
    .mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      routingNotice: null,
    });
  const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
  const dialog = render(result.current.switchDialog);
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  dialog.rerender(result.current.switchDialog);
  const decision = { choice: "manage" as const, revision: "catalog-v1" };
  await act(async () => {
    await dialogProps.current?.onPreviewCatalog?.(decision);
  });
  dialog.rerender(result.current.switchDialog);
  expect(switchProvider).toHaveBeenNthCalledWith(
    2,
    provider,
    undefined,
    decision,
    true,
  );
  expect(dialogProps.current?.catalog).toEqual(managed);
  await act(async () => {
    await dialogProps.current?.onSwitch(false, decision);
  });
  dialog.rerender(result.current.switchDialog);
  expect(switchProvider).toHaveBeenNthCalledWith(
    3,
    provider,
    false,
    decision,
    false,
  );
  expect(dialogProps.current?.targetName).toBeNull();
});

it("invalidates source-changed catalog confirmation and does not replay stale approval", async () => {
  const switchProvider = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog,
      quitRequired: false,
    })
    .mockResolvedValueOnce(undefined);
  const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
  const dialog = render(result.current.switchDialog);
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  dialog.rerender(result.current.switchDialog);
  await act(async () => {
    await dialogProps.current?.onPreviewCatalog?.({
      choice: "manage",
      revision: "catalog-v1",
    });
  });
  dialog.rerender(result.current.switchDialog);
  expect(dialogProps.current?.targetName).toBeNull();
  await act(async () => {
    await dialogProps.current?.onSwitch(false);
  });
  expect(switchProvider).toHaveBeenCalledTimes(2);
});

it("serializes repeated catalog previews and ignores a response after cancellation", async () => {
  let finish!: (value: unknown) => void;
  const switchProvider = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog,
    })
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
  const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
  const dialog = render(result.current.switchDialog);
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  dialog.rerender(result.current.switchDialog);
  act(() => {
    void dialogProps.current?.onPreviewCatalog?.({
      choice: "manage",
      revision: "catalog-v1",
    });
    void dialogProps.current?.onPreviewCatalog?.({
      choice: "manage",
      revision: "catalog-v1",
    });
    dialogProps.current?.onCancel();
  });
  expect(switchProvider).toHaveBeenCalledTimes(2);
  await act(async () => {
    finish({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog: { ...catalog, choice: "manage" },
    });
  });
  dialog.rerender(result.current.switchDialog);
  expect(dialogProps.current?.targetName).toBeNull();
});

it("rejects a preview for another file revision instead of enabling final apply", async () => {
  const switchProvider = vi
    .fn()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog,
    })
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: provider.name,
      catalog: { ...catalog, revision: "catalog-v2", choice: "manage" },
    });
  const { result } = renderHook(() => useCodexSwitchGuard(switchProvider));
  const dialog = render(result.current.switchDialog);
  await act(async () => {
    await result.current.guardedSwitch(provider);
  });
  dialog.rerender(result.current.switchDialog);
  await act(async () => {
    await dialogProps.current?.onPreviewCatalog?.({
      choice: "manage",
      revision: "catalog-v1",
    });
  });
  dialog.rerender(result.current.switchDialog);
  expect(dialogProps.current?.targetName).toBeNull();
});
