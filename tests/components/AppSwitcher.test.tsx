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

  it("「+」常驻列表末尾；全部可见时点开是空态", () => {
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={allVisible}
        onShowApp={vi.fn()}
      />,
    );
    const addButton = screen.getByTitle("appSwitcher.add");
    expect(addButton).toBeInTheDocument();
    fireEvent.click(addButton);
    expect(screen.getByText("appSwitcher.allShown")).toBeInTheDocument();
  });

  it("「+」浮层列出隐藏应用，点击加回", () => {
    const onShowApp = vi.fn();
    const partlyHidden: VisibleApps = {
      ...allVisible,
      pi: false,
      hermes: false,
    };
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={partlyHidden}
        onShowApp={onShowApp}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "Pi" }),
      "隐藏的 Pi 不应出现在 tab 里",
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByTitle("appSwitcher.add"));
    // 两个隐藏应用都列在浮层里。用正则匹配：ProviderIcon 给部分品牌图标带
    // alt 文本，条目按钮的可访问名是「alt + span」拼接，不是裸应用名。
    // （浮层常开可多选是 Radix Popover 的固有行为，不在 jsdom 里复测库本身）
    expect(screen.getByRole("button", { name: /Pi/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Hermes/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Hermes/ }));
    expect(onShowApp).toHaveBeenCalledTimes(1);
    expect(onShowApp).toHaveBeenCalledWith("hermes");
  });

  it("未传 onShowApp 时不渲染「+」", () => {
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={allVisible}
      />,
    );
    expect(screen.queryByTitle("appSwitcher.add")).not.toBeInTheDocument();
  });
});
