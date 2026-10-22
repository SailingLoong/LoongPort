import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ClientsPanel } from "@/components/clients/ClientsPanel";
import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const call = vi.mocked(invoke);
const client = {
  descriptor: { id: "workbuddy", name: "WorkBuddy", routeApps: ["codex"] },
  connection: null,
  recoveryRequired: false,
  status: "notConfigured",
};
beforeEach(() => {
  i18n.addResourceBundle(
    "zh",
    "translation",
    { clients: zh.clients },
    true,
    true,
  );
  call.mockReset();
  call.mockImplementation(async (name) => {
    if (name === "list_client_connections") return [client];
    if (name === "list_client_credentials")
      return [
        {
          id: "credential-ref",
          clientId: "workbuddy",
          routeApp: "codex",
          allowedModels: ["model-a"],
          revoked: false,
        },
      ];
    if (name === "preview_client_connection")
      return { token: "preview-token", modelCount: 1, changed: true };
    return undefined;
  });
});
describe("Client connections", () => {
  it("requires a preview and sends only the reference to apply", async () => {
    const user = userEvent.setup();
    render(<ClientsPanel />);
    await screen.findByText("WorkBuddy");
    await user.type(screen.getByLabelText("模型 ID（逗号分隔）"), "model-a");
    await user.selectOptions(
      screen.getByLabelText("接入凭据"),
      "credential-ref",
    );
    expect(
      screen.queryByRole("button", { name: "应用配置" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "预览配置" }));
    await user.click(await screen.findByRole("button", { name: "应用配置" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("apply_client_connection", {
        request: {
          clientId: "workbuddy",
          credentialRef: "credential-ref",
          routeApp: "codex",
          models: ["model-a"],
        },
        token: "preview-token",
      }),
    );
    expect(await screen.findByRole("status")).toHaveTextContent("验证对话");
  });
  it("does not report success after a conflict and requires another preview", async () => {
    const defaultCall = call.getMockImplementation()!;
    call.mockImplementation(async (name, args) => {
      if (name === "apply_client_connection")
        throw new Error("clients.configuration_changed");
      return defaultCall(name, args);
    });
    const user = userEvent.setup();
    render(<ClientsPanel />);
    await screen.findByText("WorkBuddy");
    await user.type(screen.getByLabelText("模型 ID（逗号分隔）"), "model-a");
    await user.selectOptions(
      screen.getByLabelText("接入凭据"),
      "credential-ref",
    );
    await user.click(screen.getByRole("button", { name: "预览配置" }));
    await user.click(await screen.findByRole("button", { name: "应用配置" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("操作未完成");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "应用配置" }),
    ).not.toBeInTheDocument();
  });
});
