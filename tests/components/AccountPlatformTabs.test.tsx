import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, expect, it } from "vitest";
import { AccountPlatformTabs } from "@/components/relay/accounts/AccountPlatformTabs";
import type { AccountPlatform } from "@/components/relay/accounts/accountPlatforms";
beforeEach(() => localStorage.clear());
function Harness() {
  const [active, setActive] = useState<AccountPlatform | undefined>(
    "workbuddy",
  );
  return <AccountPlatformTabs active={active} onSelect={setActive} />;
}
it("renders distinct edition tabs with official logos and a persistent add button", () => {
  render(<Harness />);
  expect(screen.getByRole("tab", { name: /TRAE 中国版/ })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: /TRAE 国际版/ })).toBeInTheDocument();
  expect(
    screen.getByRole("tab", { name: /MiniMax Code 国内版/ }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("tab", { name: /MiniMax Code 国际版/ }),
  ).toBeInTheDocument();
  expect(screen.getAllByRole("img").length).toBeGreaterThan(6);
  expect(
    screen.getByRole("button", { name: "添加平台标签" }),
  ).toBeInTheDocument();
});
it("hides only navigation, safely selects a neighbor and restores without duplicate tabs", async () => {
  const user = userEvent.setup();
  const view = render(<Harness />);
  await user.click(screen.getByRole("button", { name: "隐藏 WorkBuddy 标签" }));
  expect(
    screen.queryByRole("tab", { name: "WorkBuddy" }),
  ).not.toBeInTheDocument();
  view.unmount();
  render(<AccountPlatformTabs onSelect={() => {}} />);
  expect(
    screen.queryByRole("tab", { name: "WorkBuddy" }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "添加平台标签" }));
  await user.click(screen.getByRole("button", { name: "添加 WorkBuddy" }));
  expect(screen.getAllByRole("tab", { name: "WorkBuddy" })).toHaveLength(1);
});
it("supports keyboard selection, closing the final tab, and restoring from the empty state", async () => {
  const user = userEvent.setup();
  render(<Harness />);
  const first = screen.getAllByRole("tab")[0];
  first.focus();
  await user.keyboard("{ArrowRight}");
  await waitFor(() => expect(screen.getAllByRole("tab")[1]).toHaveFocus());
  for (const close of screen.getAllByRole("button", { name: /^隐藏 / }))
    await user.click(close);
  expect(screen.queryAllByRole("tab")).toHaveLength(0);
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "添加平台标签" })).toHaveFocus(),
  );
  await user.click(screen.getByRole("button", { name: "添加平台标签" }));
  await user.click(screen.getByRole("button", { name: "添加 WorkBuddy" }));
  expect(screen.getByRole("tab", { name: "WorkBuddy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

it("temporarily reveals an explicitly opened hidden platform without changing saved visibility", () => {
  localStorage.setItem(
    "loongport.account-platform-tabs.v1",
    JSON.stringify(["codex_oauth"]),
  );
  const view = render(
    <AccountPlatformTabs active="workbuddy" onSelect={() => {}} />,
  );
  expect(screen.getByRole("tab", { name: "WorkBuddy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(
    JSON.parse(localStorage.getItem("loongport.account-platform-tabs.v1")!),
  ).toEqual(["codex_oauth"]);
  view.rerender(
    <AccountPlatformTabs active="codex_oauth" onSelect={() => {}} />,
  );
  expect(
    screen.queryByRole("tab", { name: "WorkBuddy" }),
  ).not.toBeInTheDocument();
});

it("uses compact icon tabs with region badges only for true editions", () => {
  render(<Harness />);
  const workbuddy = screen.getByRole("tab", { name: "WorkBuddy" });
  expect(workbuddy.textContent).toBe("");
  expect(workbuddy).toHaveClass("h-10", "w-10");
  expect(workbuddy.querySelector("img")).toHaveClass("h-5", "w-5");
  expect(screen.getByRole("tab", { name: "TRAE 中国版" })).toHaveTextContent(
    /^CN$/,
  );
  expect(screen.getByRole("tab", { name: "TRAE 国际版" })).toHaveTextContent(
    /^INTL$/,
  );
  expect(
    screen.getByRole("tab", { name: "MiniMax Code 国内版" }),
  ).toHaveTextContent(/^CN$/);
  expect(
    screen.getByRole("tab", { name: "MiniMax Code 国际版" }),
  ).toHaveTextContent(/^INTL$/);
  expect(screen.getByRole("tab", { name: "Qoder" }).textContent).toBe("");
  expect(screen.getByRole("tablist")).toHaveClass("overflow-x-auto");
  expect(
    screen
      .getByRole("button", { name: "添加平台标签" })
      .closest('[role="tablist"]'),
  ).toBeNull();
});

it("shows the full edition name on hover and keyboard focus without expanding the tab", async () => {
  const user = userEvent.setup();
  render(<Harness />);
  const international = screen.getByRole("tab", { name: "TRAE 国际版" });
  await user.hover(international);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("TRAE 国际版");
  await user.unhover(international);
  const workbuddy = screen.getByRole("tab", { name: "WorkBuddy" });
  act(() => workbuddy.focus());
  await waitFor(() =>
    expect(screen.getByRole("tooltip")).toHaveTextContent("WorkBuddy"),
  );
  await user.keyboard("{Escape}");
  await waitFor(() =>
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument(),
  );
  expect(workbuddy).toHaveAttribute("aria-selected", "true");
});
