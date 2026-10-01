import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppSwitcher } from "@/components/AppSwitcher";
import { AppVisibilitySettings } from "@/components/settings/AppVisibilitySettings";
import { APP_ICON_MAP, DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import { getIcon, getIconMetadata } from "@/icons/extracted";

// Original mark from the ZCode homepage: https://zcode.z.ai/cn
const officialPaths = [
  "M134.4 0.130152L116.48 25.6022C113.665 29.5699 109.054 32.0019 104.064 32.0019H6.3999V0C6.3999 0.130149 134.4 0.130152 134.4 0.130152Z",
  "M256 0.130127L102.401 217.732H0L153.599 0.130127H256Z",
  "M121.601 217.732L139.65 192.134C142.465 188.166 147.076 185.734 152.067 185.734H249.604V217.736H121.601V217.732Z",
];

function expectOfficialMark(container: ParentNode) {
  const svg = container.querySelector("svg");
  expect(svg?.getAttribute("viewBox")).toBe("0 0 256 218");
  const paths = Array.from(svg!.querySelectorAll("path"));
  expect(paths.map((path) => path.getAttribute("d"))).toEqual(officialPaths);
  for (const path of paths)
    expect(path.getAttribute("fill")).toBe("currentColor");
}

describe("ZCode official icon", () => {
  it("keeps the official geometry and follows the surrounding theme color", () => {
    const document = new DOMParser().parseFromString(
      getIcon("zcode"),
      "image/svg+xml",
    );
    expectOfficialMark(document);
    expect(getIconMetadata("zcode")?.defaultColor).toBe("currentColor");
  });

  it("uses the official mark in the shared app icon registry", () => {
    const { container } = render(APP_ICON_MAP.zcode.icon);
    expectOfficialMark(container);
  });

  it.each(["zcode", "claude"] as const)(
    "uses the official mark in the app tab when %s is active",
    (activeApp) => {
      render(
        <AppSwitcher
          activeApp={activeApp}
          onSwitch={vi.fn()}
          visibleApps={{ ...DEFAULT_VISIBLE_APPS, zcode: true }}
        />,
      );
      expectOfficialMark(screen.getByRole("button", { name: "ZCode" }));
    },
  );

  it("uses the official mark in the add-app menu", () => {
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        onShowApp={vi.fn()}
        visibleApps={DEFAULT_VISIBLE_APPS}
      />,
    );
    fireEvent.click(screen.getByTitle("appSwitcher.add"));
    expectOfficialMark(screen.getByRole("button", { name: "ZCode" }));
  });

  it("uses the official mark in app visibility settings", () => {
    render(
      <AppVisibilitySettings
        settings={{
          visibleApps: DEFAULT_VISIBLE_APPS,
          showInTray: true,
          minimizeToTrayOnClose: false,
          preserveCodexOfficialAuthOnSwitch: true,
          unifyCodexSessionHistory: true,
          language: "en",
        }}
        onChange={vi.fn()}
      />,
    );
    expectOfficialMark(screen.getByRole("button", { name: "apps.zcode" }));
  });
});
