import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import i18n from "i18next";
import { beforeEach, expect, it, vi } from "vitest";
import { AccountPlatformTabs } from "@/components/relay/accounts/AccountPlatformTabs";
import type { AccountPlatform } from "@/components/relay/accounts/accountPlatforms";
beforeEach(() => localStorage.clear());
function Harness() {
  const [active, setActive] = useState<AccountPlatform>();
  return (
    <AccountPlatformTabs
      active={active}
      onSelect={setActive}
      platforms={["workbuddy", "trae_cn", "trae"]}
    />
  );
}
it("defaults to all accounts and only offers imported platform filters", () => {
  render(<Harness />);
  expect(screen.getByRole("tab", { name: "全部账号" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.queryByRole("tab", { name: "Qoder" })).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: /隐藏|添加平台/ }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "TRAE 中国版" })).toHaveTextContent(
    "CN",
  );
  expect(screen.getByRole("tab", { name: "TRAE 国际版" })).toHaveTextContent(
    "INTL",
  );
});
it("selects one platform and restores all without changing legacy preferences", async () => {
  const user = userEvent.setup();
  localStorage.setItem("loongport.account-platform-tabs.v1", '["qoder"]');
  render(<Harness />);
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  expect(screen.getByRole("tab", { name: "WorkBuddy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await user.click(screen.getByRole("tab", { name: "全部账号" }));
  expect(screen.getByRole("tab", { name: "全部账号" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(localStorage.getItem("loongport.account-platform-tabs.v1")).toBe(
    '["qoder"]',
  );
});
it("supports keyboard filtering and cannot delete a filter", async () => {
  const user = userEvent.setup();
  render(<Harness />);
  act(() => screen.getByRole("tab", { name: "全部账号" }).focus());
  await user.keyboard("{ArrowRight}");
  await waitFor(() =>
    expect(screen.getByRole("tab", { name: "WorkBuddy" })).toHaveFocus(),
  );
  await user.keyboard("{Delete}");
  expect(screen.getByRole("tab", { name: "WorkBuddy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await user.keyboard("{Home}");
  expect(screen.getByRole("tab", { name: "全部账号" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});
it("keeps all reachable with zero accounts and deduplicates platform projections", () => {
  const { rerender } = render(
    <AccountPlatformTabs platforms={[]} onSelect={() => {}} />,
  );
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  rerender(
    <AccountPlatformTabs
      platforms={["trae_cn", "trae_cn"]}
      onSelect={() => {}}
    />,
  );
  expect(screen.getAllByRole("tab")).toHaveLength(2);
});
it("retains compact real logos and full edition tooltips", async () => {
  const user = userEvent.setup();
  render(<Harness />);
  const cn = screen.getByRole("tab", { name: "TRAE 中国版" });
  expect(cn).toHaveClass("h-10", "w-10");
  expect(cn.querySelector("img")).toHaveClass("h-5", "w-5");
  await user.hover(cn);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("TRAE 中国版");
});

it("translates both the all-filter label and its visible text", async () => {
  i18n.addResourceBundle(
    "en",
    "translation",
    { loongport: { accounts: { allPlatforms: "All" } } },
    true,
    true,
  );
  await i18n.changeLanguage("en");
  render(<Harness />);
  expect(screen.getByRole("tab", { name: "All" })).toHaveTextContent(/^All$/);
  await i18n.changeLanguage("zh");
});

it("scrolls the keyboard-selected filter into view", async () => {
  const scroll = vi.fn();
  const original = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = scroll;
  try {
    render(<Harness />);
    screen.getByRole("tab", { name: "全部账号" }).focus();
    await userEvent.setup().keyboard("{End}");
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect(screen.getByRole("tab", { name: "TRAE 国际版" })).toHaveFocus();
  } finally {
    HTMLElement.prototype.scrollIntoView = original;
  }
});
