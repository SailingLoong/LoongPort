import { act, render, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useCodexSwitchGuard } from "@/components/relay/useCodexSwitchGuard";
import type { Provider } from "@/types";

const dialogProps = vi.hoisted(() => ({
  current: null as null | {
    targetName: string | null;
    onCancel: () => void;
    onSwitch: (quitChatgpt: boolean) => void;
  },
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
