import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { OfficialApiPage } from "@/components/relay/OfficialApiPage";
import { vendorApi } from "@/lib/api/vendor";
vi.mock("@/lib/api/vendor", async (original) => ({
  ...(await original<typeof import("@/lib/api/vendor")>()),
  vendorApi: { openLogin: vi.fn() },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
beforeEach(() => vi.clearAllMocks());
describe("OfficialApiPage", () => {
  it.each([
    ["DeepSeek", "deepseek"],
    ["智谱 BigModel", "bigmodel"],
    ["opencode", "opencode"],
  ])(
    "connects %s through real vendor API contract and advances to configuration",
    async (name, id) => {
      vi.mocked(vendorApi.openLogin).mockResolvedValue({
        rowId: 8,
        refresh: {} as never,
      });
      const onConnected = vi.fn();
      const onBack = vi.fn();
      const user = userEvent.setup();
      render(
        <OfficialApiPage
          sourceAppId="gemini"
          onConnected={onConnected}
          onBack={onBack}
        />,
      );
      await user.click(screen.getByRole("button", { name: new RegExp(name) }));
      expect(vendorApi.openLogin).toHaveBeenCalledWith(id, "gemini");
      expect(onConnected).toHaveBeenCalledWith({
        kind: "vendor",
        rowId: 8,
        name,
      });
      expect(onBack).not.toHaveBeenCalled();
    },
  );
  it("keeps a cancelled login on the official selection page", async () => {
    vi.mocked(vendorApi.openLogin).mockResolvedValue(null);
    const onConnected = vi.fn();
    const user = userEvent.setup();
    render(
      <OfficialApiPage
        sourceAppId="codex"
        onConnected={onConnected}
        onBack={vi.fn()}
      />,
    );
    await user.click(screen.getByRole("button", { name: /DeepSeek/ }));
    expect(onConnected).not.toHaveBeenCalled();
  });
});
