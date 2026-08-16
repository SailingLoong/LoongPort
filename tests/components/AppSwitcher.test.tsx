import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppSwitcher } from "@/components/AppSwitcher";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import type { VisibleApps } from "@/types";

const allVisible = DEFAULT_VISIBLE_APPS;

/** 测试环境 i18n 资源为空，t() 回落成键名本身；× 的 title 即 "appSwitcher.hide" */
const hideButtonSelector = "button[title='appSwitcher.hide']";

describe("AppSwitcher", () => {
  it("所有可见应用直接平铺，不再有「更多」折叠入口", () => {
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={allVisible}
      />,
    );
    // 顺序上的第一个与最后一个可见应用都在
    expect(
      screen.getByRole("button", { name: "Claude Code" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pi" })).toBeInTheDocument();
    expect(screen.queryByTitle("appSwitcher.more")).not.toBeInTheDocument();
  });

  it("点 × 隐藏对应应用，不触发切换", () => {
    const onSwitch = vi.fn();
    const onHideApp = vi.fn();
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={onSwitch}
        visibleApps={allVisible}
        onHideApp={onHideApp}
      />,
    );
    const piTab = screen.getByRole("button", { name: "Pi" });
    const hidePi = piTab.parentElement!.querySelector(hideButtonSelector);
    expect(hidePi, "Pi 的 tab 上应有 ×").not.toBeNull();
    fireEvent.click(hidePi!);
    expect(onHideApp).toHaveBeenCalledTimes(1);
    expect(onHideApp).toHaveBeenCalledWith("pi");
    expect(onSwitch).not.toHaveBeenCalled();
  });

  it("只剩一个可见应用时不再出 ×（与设置页同一护栏）", () => {
    const onlyClaude: VisibleApps = {
      claude: true,
      "claude-desktop": false,
      codex: false,
      "codex-image": false,
      gemini: false,
      grokbuild: false,
      opencode: false,
      openclaw: false,
      hermes: false,
      pi: false,
    };
    const { container } = render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={onlyClaude}
        onHideApp={vi.fn()}
      />,
    );
    expect(container.querySelector(hideButtonSelector)).toBeNull();
    expect(
      screen.getByRole("button", { name: "Claude Code" }),
    ).toBeInTheDocument();
  });

  it("未传 onHideApp 时不渲染 ×", () => {
    const { container } = render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={allVisible}
      />,
    );
    expect(container.querySelector(hideButtonSelector)).toBeNull();
  });
});
