import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const checkUpdate = vi.fn();
  return {
    checkUpdates: vi.fn(),
    value: {
      hasUpdate: false,
      updateInfo: null,
      isChecking: false,
      error: "network offline",
      isDismissed: false,
      dismissUpdate: vi.fn(),
      checkUpdate,
      resetDismiss: vi.fn(),
    },
  };
});

afterEach(() => {
  mocks.value.checkUpdate.mockReset();
  mocks.checkUpdates.mockReset();
});

vi.mock("@/contexts/UpdateContext", () => ({
  useUpdate: () => mocks.value,
}));

vi.mock("@/lib/api", () => ({
  settingsApi: {
    getToolVersions: vi.fn().mockResolvedValue([]),
    openExternal: vi.fn(),
    checkUpdates: mocks.checkUpdates,
    installUpdateAndRestart: vi.fn(),
    probeToolInstallations: vi.fn().mockResolvedValue([]),
    runToolLifecycleAction: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("3.24.0"),
}));

import { AboutSection } from "@/components/settings/AboutSection";

describe("AboutSection", () => {
  it("displays the current manual update-check error", async () => {
    render(<AboutSection isPortable={false} />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "network offline",
    );
  });

  it("keeps the Releases fallback when a manual check rejects", async () => {
    mocks.value.checkUpdate.mockRejectedValue(new Error("network offline"));
    render(<AboutSection isPortable={false} />);

    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.checkForUpdates",
      }),
    );

    await waitFor(() => expect(mocks.checkUpdates).toHaveBeenCalledOnce());
  });
});
