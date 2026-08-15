import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import type { AppId } from "@/lib/api";

const { openLogin } = vi.hoisted(() => ({ openLogin: vi.fn() }));

vi.mock("@/lib/api", () => ({}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { openLogin },
  DEEPSEEK_VENDOR_ID: "deepseek",
  BIGMODEL_VENDOR_ID: "bigmodel",
  VENDOR_CATALOG: [
    { id: "deepseek", displayName: "DeepSeek", descriptionKey: "loongport.officialApi.deepseekDesc" },
    { id: "bigmodel", displayName: "智谱 BigModel", descriptionKey: "loongport.officialApi.bigmodelDesc" },
  ],
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

const { OfficialApiPage } = await import("../OfficialApiPage");

function renderPage(app: AppId = "claude", onBack = vi.fn()) {
  return { onBack, view: render(<OfficialApiPage sourceAppId={app} onBack={onBack} />) };
}

describe("OfficialApiPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders every catalog vendor as a connectable card", () => {
    renderPage();

    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("智谱 BigModel")).toBeInTheDocument();
    expect(
      screen.getAllByText("loongport.officialApi.connect").length,
    ).toBe(2);
  });

  it("picking a vendor opens login with that vendor id and the source app", async () => {
    openLogin.mockResolvedValue(null); // 用户关窗：不是错误、也不返回
    const { onBack } = renderPage("codex");

    fireEvent.click(screen.getByText("智谱 BigModel"));

    await waitFor(() =>
      expect(openLogin).toHaveBeenCalledWith("bigmodel", "codex"),
    );
    expect(onBack).not.toHaveBeenCalled();
  });

  it("returns to the caller after a successful login", async () => {
    openLogin.mockResolvedValue({
      rowId: 7,
      refresh: { summary: null, balances: [] },
    });
    const { onBack } = renderPage();

    fireEvent.click(screen.getByText("DeepSeek"));

    await waitFor(() => expect(onBack).toHaveBeenCalled());
  });

  it("reports login failures instead of silently returning", async () => {
    const { toast } = await import("sonner");
    openLogin.mockRejectedValue(new Error("登录窗开不出来"));
    const { onBack } = renderPage();

    fireEvent.click(screen.getByText("DeepSeek"));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(onBack).not.toHaveBeenCalled();
  });

  it("serializes logins: the other card is disabled while one is pending", async () => {
    let finish!: (value: null) => void;
    openLogin.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    renderPage();

    fireEvent.click(screen.getByText("DeepSeek"));
    await waitFor(() => expect(openLogin).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByText("智谱 BigModel"));
    expect(openLogin).toHaveBeenCalledTimes(1);

    finish(null);
  });
});
