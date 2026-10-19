import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { WindowSettings } from "@/components/settings/WindowSettings";
import type { SettingsFormState } from "@/hooks/useSettings";

function Fixture({ initialId }: { initialId?: string }) {
  const [settings, setSettings] = useState<SettingsFormState>({
    enableAnonymousStats: true,
    statsInstallId: initialId,
  } as SettingsFormState);
  return (
    <>
      <WindowSettings
        settings={settings}
        onChange={(updates) =>
          setSettings((current) => ({ ...current, ...updates }))
        }
      />
      <output aria-label="install-id">
        {settings.statsInstallId ?? "absent"}
      </output>
    </>
  );
}

describe("WindowSettings installation identity", () => {
  it("keeps the installation identity while statistics are off and after reenabling", () => {
    render(<Fixture initialId="persisted-installation" />);
    const stats = screen.getByRole("switch", {
      name: "settings.anonymousStats",
    });

    fireEvent.click(stats);
    expect(stats).not.toBeChecked();
    expect(screen.getByLabelText("install-id")).toHaveTextContent(
      "persisted-installation",
    );

    fireEvent.click(stats);
    expect(stats).toBeChecked();
    expect(screen.getByLabelText("install-id")).toHaveTextContent(
      "persisted-installation",
    );
  });

  it("sends only the statistics choice, leaving backend-owned identity alone", () => {
    const onChange = vi.fn();
    render(
      <WindowSettings
        settings={
          {
            enableAnonymousStats: true,
            statsInstallId: "existing",
          } as SettingsFormState
        }
        onChange={onChange}
      />,
    );
    fireEvent.click(
      screen.getByRole("switch", { name: "settings.anonymousStats" }),
    );
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      enableAnonymousStats: false,
    });
    expect(onChange.mock.calls[0][0]).not.toHaveProperty("statsInstallId");
  });

  it("never creates an installation identity through a statistics toggle", () => {
    render(<Fixture />);
    const stats = screen.getByRole("switch", {
      name: "settings.anonymousStats",
    });
    fireEvent.click(stats);
    fireEvent.click(stats);
    expect(screen.getByLabelText("install-id")).toHaveTextContent("absent");
  });
});
