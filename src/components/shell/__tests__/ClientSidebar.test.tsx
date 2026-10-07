import { useEffect, useState } from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ClientSidebar } from "../ClientSidebar";

const preferenceKey = "loongport-sidebar-collapsed";
const destinations = [
  ["applications", "providers"],
  ["services", "services"],
  ["image", "image"],
  ["records", "records"],
  ["usage", "usage"],
  ["resources", "resources"],
  ["plaza", "plaza"],
] as const;

function resize(width: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: width,
  });
  fireEvent(window, new Event("resize"));
}

function renderSidebar(
  props: Partial<Parameters<typeof ClientSidebar>[0]> = {},
) {
  const onNavigate = vi.fn();
  const onHelp = vi.fn();
  const rendered = render(
    <div data-client-shell>
      <ClientSidebar
        view="providers"
        top={28}
        disabled={false}
        onNavigate={onNavigate}
        onHelp={onHelp}
        {...props}
      />
      <main id="main-content" tabIndex={-1}>
        Main content
      </main>
    </div>,
  );
  return { ...rendered, onNavigate, onHelp };
}

describe("approved seven-destination shell", () => {
  beforeEach(() => {
    localStorage.removeItem(preferenceKey);
    resize(1200);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    localStorage.removeItem(preferenceKey);
  });

  it("collapses without navigating and preserves all destination names", async () => {
    const user = userEvent.setup();
    const { onNavigate } = renderSidebar();
    const navigation = screen.getByRole("navigation", {
      name: "client.navigation",
    });
    expect(within(navigation).getAllByRole("button")).toHaveLength(7);
    const toggle = screen.getByRole("button", {
      name: "client.collapseSidebar",
    });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await user.click(toggle);
    expect(onNavigate).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "client.expandSidebar" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    for (const [key, route] of destinations) {
      const button = within(navigation).getByRole("button", {
        name: `client.${key}`,
      });
      await user.click(button);
      expect(onNavigate).toHaveBeenLastCalledWith(route);
    }
    expect(
      within(navigation).getByRole("button", { name: "client.applications" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("uses one shell width for the sidebar and fixed content offsets", async () => {
    const user = userEvent.setup();
    const { container } = renderSidebar();
    const shell = container.querySelector<HTMLElement>("[data-client-shell]")!;
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe("200px");
    await user.click(
      screen.getByRole("button", { name: "client.collapseSidebar" }),
    );
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe("72px");
    await user.click(
      screen.getByRole("button", { name: "client.expandSidebar" }),
    );
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe("200px");
  });

  it("automatically uses the rail below 960 and allows explicit keyboard expansion", () => {
    resize(959);
    const { onNavigate } = renderSidebar();
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    fireEvent.keyDown(window, { key: "\\", ctrlKey: true });
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    expect(onNavigate).not.toHaveBeenCalled();
    expect(localStorage.getItem(preferenceKey)).toBeNull();
  });

  it("follows resize until a manual preference is set", () => {
    renderSidebar();
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    resize(959);
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    resize(960);
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
  });

  it("starts a new narrow visit collapsed and restores the saved wide preference", () => {
    localStorage.setItem(preferenceKey, "false");
    renderSidebar();
    resize(959);
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    fireEvent.keyDown(window, { key: "\\", ctrlKey: true });
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    resize(1200);
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    resize(959);
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    expect(localStorage.getItem(preferenceKey)).toBe("false");
  });

  it("ignores repeated, composing and already consumed keyboard shortcuts", () => {
    renderSidebar();
    fireEvent.keyDown(window, { key: "\\", metaKey: true, repeat: true });
    fireEvent.keyDown(window, { key: "\\", metaKey: true, isComposing: true });
    const consumed = new KeyboardEvent("keydown", {
      key: "\\",
      ctrlKey: true,
      cancelable: true,
    });
    consumed.preventDefault();
    act(() => window.dispatchEvent(consumed));
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    fireEvent.keyDown(window, { key: "\\", metaKey: true });
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
  });

  it.each([{ altKey: true }, { shiftKey: true }])(
    "does not intercept modified backslash input in an editor: %j",
    (modifier) => {
      renderSidebar();
      render(<input aria-label="Configuration path" />);
      const input = screen.getByRole("textbox", { name: "Configuration path" });
      input.focus();
      const typing = new KeyboardEvent("keydown", {
        key: "\\",
        ctrlKey: true,
        ...modifier,
        bubbles: true,
        cancelable: true,
      });
      act(() => input.dispatchEvent(typing));
      expect(typing.defaultPrevented).toBe(false);
      expect(screen.getByRole("complementary")).toHaveStyle({ width: "200px" });
    },
  );

  it("keeps the skip link outside the sidebar stacking context", () => {
    renderSidebar();
    const skip = screen.getByRole("link", { name: "client.skipToContent" });
    expect(screen.getByRole("complementary")).not.toContainElement(skip);
    expect(skip).toHaveStyle({ zIndex: "55" });
  });

  it("portals the collapse tooltip outside the sidebar stacking context", async () => {
    renderSidebar();
    fireEvent.focus(
      screen.getByRole("button", { name: "client.collapseSidebar" }),
    );
    const tooltip = await screen.findByRole("tooltip");
    expect(screen.getByRole("complementary")).not.toContainElement(tooltip);
    expect(tooltip).toHaveStyle({ zIndex: "55" });
  });

  it("keeps existing footer state mounted across repeated toggles", async () => {
    const user = userEvent.setup();
    const mounted = vi.fn();
    function Footer() {
      const [count, setCount] = useState(0);
      useEffect(() => {
        mounted();
      }, []);
      return (
        <button onClick={() => setCount(count + 1)}>footer-{count}</button>
      );
    }
    renderSidebar({ footer: <Footer /> });
    await user.click(screen.getByRole("button", { name: "footer-0" }));
    await user.click(
      screen.getByRole("button", { name: "client.collapseSidebar" }),
    );
    await user.click(
      screen.getByRole("button", { name: "client.expandSidebar" }),
    );
    expect(
      screen.getByRole("button", { name: "footer-1" }),
    ).toBeInTheDocument();
    expect(mounted).toHaveBeenCalledTimes(1);
  });

  it("retains disabled navigation while collapse and help remain usable", async () => {
    const user = userEvent.setup();
    const { onNavigate, onHelp } = renderSidebar({ disabled: true });
    await user.click(
      screen.getByRole("button", { name: "client.collapseSidebar" }),
    );
    for (const [key] of destinations) {
      expect(
        screen.getByRole("button", { name: `client.${key}` }),
      ).toBeDisabled();
    }
    await user.click(screen.getByRole("button", { name: "client.help" }));
    expect(onHelp).toHaveBeenCalledTimes(1);
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it("provides a skip link and preserves the native titlebar exclusion", () => {
    renderSidebar();
    expect(
      screen.getByRole("link", { name: "client.skipToContent" }),
    ).toHaveAttribute("href", "#main-content");
    expect(screen.getByRole("complementary")).toHaveStyle({ top: "28px" });
  });

  it("remains usable when persistence is unavailable and releases its keyboard listener", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("Storage unavailable");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("Storage unavailable");
    });
    const { unmount } = renderSidebar();
    fireEvent.keyDown(window, { key: "\\", ctrlKey: true });
    expect(screen.getByRole("complementary")).toHaveStyle({ width: "72px" });
    unmount();
    const afterUnmount = new KeyboardEvent("keydown", {
      key: "\\",
      ctrlKey: true,
      cancelable: true,
    });
    window.dispatchEvent(afterUnmount);
    expect(afterUnmount.defaultPrevented).toBe(false);
  });
});
