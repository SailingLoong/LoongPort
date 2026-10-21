import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
const { importKey } = vi.hoisted(() => ({ importKey: vi.fn() }));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { importOpenCodeKey: importKey },
}));
import { OpenCodePersonalKeyForm } from "../OpenCodePersonalKeyForm";

describe("OpenCode personal key entry", () => {
  beforeEach(() => vi.clearAllMocks());
  it("requires explicit import and does not request a key or infer account details", () => {
    render(<OpenCodePersonalKeyForm onSaved={vi.fn()} />);
    expect(importKey).not.toHaveBeenCalled();
    expect(screen.getByLabelText("OpenCode Personal API key")).toHaveAttribute(
      "type",
      "password",
    );
    expect(
      screen.getByRole("button", { name: "同意加密保存并添加 OpenCode" }),
    ).toBeDisabled();
    expect(
      screen.getByText(/余额、workspace 身份和调用权限尚未验证/),
    ).toBeInTheDocument();
  });
  it("imports only on consent, clears the secret and distinguishes saved from tested", async () => {
    importKey.mockResolvedValue({ rowId: 8 });
    const saved = vi.fn();
    render(<OpenCodePersonalKeyForm onSaved={saved} />);
    const key = screen.getByLabelText("OpenCode Personal API key");
    fireEvent.change(key, {
      target: { value: "oc_sk_synthetic_SyntheticOnly123456789" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "同意加密保存并添加 OpenCode" }),
    );
    await waitFor(() => expect(saved).toHaveBeenCalledWith(8));
    expect(key).toHaveValue("");
    expect(
      screen.getByText(/已保存接入配置，尚未验证真实调用/),
    ).toBeInTheDocument();
  });
  it("prevents repeated submit, does not expose backend error payload, and permits retry", async () => {
    let reject!: (error: Error) => void;
    importKey.mockReturnValueOnce(
      new Promise((_, no) => {
        reject = no;
      }),
    );
    importKey.mockResolvedValueOnce({ rowId: 9 });
    render(<OpenCodePersonalKeyForm onSaved={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("OpenCode Personal API key"), {
      target: { value: "oc_sk_synthetic_SyntheticOnly123456789" },
    });
    const button = screen.getByRole("button", {
      name: "同意加密保存并添加 OpenCode",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(importKey).toHaveBeenCalledTimes(1);
    reject(new Error("SYNTHETIC_SECRET_MUST_NOT_APPEAR"));
    await waitFor(() => expect(button).not.toBeDisabled());
    expect(
      screen.queryByText(/SYNTHETIC_SECRET_MUST_NOT_APPEAR/),
    ).not.toBeInTheDocument();
    fireEvent.click(button);
    await waitFor(() => expect(importKey).toHaveBeenCalledTimes(2));
  });
});
