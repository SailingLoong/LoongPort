import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppSwitcher } from "@/components/AppSwitcher";
import {
  APP_IDS,
  DEFAULT_VISIBLE_APPS,
  MCP_APP_IDS,
  SKILLS_APP_IDS,
  isAdditiveAppId,
  isProxyAppId,
  usesProviderStore,
} from "@/config/appConfig";
import type { AppId } from "@/lib/api";

describe("MiniMax Code application", () => {
  it("registers a provider-backed additive app, not an account or proxy capability", () => {
    expect(APP_IDS).toContain("mcode");
    expect(APP_IDS).not.toContain("minimax_code");
    expect(usesProviderStore("mcode" as AppId)).toBe(true);
    expect(isAdditiveAppId("mcode")).toBe(true);
    expect(isProxyAppId("mcode")).toBe(false);
    expect(MCP_APP_IDS).not.toContain("mcode");
    expect(SKILLS_APP_IDS).not.toContain("mcode");
  });

  it("starts hidden and can be added from the existing + menu", () => {
    const onShowApp = vi.fn();
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        onShowApp={onShowApp}
        visibleApps={DEFAULT_VISIBLE_APPS}
      />,
    );
    expect(
      screen.queryByRole("button", { name: /MiniMax Code/ }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByTitle("appSwitcher.add"));
    fireEvent.click(screen.getByRole("button", { name: /MiniMax Code/ }));
    expect(onShowApp).toHaveBeenCalledWith("mcode");
  });
});
