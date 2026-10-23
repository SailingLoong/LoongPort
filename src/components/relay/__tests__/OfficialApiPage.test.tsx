import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import type { AppId } from "@/lib/api";

const {
  openLogin,
  importOpenCodeKey,
  previewOpenCodeLocal,
  importOpenCodeLocal,
} = vi.hoisted(() => ({
  openLogin: vi.fn(),
  importOpenCodeKey: vi.fn(),
  previewOpenCodeLocal: vi.fn(),
  importOpenCodeLocal: vi.fn(),
}));

vi.mock("@/lib/api", () => ({}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    openLogin,
    importOpenCodeKey,
    previewOpenCodeLocal,
    importOpenCodeLocal,
  },
  DEEPSEEK_VENDOR_ID: "deepseek",
  BIGMODEL_VENDOR_ID: "bigmodel",
  VENDOR_CATALOG: [
    {
      id: "deepseek",
      displayName: "DeepSeek",
      descriptionKey: "loongport.officialApi.deepseekDesc",
    },
    {
      id: "opencode",
      displayName: "OpenCode",
      descriptionKey: "loongport.officialApi.opencodeDesc",
    },
    {
      id: "bigmodel",
      displayName: "智谱 BigModel",
      descriptionKey: "loongport.officialApi.bigmodelDesc",
    },
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
  return {
    onBack,
    view: render(<OfficialApiPage sourceAppId={app} onBack={onBack} />),
  };
}

describe("OfficialApiPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders every catalog vendor as a connectable card", () => {
    renderPage();

    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("智谱 BigModel")).toBeInTheDocument();
    expect(screen.getAllByText("loongport.officialApi.connect").length).toBe(2);
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
  it("ends OpenCode handoff errors without success and permits retry/cancel", async () => {
    const { toast } = await import("sonner");
    openLogin.mockRejectedValueOnce(
      new Error("新版 Console 尚未接通，请使用已有完整 API key"),
    );
    openLogin.mockResolvedValueOnce(null);
    const { onBack } = renderPage();
    fireEvent.click(screen.getByText("OpenCode"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getAllByText("loongport.officialApi.connect")).toHaveLength(
        2,
      ),
    );
    expect(toast.success).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("OpenCode"));
    await waitFor(() => expect(openLogin).toHaveBeenCalledTimes(2));
    expect(toast.success).not.toHaveBeenCalled();
  });
  it("offers existing key forms after OpenCode failure without reading or saving credentials", async () => {
    const { toast } = await import("sonner");
    openLogin.mockRejectedValueOnce(new Error("新版 Console 未取得可用密钥"));
    const onConnected = vi.fn();
    const onBack = vi.fn();
    render(
      <OfficialApiPage
        sourceAppId="codex"
        vendorId="opencode"
        onBack={onBack}
        onConnected={onConnected}
      />,
    );
    const login = screen.getByRole("button", { name: /官网账户登录/ });
    const keyInput = screen.getByLabelText("OpenCode Personal API key");
    fireEvent.change(keyInput, {
      target: { value: "oc_sk_synthetic_unsubmitted" },
    });
    fireEvent.click(login);
    const recovery = await screen.findByRole("button", {
      name: "使用已有 Personal key 或本机导入",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("尚未验证真实调用");
    expect(login).toBeEnabled();
    fireEvent.click(recovery);
    const methods = screen.getByRole("region", {
      name: "OpenCode API key 接入方式",
    });
    expect(methods).toHaveFocus();
    expect(methods).toContainElement(keyInput);
    expect(methods).toContainElement(
      screen.getByRole("button", { name: "同意只读预览本机 OpenCode 凭据" }),
    );
    expect(keyInput).toHaveValue("oc_sk_synthetic_unsubmitted");
    expect(
      screen.queryByText("oc_sk_synthetic_unsubmitted"),
    ).not.toBeInTheDocument();
    expect(importOpenCodeKey).not.toHaveBeenCalled();
    expect(previewOpenCodeLocal).not.toHaveBeenCalled();
    expect(importOpenCodeLocal).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(onConnected).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();

    openLogin.mockResolvedValueOnce(null);
    fireEvent.click(login);
    await waitFor(() => expect(openLogin).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(onConnected).not.toHaveBeenCalled();
  });

  it("does not offer OpenCode recovery for other vendors or a cancelled login", async () => {
    const { toast } = await import("sonner");
    openLogin.mockRejectedValueOnce(new Error("登录未完成"));
    renderPage();
    fireEvent.click(screen.getByText("DeepSeek"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(
      screen.queryByRole("button", {
        name: "使用已有 Personal key 或本机导入",
      }),
    ).not.toBeInTheDocument();
    openLogin.mockResolvedValueOnce(null);
    fireEvent.click(screen.getByText("OpenCode"));
    await waitFor(() => expect(openLogin).toHaveBeenCalledTimes(2));
    expect(
      screen.queryByRole("button", {
        name: "使用已有 Personal key 或本机导入",
      }),
    ).not.toBeInTheDocument();
  });
});
