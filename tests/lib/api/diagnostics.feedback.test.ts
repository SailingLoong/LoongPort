import { beforeEach, describe, expect, it, vi } from "vitest";
import { feedbackApi } from "@/lib/api/diagnostics";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

beforeEach(() => invokeMock.mockReset());

describe("feedback local receipt API", () => {
  it("reads the native owner's latest receipt without reconstructing it", async () => {
    const result = {
      reportId: "synthetic-report",
      localCopy: { status: "missing" },
    };
    invokeMock.mockResolvedValue(result);
    await expect(feedbackApi.getLastReport()).resolves.toBe(result);
    expect(invokeMock).toHaveBeenCalledWith("feedback_get_last_report");
  });

  it("exports only the report selected by the user and preserves cancellation", async () => {
    invokeMock.mockResolvedValue(false);
    await expect(
      feedbackApi.exportLastDiagnostics("synthetic-report"),
    ).resolves.toBe(false);
    expect(invokeMock).toHaveBeenCalledWith(
      "feedback_export_last_diagnostics",
      { expectedReportId: "synthetic-report" },
    );
  });

  it("deletes only the matching local report through the native owner", async () => {
    invokeMock.mockResolvedValue(undefined);
    await feedbackApi.deleteLocalReport("synthetic-report");
    expect(invokeMock).toHaveBeenCalledWith("feedback_delete_local_report", {
      expectedReportId: "synthetic-report",
    });
  });

  it.each([
    { status: "unknown" },
    { status: "accepted", receipt: null, localSave: "unavailable" },
    { status: "accepted", receipt: null, localSave: "failed" },
  ])("returns upload result $status unchanged", async (result) => {
    invokeMock.mockResolvedValue(result);
    const input = { previewId: "synthetic-preview" };
    await expect(feedbackApi.submit(input)).resolves.toBe(result);
    expect(invokeMock).toHaveBeenCalledWith("submit_feedback", input);
  });
});

describe("feedback user-supplied context contract", () => {
  it("forwards related app IDs and optional contact as independent request fields", async () => {
    invokeMock.mockResolvedValue({ status: "unknown" });
    const input = {
      description: "Synthetic issue",
      relatedApps: ["codex", "other"] as ("codex" | "other")[],
      contact: "followup@example.invalid",
      includeDiagnostics: false,
      includeSites: false,
      screenshots: [],
    };
    await feedbackApi.prepare(input);
    expect(invokeMock).toHaveBeenCalledWith("prepare_feedback", input);
  });
});

it("reads extended-field support from the native owner without attempting an upload", async () => {
  invokeMock.mockResolvedValue({ extendedFields: false });
  await expect(feedbackApi.getFormCapabilities()).resolves.toEqual({
    extendedFields: false,
  });
  expect(invokeMock).toHaveBeenCalledOnce();
  expect(invokeMock).toHaveBeenCalledWith("feedback_get_form_capabilities");
});

describe("feedback frozen preview API", () => {
  it("prepares user input once through the native owner", async () => {
    const input = {
      description: "Synthetic draft",
      relatedApps: [],
      includeDiagnostics: false,
      includeSites: false,
      screenshots: [],
    };
    const prepared = {
      previewId: "synthetic-preview",
      files: [],
      applicationConfigs: null,
    };
    invokeMock.mockResolvedValue(prepared);
    await expect(feedbackApi.prepare(input)).resolves.toBe(prepared);
    expect(invokeMock).toHaveBeenCalledWith("prepare_feedback", input);
  });
  it("cancels only the supplied preview token", async () => {
    invokeMock.mockResolvedValue(undefined);
    await feedbackApi.cancelPreview("synthetic-preview");
    expect(invokeMock).toHaveBeenCalledWith("feedback_cancel_preview", {
      previewId: "synthetic-preview",
    });
  });
});
