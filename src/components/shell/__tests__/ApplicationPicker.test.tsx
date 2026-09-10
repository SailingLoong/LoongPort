import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  APP_IDS,
  APP_DISPLAY_NAME,
  DEFAULT_VISIBLE_APPS,
} from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { ApplicationPicker } from "../ApplicationPicker";

afterEach(() => vi.restoreAllMocks());

describe("ApplicationPicker", () => {
  it("keeps all nine applications selectable under More without changing hidden preferences", async () => {
    const preferences = Object.freeze(
      APP_IDS.reduce((prefs, app) => ({ ...prefs, [app]: false }), {
        ...DEFAULT_VISIBLE_APPS,
      }),
    );
    const onSwitch = vi.fn();
    function Harness() {
      const [app, setApp] = useState<AppId>("codex");
      return (
        <ApplicationPicker
          activeApp={app}
          visibleApps={preferences}
          onSwitch={(selected) => {
            onSwitch(selected);
            setApp(selected);
          }}
        />
      );
    }
    const user = userEvent.setup();
    const writeStorage = vi.spyOn(Storage.prototype, "setItem");
    render(<Harness />);
    const applications = APP_IDS.filter((app) => app !== "codex-image");
    expect(applications).toHaveLength(9);
    for (const app of applications) {
      await user.click(
        screen.getByRole("combobox", { name: "client.selectApplication" }),
      );
      const more = screen.getByRole("group", { name: "client.moreApps" });
      expect(within(more).getAllByRole("option")).toHaveLength(9);
      expect(
        screen.queryByRole("option", { name: APP_DISPLAY_NAME["codex-image"] }),
      ).not.toBeInTheDocument();
      await user.click(
        within(more).getByRole("option", { name: APP_DISPLAY_NAME[app] }),
      );
      expect(onSwitch).toHaveBeenLastCalledWith(app);
      expect(screen.getByRole("combobox")).toHaveTextContent(
        APP_DISPLAY_NAME[app],
      );
    }
    expect(onSwitch.mock.calls.map(([app]) => app)).toEqual(applications);
    expect(Object.values(preferences).every((visible) => !visible)).toBe(true);
    expect(writeStorage).not.toHaveBeenCalled();
  });
});
