import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { DatabaseUpgrade } from "@/components/DatabaseUpgrade";
import { server } from "../msw/server";

vi.mock("@tauri-apps/plugin-process", () => ({ exit: vi.fn() }));

const payload = {
  path: "/home/mock/.loongport/loongport.db",
  error:
    "LoongPort 数据版本过新（24），当前应用仅支持 23，请升级应用后再尝试。",
  kind: "db_version_too_new",
  db_version: 24,
  supported_version: 23,
};

// 无可用更新 → 界面进入 incompatible 态（逃生门在所有非 checking/updating 态可见）。
function noUpdateAvailable() {
  server.use(
    http.post("http://tauri.local/check_app_update_available", () =>
      HttpResponse.json(null),
    ),
  );
}

function renderScreen() {
  return render(<DatabaseUpgrade payload={payload} />);
}

describe("DatabaseUpgrade fresh start escape hatch", () => {
  it("reveals a two-step confirm and cancels without invoking anything", async () => {
    noUpdateAvailable();
    renderScreen();
    await userEvent.click(
      await screen.findByRole("button", { name: "保留备份，重新开始" }),
    );
    expect(screen.getByText("留在此版本，重新开始？")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(
      screen.queryByText("留在此版本，重新开始？"),
    ).not.toBeInTheDocument();
  });

  it("invokes the quarantine command only after explicit confirmation", async () => {
    noUpdateAvailable();
    let quarantineCalls = 0;
    server.use(
      http.post("http://tauri.local/quarantine_app_data_and_restart", () => {
        quarantineCalls += 1;
        // 成功路径上后端直接重启进程，前端不会收到返回；挂起的 Promise 等价。
        return new Promise<never>(() => {});
      }),
    );
    renderScreen();
    await userEvent.click(
      await screen.findByRole("button", { name: "保留备份，重新开始" }),
    );
    expect(quarantineCalls).toBe(0);
    await userEvent.click(
      screen.getByRole("button", { name: "保留备份并重启" }),
    );
    await waitFor(() => expect(quarantineCalls).toBe(1));
  });

  it("surfaces a failed quarantine as an in-screen error and stays retryable", async () => {
    noUpdateAvailable();
    server.use(
      http.post("http://tauri.local/quarantine_app_data_and_restart", () =>
        HttpResponse.text("quarantine failed: permission denied", {
          status: 500,
        }),
      ),
    );
    renderScreen();
    await userEvent.click(
      await screen.findByRole("button", { name: "保留备份，重新开始" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "保留备份并重启" }),
    );
    expect(await screen.findByText(/quarantine failed/)).toBeInTheDocument();
    expect(
      screen.queryByText("留在此版本，重新开始？"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "保留备份，重新开始" }),
    ).toBeEnabled();
  });
});
