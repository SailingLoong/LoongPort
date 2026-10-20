import { render, screen, waitFor } from "@testing-library/react";
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
