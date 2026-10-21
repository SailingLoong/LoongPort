import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState, type ReactElement } from "react";
import {
  fireEvent,
  render as renderView,
  screen,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AddHubPage } from "../AddHubPage";

vi.mock("@/lib/api/providers", () => ({
  providersApi: { getAll: vi.fn(async () => ({})) },
}));
function render(view: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderView(
    <QueryClientProvider client={client}>{view}</QueryClientProvider>,
  );
}

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
  OfficialApiPage: ({ onConnected }: any) => (
    <button
      data-testid="official-api"
      onClick={() => onConnected({ kind: "vendor", rowId: 1 })}
    >
      Connect test
    </button>
  ),
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: () => <div data-testid="add-provider-form" />,
}));

vi.mock("../onboarding/ServiceConfiguration", () => ({
  ServiceConfiguration: ({ onBack }: any) => {
    const [draft, setDraft] = useState("");
    return (
      <div>
        <input
          aria-label="Configuration draft"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
        />
        <button onClick={onBack}>Back to hub</button>
      </div>
    );
  },
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

it("distinguishes native platform editions rather than routing to API configuration", () => {
  const platform = vi.fn();
  render(
    <AddHubPage
      sourceAppId="claude"
      onBack={() => {}}
      onAddProvider={() => {}}
      onOpenPlatform={platform}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "MiniMax Code 国际版" }));
  expect(platform).toHaveBeenLastCalledWith("minimax_code_global");
  fireEvent.click(screen.getByRole("button", { name: "TRAE 中国版" }));
  expect(platform).toHaveBeenLastCalledWith("trae_cn");
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

it("preserves a connected service draft while visiting another source", () => {
  render(
    <AddHubPage
      sourceAppId="codex"
      initialTab="official"
      onBack={() => {}}
      onAddProvider={() => {}}
    />,
  );
  fireEvent.click(screen.getByText("Connect test"));
  fireEvent.change(screen.getByLabelText("Configuration draft"), {
    target: { value: "keep me" },
  });
  fireEvent.click(screen.getByText("Back to hub"));
  fireEvent.mouseDown(screen.getByRole("tab", { name: "账号" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(screen.getByRole("button", { name: "Qoder" }));
  fireEvent.click(screen.getByText("返回平台列表"));
  fireEvent.click(screen.getByText("loongport.onboarding.resume"));
  expect(screen.getByLabelText("Configuration draft")).toHaveValue("keep me");
});
