import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AddHubPage } from "../AddHubPage";

/**
 * 广场开关的消费端在 `RelayDirectoryPage`（只藏推荐列表）。
 * 聚合页这里守的是另一件事：「中转站」tab 常驻 —— 摘 tab 会把搜索框直连
 * 这条添加站点的路一起关掉，那是 2026-09-08 修正掉的旧语义。
 */
vi.mock("@/lib/query/vendor", () => ({
  useVendorSupportedQuery: () => ({ data: false }),
}));
vi.mock("../directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: () => <div data-testid="relay-directory" />,
}));
vi.mock("../OfficialApiPage", () => ({
  OfficialApiPage: () => <div data-testid="official-api" />,
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: () => <div data-testid="add-provider-form" />,
}));

function renderHub() {
  return render(
    <AddHubPage
      sourceAppId="codex"
      onBack={() => undefined}
      onAddProvider={() => undefined as never}
    />,
  );
}

describe("AddHubPage 中转站 tab 常驻", () => {
  it("默认展示账号平台，指定中转站仍能直达", () => {
    renderHub();
    expect(
      screen.getByRole("button", { name: "GitHub Copilot" }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("relay-directory")).not.toBeInTheDocument();
  });
});

it("keeps existing API configuration owners and their actual application context", () => {
  const open = vi.fn();
  const platform = vi.fn();
  render(
    <AddHubPage
      sourceAppId="claude"
      onBack={() => {}}
      onAddProvider={() => undefined as never}
      onOpenConfiguration={open}
      onOpenPlatform={platform}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Qoder" }));
  expect(open).toHaveBeenLastCalledWith("codex", "manual");
  fireEvent.click(screen.getByRole("button", { name: "OpenRouter · API Key" }));
  expect(open).toHaveBeenLastCalledWith("claude", "manual");
  fireEvent.click(screen.getByRole("button", { name: "OpenCode" }));
  expect(open).toHaveBeenLastCalledWith("claude", "official");
  expect(platform).not.toHaveBeenCalled();
});
it("preserves the original direct relay destination", () => {
  render(
    <AddHubPage
      sourceAppId="codex"
      initialTab="directory"
      onBack={() => {}}
      onAddProvider={() => undefined as never}
    />,
  );
  expect(screen.getByTestId("relay-directory")).toBeInTheDocument();
});
