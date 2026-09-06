import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AddHubPage } from "../AddHubPage";

/**
 * 广场开关在聚合页的表现：false = 广场 tab 整个不出现、默认落「手动添加」。
 * 默认值由后端按首启归因播种（站长引流来的用户默认关），这里是它的消费端。
 */
const settingsQueryMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/query", () => ({
  useSettingsQuery: () => settingsQueryMock(),
}));
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

describe("AddHubPage 广场开关", () => {
  it("未播种（默认）时广场 tab 存在，添加站点默认落广场", () => {
    settingsQueryMock.mockReturnValue({ data: { plazaVisible: null } });
    renderHub();
    expect(screen.getByTestId("relay-directory")).toBeInTheDocument();
  });

  it("开关关闭时广场 tab 不渲染、默认落「手动添加」", () => {
    settingsQueryMock.mockReturnValue({ data: { plazaVisible: false } });
    renderHub();
    expect(screen.queryByTestId("relay-directory")).not.toBeInTheDocument();
    expect(screen.getByTestId("add-provider-form")).toBeInTheDocument();
  });

  it("设置晚到（先渲染、后变 false）也能把选中态拨回手动添加", () => {
    settingsQueryMock.mockReturnValue({ data: undefined });
    const view = renderHub();
    expect(screen.getByTestId("relay-directory")).toBeInTheDocument();

    settingsQueryMock.mockReturnValue({ data: { plazaVisible: false } });
    view.rerender(
      <AddHubPage
        sourceAppId="codex"
        onBack={() => undefined}
        onAddProvider={() => undefined as never}
      />,
    );
    expect(screen.queryByTestId("relay-directory")).not.toBeInTheDocument();
    expect(screen.getByTestId("add-provider-form")).toBeInTheDocument();
  });
});
