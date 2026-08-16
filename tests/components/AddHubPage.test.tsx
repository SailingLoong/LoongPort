import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AddHubPage } from "@/components/relay/AddHubPage";
import { createTestQueryClient } from "../utils/testQueryClient";

const listVendorAccounts = vi.fn();

vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { list: (...args: unknown[]) => listVendorAccounts(...args) },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: (props: any) => (
    <div data-testid="directory-content">directory {props.sourceAppId}</div>
  ),
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: (props: any) => (
    <div data-testid="official-content">official {props.sourceAppId}</div>
  ),
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: (props: any) => (
    <div data-testid="manual-content">manual {props.appId}</div>
  ),
}));

function renderHub(appId: "codex" | "gemini") {
  const onBack = vi.fn();
  const onAddProvider = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage
        sourceAppId={appId}
        onBack={onBack}
        onAddProvider={onAddProvider}
      />
    </QueryClientProvider>,
  );
  return { onBack, onAddProvider };
}

describe("AddHubPage", () => {
  beforeEach(() => {
    listVendorAccounts.mockReset();
  });

  it("lands on the relay directory tab and switches in place", async () => {
    const user = userEvent.setup();
    listVendorAccounts.mockResolvedValue({ supported: true, accounts: [] });
    renderHub("codex");

    // 默认展示中转站广场 —— 点一次「+」进来不用再选。
    expect(screen.getByTestId("directory-content")).toBeInTheDocument();

    // supported 是异步查询，等官方 API 标签出现再切。
    const officialTab = await screen.findByRole("tab", {
      name: "loongport.sections.official",
    });
    await user.click(officialTab);
    expect(screen.getByTestId("official-content")).toBeInTheDocument();

    await user.click(
      screen.getByRole("tab", { name: "loongport.addEntry.manual" }),
    );
    expect(screen.getByTestId("manual-content")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "common.back" }));
    expect(screen.queryByTestId("directory-content")).toBeInTheDocument;
  });

  it("hides the official tab when the backend says the app is unsupported", async () => {
    listVendorAccounts.mockResolvedValue({ supported: false, accounts: [] });
    renderHub("gemini");

    expect(
      screen.queryByRole("tab", { name: "loongport.sections.official" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: "loongport.sections.relay" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: "loongport.addEntry.manual" }),
    ).toBeInTheDocument();
  });

  it("invokes onBack from the back button", async () => {
    const user = userEvent.setup();
    listVendorAccounts.mockResolvedValue({ supported: true, accounts: [] });
    const { onBack } = renderHub("codex");

    await user.click(screen.getByRole("button", { name: "common.back" }));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});
